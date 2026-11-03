//! Private annotation sets and their page layers.
//!
//! Every query is scoped to the requesting user: there is no route that
//! returns someone else's annotations, for admins neither. A set also
//! disappears from view when the user loses access to the part.
//!
//! Saving uses optimistic concurrency: `PUT .../pages/{n}` carries
//! `If-Match: "<revision>"` (`"0"` for a page never saved). If another device
//! saved in between, the server answers 409 with the current revision and
//! the client merges ([`musicus_api::ink::merge`]) and retries.

use crate::{
    access::ensure_part_readable,
    auth::AuthUser,
    db::now,
    error::{ApiResult, AppError, required},
    state::{App, AppState},
};
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use musicus_api::{
    AnnotationSet, Id, NewAnnotationSet, PageConflict, PageRevision, SavedPage, UpdateAnnotationSet, ink,
};
use std::collections::HashMap;

/// Pages are numbered from 0; this caps the page index of files whose page
/// count the server could not determine.
const MAX_PAGES: i64 = 2_000;

#[derive(sqlx::FromRow)]
struct SetRow {
    id: Id,
    part_id: Id,
    file_id: Id,
    sha256: String,
    version: i64,
    name: String,
    programme_id: Option<Id>,
    is_default: bool,
    created_at: i64,
    updated_at: i64,
}

const SET_SELECT: &str = "SELECT s.id, s.part_id, s.file_id, f.sha256,
        COALESCE((SELECT MAX(version) FROM part_files pf WHERE pf.part_id = s.part_id AND pf.file_id = s.file_id), 0) AS version,
        s.name, s.programme_id, s.is_default, s.created_at, s.updated_at
    FROM annotation_sets s JOIN files f ON f.id = s.file_id";

async fn sets_with_pages(app: &App, rows: Vec<SetRow>) -> ApiResult<Vec<AnnotationSet>> {
    let ids: Vec<Id> = rows.iter().map(|r| r.id).collect();
    let pages: Vec<(Id, i64, i64)> = sqlx::query_as(
        "SELECT set_id, page, revision FROM annotation_pages WHERE set_id IN (SELECT value FROM json_each(?1)) ORDER BY page",
    )
    .bind(crate::db::json_ids(&ids))
    .fetch_all(&app.db.read)
    .await?;
    let mut by_set: HashMap<Id, Vec<PageRevision>> = HashMap::new();
    for (set_id, page, revision) in pages {
        by_set.entry(set_id).or_default().push(PageRevision { page, revision });
    }
    Ok(rows
        .into_iter()
        .map(|r| AnnotationSet {
            pages: by_set.remove(&r.id).unwrap_or_default(),
            id: r.id,
            part_id: r.part_id,
            file_sha256: r.sha256,
            file_version: r.version,
            name: r.name,
            programme_id: r.programme_id,
            is_default: r.is_default,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// Loads one of the user's own sets; 404 for anything else.
async fn own_set(app: &App, user: &AuthUser, set_id: Id) -> ApiResult<SetRow> {
    let row: SetRow = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SET_SELECT} WHERE s.id = ?1 AND s.user_id = ?2")))
        .bind(set_id)
        .bind(user.id)
        .fetch_optional(&app.db.read)
        .await?
        .ok_or(AppError::NotFound)?;
    ensure_part_readable(app, user, row.part_id).await?;
    Ok(row)
}

async fn one_set(app: &App, user: &AuthUser, set_id: Id) -> ApiResult<Json<AnnotationSet>> {
    let row = own_set(app, user, set_id).await?;
    Ok(Json(sets_with_pages(app, vec![row]).await?.pop().ok_or(AppError::NotFound)?))
}

pub async fn list(
    State(app): State<AppState>,
    user: AuthUser,
    Path(part_id): Path<Id>,
) -> ApiResult<Json<Vec<AnnotationSet>>> {
    ensure_part_readable(&app, &user, part_id).await?;
    let rows: Vec<SetRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SET_SELECT} WHERE s.part_id = ?1 AND s.user_id = ?2 ORDER BY s.is_default DESC, s.updated_at DESC"
    )))
    .bind(part_id)
    .bind(user.id)
    .fetch_all(&app.db.read)
    .await?;
    Ok(Json(sets_with_pages(&app, rows).await?))
}

