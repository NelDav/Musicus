//! Concert programmes: a dated, ordered list of scores. Clients use them to
//! download everything for tonight ahead of time.

use crate::{
    access::librarian_flag,
    auth::AuthUser,
    db::{json_ids, now},
    error::{ApiResult, AppError, optional, required},
    part_visible,
    routes::library::scores_by_ids,
    state::{App, AppState},
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use musicus_api::{Id, IdList, Programme, ProgrammeInput, ProgrammeItem, ProgrammeSummary};

/// Accepts `YYYY-MM-DD` with a plausible month and day.
fn valid_date(date: &str) -> bool {
    let b = date.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| date[r].parse::<u32>().ok();
    matches!((num(0..4), num(5..7), num(8..10)), (Some(_), Some(m), Some(d)) if (1..=12).contains(&m) && (1..=31).contains(&d))
}

fn clean(input: &ProgrammeInput) -> ApiResult<(String, Option<String>, Option<String>)> {
    let date = optional("date", input.date.as_deref(), 10)?;
    if date.as_deref().is_some_and(|d| !valid_date(d)) {
        return Err(AppError::BadRequest("the date must look like 2026-12-24".into()));
    }
    Ok((required("title", &input.title, 200)?, date, optional("notes", input.notes.as_deref(), 4000)?))
}

pub async fn list(State(app): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<ProgrammeSummary>>> {
    type Row = (Id, String, Option<String>, Option<String>, i64, i64);
    let rows: Vec<Row> = sqlx::query_as(concat!(
        "SELECT pr.id, pr.title, pr.date, pr.notes,
                (SELECT COUNT(*) FROM programme_items i WHERE i.programme_id = pr.id AND (?2 = 1 OR EXISTS (
                    SELECT 1 FROM parts p WHERE p.score_id = i.score_id AND ",
        part_visible!(),
        "))) AS visible,
                (SELECT COUNT(*) FROM programme_items i WHERE i.programme_id = pr.id) AS total
         FROM programmes pr ORDER BY pr.date IS NULL, pr.date DESC, pr.id DESC"
    ))
    .bind(user.id)
    .bind(librarian_flag(&user))
    .fetch_all(&app.db.read)
    .await?;
    Ok(Json(
        rows.into_iter()
            // Members only see programmes they play in; empty ones are for librarians.
            .filter(|r| user.is_librarian() || r.4 > 0)
            .map(|(id, title, date, notes, visible, _)| ProgrammeSummary {
                id,
                title,
                date,
                notes,
                item_count: visible,
            })
            .collect(),
    ))
}

async fn load(app: &App, user: &AuthUser, id: Id) -> ApiResult<Programme> {
    let (title, date, notes): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT title, date, notes FROM programmes WHERE id = ?1")
            .bind(id)
            .fetch_optional(&app.db.read)
            .await?
            .ok_or(AppError::NotFound)?;
    let items: Vec<(i64, Id)> =
        sqlx::query_as("SELECT position, score_id FROM programme_items WHERE programme_id = ?1 ORDER BY position")
            .bind(id)
            .fetch_all(&app.db.read)
            .await?;
    let ids: Vec<Id> = items.iter().map(|i| i.1).collect();
    let mut scores = scores_by_ids(app, user, &ids).await?.into_iter();
    // scores_by_ids keeps the order and drops invisible ones; pair them up again.
    let mut out = Vec::new();
    let mut next = scores.next();
    for (position, score_id) in items {
        if next.as_ref().is_some_and(|s| s.id == score_id) {
            out.push(ProgrammeItem { position, score: next.take().expect("checked") });
            next = scores.next();
        }
    }
    if out.is_empty() && !user.is_librarian() {
        return Err(AppError::NotFound);
    }
    Ok(Programme { id, title, date, notes, items: out })
}

pub async fn get(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<Json<Programme>> {
    Ok(Json(load(&app, &user, id).await?))
}

pub async fn create(
    State(app): State<AppState>,
    user: AuthUser,
    Json(input): Json<ProgrammeInput>,
) -> ApiResult<Json<Programme>> {
    user.require_librarian()?;
    let (title, date, notes) = clean(&input)?;
    let id: Id = sqlx::query_scalar(
        "INSERT INTO programmes (title, date, notes, created_at) VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(title)
    .bind(date)
    .bind(notes)
    .bind(now())
    .fetch_one(&app.db.write)
    .await?;
    Ok(Json(load(&app, &user, id).await?))
}

pub async fn update(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(input): Json<ProgrammeInput>,
) -> ApiResult<Json<Programme>> {
    user.require_librarian()?;
    let (title, date, notes) = clean(&input)?;
    let done = sqlx::query("UPDATE programmes SET title = ?2, date = ?3, notes = ?4 WHERE id = ?1")
        .bind(id)
        .bind(title)
        .bind(date)
        .bind(notes)
        .execute(&app.db.write)
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(load(&app, &user, id).await?))
}

pub async fn delete(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    user.require_librarian()?;
    let done = sqlx::query("DELETE FROM programmes WHERE id = ?1").bind(id).execute(&app.db.write).await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Replaces the pieces of a programme, in concert order.
pub async fn set_items(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<IdList>,
) -> ApiResult<Json<Programme>> {
    user.require_librarian()?;
    if req.ids.len() > 200 {
        return Err(AppError::BadRequest("at most 200 pieces per programme".into()));
    }
    let mut tx = app.db.write.begin().await?;
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM programmes WHERE id = ?1").bind(id).fetch_optional(&mut *tx).await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    sqlx::query("DELETE FROM programme_items WHERE programme_id = ?1").bind(id).execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO programme_items (programme_id, position, score_id) SELECT ?1, key, value FROM json_each(?2)",
    )
    .bind(id)
    .bind(json_ids(&req.ids))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(load(&app, &user, id).await?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert!(super::valid_date("2026-12-24"));
        assert!(!super::valid_date("2026-13-01"));
        assert!(!super::valid_date("24.12.2026"));
    }
}
