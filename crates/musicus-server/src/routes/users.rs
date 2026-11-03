//! User administration (admins only). Users never get a password from the
//! admin: they receive a one-time invite link and choose it themselves.

use crate::{
    auth::{AuthUser, new_token},
    db::now,
    error::{ApiResult, AppError, required},
    state::{App, AppState},
};
use axum::{
    Json,
    extract::{Path, State},
};
use musicus_api::{CreateUser, CreatedUser, Id, Invite, Role, UpdateUser, UserSummary};

/// Creates a fresh invite for a user, replacing older ones.
pub async fn issue_invite(app: &App, user_id: Id) -> ApiResult<Invite> {
    let (token, hash) = new_token();
    let expires_at = now() + app.settings.invite_ttl_secs;
    let mut tx = app.db.write.begin().await?;
    sqlx::query("DELETE FROM invites WHERE user_id = ?1").bind(user_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO invites (token_hash, user_id, expires_at) VALUES (?1, ?2, ?3)")
        .bind(hash)
        .bind(user_id)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Invite { user_id, token, expires_at })
}

/// Inserts a user without password. Used by the API and the CLI bootstrap.
pub async fn insert_user(app: &App, username: &str, display_name: &str, role: Role) -> ApiResult<Id> {
    let username = required("username", username, 64)?.to_lowercase();
    if !username.chars().all(|c| c.is_alphanumeric() || "._-".contains(c)) {
        return Err(AppError::BadRequest("usernames may contain letters, digits, '.', '_' and '-'".into()));
    }
    let display_name = required("display name", display_name, 100)?;
    let id = sqlx::query_scalar(
        "INSERT INTO users (username, display_name, role, created_at) VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(username)
    .bind(display_name)
    .bind(role.as_str())
    .bind(now())
    .fetch_one(&app.db.write)
    .await?;
    Ok(id)
}

async fn load_users(app: &App, only: Option<Id>) -> ApiResult<Vec<UserSummary>> {
    let rows: Vec<(Id, String, String, String, bool, bool, String)> = sqlx::query_as(
        "SELECT u.id, u.username, u.display_name, u.role, u.disabled, u.password_hash IS NOT NULL,
                COALESCE((SELECT json_group_array(group_id) FROM group_members WHERE user_id = u.id), '[]')
         FROM users u WHERE ?1 IS NULL OR u.id = ?1 ORDER BY u.display_name COLLATE NOCASE",
    )
    .bind(only)
    .fetch_all(&app.db.read)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, username, display_name, role, disabled, has_password, groups)| UserSummary {
            id,
            username,
            display_name,
            role: Role::parse(&role).unwrap_or(Role::Member),
            disabled,
            has_password,
            group_ids: serde_json::from_str(&groups).unwrap_or_default(),
        })
        .collect())
}

pub async fn list(State(app): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<UserSummary>>> {
    user.require_admin()?;
    Ok(Json(load_users(&app, None).await?))
}

pub async fn create(
    State(app): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateUser>,
) -> ApiResult<Json<CreatedUser>> {
    user.require_admin()?;
    let id = insert_user(&app, &req.username, &req.display_name, req.role).await?;
    let invite = issue_invite(&app, id).await?;
    let user = load_users(&app, Some(id)).await?.pop().ok_or(AppError::NotFound)?;
    Ok(Json(CreatedUser { user, invite }))
}

pub async fn update(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<UpdateUser>,
) -> ApiResult<Json<UserSummary>> {
    user.require_admin()?;
    if id == user.id && (req.disabled == Some(true) || req.role.is_some_and(|r| r != Role::Admin)) {
        return Err(AppError::Conflict("you cannot disable yourself or remove your own admin role".into()));
    }
    let display_name = req.display_name.as_deref().map(|d| required("display name", d, 100)).transpose()?;
    let mut tx = app.db.write.begin().await?;
    let done = sqlx::query(
        "UPDATE users SET display_name = COALESCE(?2, display_name), role = COALESCE(?3, role),
                          disabled = COALESCE(?4, disabled) WHERE id = ?1",
    )
    .bind(id)
    .bind(display_name)
    .bind(req.role.map(Role::as_str))
    .bind(req.disabled)
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    if req.disabled == Some(true) {
        // Disabled users are logged out everywhere; their annotations stay.
        sqlx::query("DELETE FROM sessions WHERE user_id = ?1").bind(id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM invites WHERE user_id = ?1").bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(load_users(&app, Some(id)).await?.pop().ok_or(AppError::NotFound)?))
}

/// A new invite link, e.g. when a user forgot their password.
pub async fn reinvite(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<Json<Invite>> {
    user.require_admin()?;
    let disabled: bool = sqlx::query_scalar("SELECT disabled FROM users WHERE id = ?1")
        .bind(id)
        .fetch_optional(&app.db.read)
        .await?
        .ok_or(AppError::NotFound)?;
    if disabled {
        return Err(AppError::Conflict("enable the user before inviting them".into()));
    }
    Ok(Json(issue_invite(&app, id).await?))
}