async fn check_programme(app: &App, programme_id: Option<Id>) -> ApiResult<()> {
    if let Some(id) = programme_id {
        let exists: Option<i64> =
            sqlx::query_scalar("SELECT 1 FROM programmes WHERE id = ?1").bind(id).fetch_optional(&app.db.read).await?;
        if exists.is_none() {
            return Err(AppError::BadRequest("unknown programme".into()));
        }
    }
    Ok(())
}

/// Creates a set on the part's current file version, or duplicates an
/// existing set (then on that set's file version).
pub async fn create(
    State(app): State<AppState>,
    user: AuthUser,
    Path(part_id): Path<Id>,
    Json(req): Json<NewAnnotationSet>,
) -> ApiResult<Json<AnnotationSet>> {
    ensure_part_readable(&app, &user, part_id).await?;
    let name = required("name", &req.name, 100)?;
    check_programme(&app, req.programme_id).await?;
    let source = match req.copy_from {
        Some(id) => {
            let src = own_set(&app, &user, id).await?;
            if src.part_id != part_id {
                return Err(AppError::BadRequest("the set to copy belongs to another part".into()));
            }
            Some(src)
        }
        None => None,
    };
    let file_id: Id = match &source {
        Some(src) => src.file_id,
        None => sqlx::query_scalar("SELECT file_id FROM part_files WHERE part_id = ?1 ORDER BY version DESC LIMIT 1")
            .bind(part_id)
            .fetch_optional(&app.db.read)
            .await?
            .ok_or_else(|| AppError::Conflict("this part has no file yet".into()))?,
    };
    let now = now();
    let mut tx = app.db.write.begin().await?;
    let first: bool =
        sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM annotation_sets WHERE user_id = ?1 AND part_id = ?2)")
            .bind(user.id)
            .bind(part_id)
            .fetch_one(&mut *tx)
            .await?;
    let id: Id = sqlx::query_scalar(
        "INSERT INTO annotation_sets (user_id, part_id, file_id, name, programme_id, is_default, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) RETURNING id",
    )
    .bind(user.id)
    .bind(part_id)
    .bind(file_id)
    .bind(name)
    .bind(req.programme_id)
    .bind(first)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(src) = source {
        sqlx::query(
            "INSERT INTO annotation_pages (set_id, page, revision, data, updated_at)
             SELECT ?1, page, 1, data, ?3 FROM annotation_pages WHERE set_id = ?2",
        )
        .bind(id)
        .bind(src.id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    one_set(&app, &user, id).await
}

pub async fn update(
    State(app): State<AppState>,
    user: AuthUser,
    Path(set_id): Path<Id>,
    Json(req): Json<UpdateAnnotationSet>,
) -> ApiResult<Json<AnnotationSet>> {
    let set = own_set(&app, &user, set_id).await?;
    let name = req.name.as_deref().map(|n| required("name", n, 100)).transpose()?;
    if let Some(programme) = req.programme_id {
        check_programme(&app, programme).await?;
    }
    let mut tx = app.db.write.begin().await?;
    if req.is_default == Some(true) {
        sqlx::query("UPDATE annotation_sets SET is_default = 0 WHERE user_id = ?1 AND part_id = ?2")
            .bind(user.id)
            .bind(set.part_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        "UPDATE annotation_sets SET name = COALESCE(?2, name),
                programme_id = CASE WHEN ?3 THEN ?4 ELSE programme_id END,
                is_default = COALESCE(?5, is_default), updated_at = ?6
         WHERE id = ?1",
    )
    .bind(set_id)
    .bind(name)
    .bind(req.programme_id.is_some())
    .bind(req.programme_id.flatten())
    .bind(req.is_default)
    .bind(now())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    one_set(&app, &user, set_id).await
}

pub async fn delete(State(app): State<AppState>, user: AuthUser, Path(set_id): Path<Id>) -> ApiResult<StatusCode> {
    own_set(&app, &user, set_id).await?;
    sqlx::query("DELETE FROM annotation_sets WHERE id = ?1 AND user_id = ?2")
        .bind(set_id)
        .bind(user.id)
        .execute(&app.db.write)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Moves a set to the part's current file version. Only allowed when the
/// page count is unchanged, so objects keep their place; the client should
/// ask the user to review the result.
pub async fn migrate(
    State(app): State<AppState>,
    user: AuthUser,
    Path(set_id): Path<Id>,
) -> ApiResult<Json<AnnotationSet>> {
    let set = own_set(&app, &user, set_id).await?;
    let (current, pages): (Id, Option<i64>) = sqlx::query_as(
        "SELECT pf.file_id, f.page_count FROM part_files pf JOIN files f ON f.id = pf.file_id
         WHERE pf.part_id = ?1 ORDER BY pf.version DESC LIMIT 1",
    )
    .bind(set.part_id)
    .fetch_one(&app.db.read)
    .await?;
    if current != set.file_id {
        let old_pages: Option<i64> = sqlx::query_scalar("SELECT page_count FROM files WHERE id = ?1")
            .bind(set.file_id)
            .fetch_one(&app.db.read)
            .await?;
        if pages.is_none() || pages != old_pages {
            return Err(AppError::Conflict("the new file version has a different page count".into()));
        }
        sqlx::query("UPDATE annotation_sets SET file_id = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(set_id)
            .bind(current)
            .bind(now())
            .execute(&app.db.write)
            .await?;
    }
    one_set(&app, &user, set_id).await
}

fn revision_etag(revision: i64) -> HeaderValue {
    HeaderValue::from_str(&format!("\"{revision}\"")).expect("digits")
}

/// The stored layer as `application/octet-stream` (see [`musicus_api::ink`]),
/// with `ETag: "<revision>"`. 404 if the page was never saved.
pub async fn get_page(
    State(app): State<AppState>,
    user: AuthUser,
    Path((set_id, page)): Path<(Id, i64)>,
) -> ApiResult<Response> {
    own_set(&app, &user, set_id).await?;
    let (revision, data): (i64, Vec<u8>) =
        sqlx::query_as("SELECT revision, data FROM annotation_pages WHERE set_id = ?1 AND page = ?2")
            .bind(set_id)
            .bind(page)
            .fetch_optional(&app.db.read)
            .await?
            .ok_or(AppError::NotFound)?;
    let layer = zstd::decode_all(data.as_slice()).map_err(AppError::internal)?;
    let mut response = layer.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    headers.insert(header::ETAG, revision_etag(revision));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    Ok(response)
}

fn expected_revision(headers: &HeaderMap) -> ApiResult<i64> {
    let value = headers
        .get(header::IF_MATCH)
        .ok_or(AppError::PreconditionRequired("send If-Match with the revision you edited (\"0\" for a new page)"))?;
    value
        .to_str()
        .ok()
        .map(|v| v.trim().trim_start_matches("W/").trim_matches('"'))
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| AppError::BadRequest("If-Match must be a revision number".into()))
}

pub async fn put_page(
    State(app): State<AppState>,
    user: AuthUser,
    Path((set_id, page)): Path<(Id, i64)>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let set = own_set(&app, &user, set_id).await?;
    let expected = expected_revision(&headers)?;
    let page_count: Option<i64> = sqlx::query_scalar("SELECT page_count FROM files WHERE id = ?1")
        .bind(set.file_id)
        .fetch_one(&app.db.read)
        .await?;
    if page < 0 || page >= page_count.unwrap_or(MAX_PAGES) {
        return Err(AppError::NotFound);
    }
    ink::decode(&body).map_err(|e| AppError::Unprocessable(e.to_string()))?;
    let compressed = zstd::encode_all(body.as_ref(), 3).map_err(AppError::internal)?;

    let now = now();
    let mut tx = app.db.write.begin().await?;
    let current: i64 = sqlx::query_scalar("SELECT revision FROM annotation_pages WHERE set_id = ?1 AND page = ?2")
        .bind(set_id)
        .bind(page)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(0);
    if current != expected {
        return Err(AppError::ConflictWith(
            serde_json::to_value(PageConflict { current_revision: current }).expect("serialises"),
        ));
    }
    let revision = current + 1;
    sqlx::query(
        "INSERT INTO annotation_pages (set_id, page, revision, data, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (set_id, page) DO UPDATE SET revision = excluded.revision, data = excluded.data, updated_at = excluded.updated_at",
    )
    .bind(set_id)
    .bind(page)
    .bind(revision)
    .bind(compressed)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE annotation_sets SET updated_at = ?2 WHERE id = ?1")
        .bind(set_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut response = Json(SavedPage { revision }).into_response();
    response.headers_mut().insert(header::ETAG, revision_etag(revision));
    Ok(response)
}
