//! Instruments (controlled vocabulary) and groups with their members and
//! grants. Admins manage groups and membership; librarians manage
//! instruments and grants.

use crate::{
    auth::AuthUser,
    db::json_ids,
    error::{ApiResult, AppError, optional, required},
    state::{App, AppState},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use musicus_api::{Group, GroupInput, Id, IdList, Instrument, InstrumentInput};

// --- Instruments -----------------------------------------------------------

pub async fn list_instruments(State(app): State<AppState>, _user: AuthUser) -> ApiResult<Json<Vec<Instrument>>> {
    let rows: Vec<(Id, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, family FROM instruments ORDER BY name COLLATE NOCASE")
            .fetch_all(&app.db.read)
            .await?;
    Ok(Json(rows.into_iter().map(|(id, name, family)| Instrument { id, name, family }).collect()))
}

pub async fn create_instrument(
    State(app): State<AppState>,
    user: AuthUser,
    Json(req): Json<InstrumentInput>,
) -> ApiResult<Json<Instrument>> {
    user.require_librarian()?;
    let name = required("name", &req.name, 80)?;
    let family = optional("family", req.family.as_deref(), 80)?;
    let id = sqlx::query_scalar("INSERT INTO instruments (name, family) VALUES (?1, ?2) RETURNING id")
        .bind(&name)
        .bind(&family)
        .fetch_one(&app.db.write)
        .await?;
    Ok(Json(Instrument { id, name, family }))
}

pub async fn update_instrument(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<InstrumentInput>,
) -> ApiResult<Json<Instrument>> {
    user.require_librarian()?;
    let name = required("name", &req.name, 80)?;
    let family = optional("family", req.family.as_deref(), 80)?;
    let done = sqlx::query("UPDATE instruments SET name = ?2, family = ?3 WHERE id = ?1")
        .bind(id)
        .bind(&name)
        .bind(&family)
        .execute(&app.db.write)
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(Instrument { id, name, family }))
}

/// Fails with 409 while parts still use the instrument.
pub async fn delete_instrument(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
) -> ApiResult<StatusCode> {
    user.require_librarian()?;
    let done = sqlx::query("DELETE FROM instruments WHERE id = ?1").bind(id).execute(&app.db.write).await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

// --- Groups ----------------------------------------------------------------

async fn load_groups(app: &App, only: Option<Id>) -> ApiResult<Vec<Group>> {
    let rows: Vec<(Id, String, String, String, String)> = sqlx::query_as(
        "SELECT g.id, g.name,
                (SELECT json_group_array(user_id) FROM group_members WHERE group_id = g.id),
                (SELECT json_group_array(instrument_id) FROM group_instrument_grants WHERE group_id = g.id),
                (SELECT json_group_array(score_id) FROM group_score_grants WHERE group_id = g.id)
         FROM groups g WHERE ?1 IS NULL OR g.id = ?1 ORDER BY g.name COLLATE NOCASE",
    )
    .bind(only)
    .fetch_all(&app.db.read)
    .await?;
    let ids = |json: &str| -> Vec<Id> {
        let mut v: Vec<Id> = serde_json::from_str(json).unwrap_or_default();
        v.sort_unstable();
        v
    };
    Ok(rows
        .into_iter()
        .map(|(id, name, members, instruments, scores)| Group {
            id,
            name,
            member_ids: ids(&members),
            instrument_ids: ids(&instruments),
            score_ids: ids(&scores),
        })
        .collect())
}

async fn one_group(app: &App, id: Id) -> ApiResult<Json<Group>> {
    Ok(Json(load_groups(app, Some(id)).await?.pop().ok_or(AppError::NotFound)?))
}

pub async fn list_groups(State(app): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<Group>>> {
    user.require_librarian()?;
    Ok(Json(load_groups(&app, None).await?))
}

pub async fn create_group(
    State(app): State<AppState>,
    user: AuthUser,
    Json(req): Json<GroupInput>,
) -> ApiResult<Json<Group>> {
    user.require_admin()?;
    let name = required("name", &req.name, 80)?;
    let id: Id = sqlx::query_scalar("INSERT INTO groups (name) VALUES (?1) RETURNING id")
        .bind(name)
        .fetch_one(&app.db.write)
        .await?;
    one_group(&app, id).await
}

pub async fn rename_group(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<GroupInput>,
) -> ApiResult<Json<Group>> {
    user.require_admin()?;
    let name = required("name", &req.name, 80)?;
    let done =
        sqlx::query("UPDATE groups SET name = ?2 WHERE id = ?1").bind(id).bind(name).execute(&app.db.write).await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    one_group(&app, id).await
}

pub async fn delete_group(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    user.require_admin()?;
    let done = sqlx::query("DELETE FROM groups WHERE id = ?1").bind(id).execute(&app.db.write).await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Which link table a list replacement targets.
#[derive(Clone, Copy)]
enum Link {
    Members,
    Instruments,
    Scores,
}

async fn replace_links(app: &App, group: Id, link: Link, ids: &[Id]) -> ApiResult<()> {
    let mut tx = app.db.write.begin().await?;
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM groups WHERE id = ?1").bind(group).fetch_optional(&mut *tx).await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let (delete, insert) = match link {
        Link::Members => (
            "DELETE FROM group_members WHERE group_id = ?1",
            "INSERT OR IGNORE INTO group_members (group_id, user_id) SELECT ?1, value FROM json_each(?2)",
        ),
        Link::Instruments => (
            "DELETE FROM group_instrument_grants WHERE group_id = ?1",
            "INSERT OR IGNORE INTO group_instrument_grants (group_id, instrument_id) SELECT ?1, value FROM json_each(?2)",
        ),
        Link::Scores => (
            "DELETE FROM group_score_grants WHERE group_id = ?1",
            "INSERT OR IGNORE INTO group_score_grants (group_id, score_id) SELECT ?1, value FROM json_each(?2)",
        ),
    };
    sqlx::query(delete).bind(group).execute(&mut *tx).await?;
    // Unknown ids fail the foreign key and roll everything back (409).
    sqlx::query(insert).bind(group).bind(json_ids(ids)).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn set_members(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<IdList>,
) -> ApiResult<Json<Group>> {
    user.require_admin()?;
    replace_links(&app, id, Link::Members, &req.ids).await?;
    one_group(&app, id).await
}

pub async fn set_instrument_grants(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<IdList>,
) -> ApiResult<Json<Group>> {
    user.require_librarian()?;
    replace_links(&app, id, Link::Instruments, &req.ids).await?;
    one_group(&app, id).await
}

pub async fn set_score_grants(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<IdList>,
) -> ApiResult<Json<Group>> {
    user.require_librarian()?;
    replace_links(&app, id, Link::Scores, &req.ids).await?;
    one_group(&app, id).await
}
