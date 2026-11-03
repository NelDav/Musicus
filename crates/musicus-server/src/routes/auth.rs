//! Login, logout, invites, password changes and the user's own sessions.

use crate::{
    auth::{self, AuthUser, ClientIp, check_password_policy, create_session, session_cookie, token_hash},
    db::now,
    error::{ApiResult, AppError},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use musicus_api::{
    AcceptInvite, ChangePassword, CurrentUser, Id, InviteInfo, LoginRequest, LoginResponse, Role, SessionInfo,
};

fn user_agent(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok())
}

pub async fn current_user(app: &AppState, id: Id) -> ApiResult<CurrentUser> {
    let (username, display_name, role): (String, String, String) =
        sqlx::query_as("SELECT username, display_name, role FROM users WHERE id = ?1")
            .bind(id)
            .fetch_one(&app.db.read)
            .await?;
    Ok(CurrentUser { id, username, display_name, role: Role::parse(&role).unwrap_or(Role::Member) })
}

/// Responds with the user and either a cookie (web) or a bearer token (app).
async fn logged_in(app: &AppState, user_id: Id, bearer: bool, headers: &HeaderMap) -> ApiResult<Response> {
    let token = create_session(app, user_id, user_agent(headers)).await?;
    let user = current_user(app, user_id).await?;
    if bearer {
        return Ok(Json(LoginResponse { user, token: Some(token) }).into_response());
    }
    let mut response = Json(LoginResponse { user, token: None }).into_response();
    response.headers_mut().insert(header::SET_COOKIE, session_cookie(app, Some(&token)));
    Ok(response)
}

pub async fn login(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Response> {
    let username = req.username.trim().to_lowercase();
    let user_key = format!("user:{username}");
    let keys = [user_key.clone(), format!("ip:{ip}")];
    app.limiter.check(&keys)?;

    let row: Option<(Id, Option<String>, bool)> =
        sqlx::query_as("SELECT id, password_hash, disabled FROM users WHERE username = ?1")
            .bind(&username)
            .fetch_optional(&app.db.read)
            .await?;
    let hash = row.as_ref().and_then(|r| r.1.as_deref());
    let password_ok = auth::verify_password(&app, hash, &req.password).await?;
    match row {
        Some((id, _, false)) if password_ok => {
            app.limiter.clear(&user_key);
            logged_in(&app, id, req.bearer, &headers).await
        }
        _ => {
            app.limiter.record_failure(&keys);
            Err(AppError::Unauthorized)
        }
    }
}

pub async fn logout(State(app): State<AppState>, user: AuthUser) -> ApiResult<Response> {
    sqlx::query("DELETE FROM sessions WHERE id = ?1").bind(user.session_id).execute(&app.db.write).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(header::SET_COOKIE, session_cookie(&app, None));
    Ok(response)
}

pub async fn me(State(app): State<AppState>, user: AuthUser) -> ApiResult<Json<CurrentUser>> {
    Ok(Json(current_user(&app, user.id).await?))
}

async fn invite_user(app: &AppState, token: &str) -> ApiResult<(Id, String, String)> {
    sqlx::query_as(
        "SELECT u.id, u.username, u.display_name FROM invites i JOIN users u ON u.id = i.user_id
         WHERE i.token_hash = ?1 AND i.expires_at > ?2 AND u.disabled = 0",
    )
    .bind(token_hash(token))
    .bind(now())
    .fetch_optional(&app.db.read)
    .await?
    .ok_or(AppError::NotFound)
}

pub async fn invite_info(State(app): State<AppState>, Path(token): Path<String>) -> ApiResult<Json<InviteInfo>> {
    let (_, username, display_name) = invite_user(&app, &token).await?;
    Ok(Json(InviteInfo { username, display_name }))
}

pub async fn accept_invite(
    State(app): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Json(req): Json<AcceptInvite>,
) -> ApiResult<Response> {
    let (user_id, _, _) = invite_user(&app, &token).await?;
    check_password_policy(&req.password)?;
    let hash = auth::hash_password(&app, &req.password).await?;
    let mut tx = app.db.write.begin().await?;
    sqlx::query("UPDATE users SET password_hash = ?2 WHERE id = ?1").bind(user_id).bind(hash).execute(&mut *tx).await?;
    // A reset invalidates every invite and every existing session.
    sqlx::query("DELETE FROM invites WHERE user_id = ?1").bind(user_id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1").bind(user_id).execute(&mut *tx).await?;
    tx.commit().await?;
    logged_in(&app, user_id, false, &headers).await
}

pub async fn change_password(
    State(app): State<AppState>,
    user: AuthUser,
    Json(req): Json<ChangePassword>,
) -> ApiResult<StatusCode> {
    let hash: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
        .bind(user.id)
        .fetch_one(&app.db.read)
        .await?;
    if !auth::verify_password(&app, hash.as_deref(), &req.current_password).await? {
        return Err(AppError::Forbidden("the current password is wrong"));
    }
    check_password_policy(&req.new_password)?;
    let new_hash = auth::hash_password(&app, &req.new_password).await?;
    let mut tx = app.db.write.begin().await?;
    sqlx::query("UPDATE users SET password_hash = ?2 WHERE id = ?1")
        .bind(user.id)
        .bind(new_hash)
        .execute(&mut *tx)
        .await?;
    // Log out every other device.
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1 AND id <> ?2")
        .bind(user.id)
        .bind(user.session_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_sessions(State(app): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<SessionInfo>>> {
    let rows: Vec<(Id, i64, i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT id, created_at, last_seen_at, expires_at, user_agent FROM sessions
         WHERE user_id = ?1 AND expires_at > ?2 ORDER BY last_seen_at DESC",
    )
    .bind(user.id)
    .bind(now())
    .fetch_all(&app.db.read)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, created_at, last_seen_at, expires_at, user_agent)| SessionInfo {
                id,
                created_at,
                last_seen_at,
                expires_at,
                user_agent,
                current: id == user.session_id,
            })
            .collect(),
    ))
}

pub async fn revoke_session(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    let done = sqlx::query("DELETE FROM sessions WHERE id = ?1 AND user_id = ?2")
        .bind(id)
        .bind(user.id)
        .execute(&app.db.write)
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}
