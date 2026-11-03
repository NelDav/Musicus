//! Scores, parts, uploads and file versions.

use crate::{
    access::{ensure_part_readable, librarian_flag},
    auth::AuthUser,
    blobs::UploadError,
    db::{json_ids, now},
    error::{ApiResult, AppError, optional, required},
    part_visible,
    processors::{self, Processed},
    state::{App, AppState},
};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::StatusCode,
};
use musicus_api::{FileVersion, Id, Part, PartInput, Score, ScoreInput};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::io::AsyncReadExt;

#[derive(sqlx::FromRow)]
pub struct ScoreRow {
    id: Id,
    title: String,
    subtitle: Option<String>,
    composer: Option<String>,
    arranger: Option<String>,
    genre: Option<String>,
    tags: String,
    notes: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PartRow {
    id: Id,
    score_id: Id,
    label: String,
    voice: Option<i64>,
    transposition: Option<String>,
    notes: Option<String>,
    instruments: String,
    version: Option<i64>,
    sha256: Option<String>,
    mime: Option<String>,
    size_bytes: Option<i64>,
    page_count: Option<i64>,
    has_thumbnail: Option<bool>,
    uploaded_at: Option<i64>,
}

impl From<PartRow> for Part {
    fn from(r: PartRow) -> Self {
        let file = match (r.version, r.sha256, r.mime) {
            (Some(version), Some(sha256), Some(mime)) => Some(FileVersion {
                version,
                sha256,
                mime,
                size_bytes: r.size_bytes.unwrap_or(0),
                page_count: r.page_count,
                has_thumbnail: r.has_thumbnail.unwrap_or(false),
                uploaded_at: r.uploaded_at.unwrap_or(0),
            }),
            _ => None,
        };
        let mut instrument_ids: Vec<Id> = serde_json::from_str(&r.instruments).unwrap_or_default();
        instrument_ids.sort_unstable();
        Part {
            id: r.id,
            score_id: r.score_id,
            label: r.label,
            instrument_ids,
            voice: r.voice,
            transposition: r.transposition,
            notes: r.notes,
            file,
        }
    }
}

/// Parts the user may read, restricted to some scores or to one part.
pub async fn visible_parts(
    app: &App,
    user: &AuthUser,
    score_ids: Option<&[Id]>,
    part_id: Option<Id>,
) -> ApiResult<Vec<Part>> {
    let rows: Vec<PartRow> = sqlx::query_as(concat!(
        "SELECT p.id, p.score_id, p.label, p.voice, p.transposition, p.notes,
                (SELECT json_group_array(instrument_id) FROM part_instruments WHERE part_id = p.id) AS instruments,
                pf.version, f.sha256, f.mime, f.size_bytes, f.page_count,
                f.thumbnail_sha256 IS NOT NULL AS has_thumbnail, pf.uploaded_at
         FROM parts p
         LEFT JOIN part_files pf ON pf.part_id = p.id
              AND pf.version = (SELECT MAX(version) FROM part_files x WHERE x.part_id = p.id)
         LEFT JOIN files f ON f.id = pf.file_id
         WHERE (?3 IS NULL OR p.score_id IN (SELECT value FROM json_each(?3)))
           AND (?4 IS NULL OR p.id = ?4) AND ",
        part_visible!(),
        " ORDER BY p.score_id, p.voice IS NULL, p.voice, p.label COLLATE NOCASE"
    ))
    .bind(user.id)
    .bind(librarian_flag(user))
    .bind(score_ids.map(json_ids))
    .bind(part_id)
    .fetch_all(&app.db.read)
    .await?;
    Ok(rows.into_iter().map(Part::from).collect())
}

/// Attaches the user's visible parts to score rows.
pub async fn with_parts(app: &App, user: &AuthUser, rows: Vec<ScoreRow>) -> ApiResult<Vec<Score>> {
    let ids: Vec<Id> = rows.iter().map(|r| r.id).collect();
    let mut parts: HashMap<Id, Vec<Part>> = HashMap::new();
    for part in visible_parts(app, user, Some(&ids), None).await? {
        parts.entry(part.score_id).or_default().push(part);
    }
    Ok(rows
        .into_iter()
        .map(|r| Score {
            parts: parts.remove(&r.id).unwrap_or_default(),
            id: r.id,
            title: r.title,
            subtitle: r.subtitle,
            composer: r.composer,
            arranger: r.arranger,
            genre: r.genre,
            tags: serde_json::from_str(&r.tags).unwrap_or_default(),
            notes: r.notes,
        })
        .collect())
}

/// Scores by id, in the given order, that the user may see: librarians see
/// all, members those with at least one readable part.
pub async fn scores_by_ids(app: &App, user: &AuthUser, ids: &[Id]) -> ApiResult<Vec<Score>> {
    let rows: Vec<ScoreRow> = sqlx::query_as(concat!(
        "SELECT s.id, s.title, s.subtitle, s.composer, s.arranger, s.genre, s.tags, s.notes FROM scores s
         WHERE s.id IN (SELECT value FROM json_each(?3))
           AND (?2 = 1 OR EXISTS (SELECT 1 FROM parts p WHERE p.score_id = s.id AND ",
        part_visible!(),
        "))"
    ))
    .bind(user.id)
    .bind(librarian_flag(user))
    .bind(json_ids(ids))
    .fetch_all(&app.db.read)
    .await?;
    let scores: HashMap<Id, Score> = with_parts(app, user, rows).await?.into_iter().map(|s| (s.id, s)).collect();
    // A programme may list a piece twice (an encore), hence clone.
    Ok(ids.iter().filter_map(|id| scores.get(id).cloned()).collect())
}

/// Turns user input into an FTS5 query: every word must match as a prefix.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|w| !w.is_empty())
        .take(10)
        .map(|w| format!("\"{w}\"*"))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[derive(Deserialize)]
pub struct ScoreQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    tag: Option<String>,
}

pub async fn list_scores(
    State(app): State<AppState>,
    user: AuthUser,
    Query(query): Query<ScoreQuery>,
) -> ApiResult<Json<Vec<Score>>> {
    let rows: Vec<ScoreRow> = sqlx::query_as(concat!(
        "SELECT s.id, s.title, s.subtitle, s.composer, s.arranger, s.genre, s.tags, s.notes FROM scores s
         WHERE (?3 IS NULL OR s.id IN (SELECT rowid FROM score_fts WHERE score_fts MATCH ?3))
           AND (?4 IS NULL OR s.genre = ?4 COLLATE NOCASE)
           AND (?5 IS NULL OR EXISTS (SELECT 1 FROM json_each(s.tags) WHERE value = ?5 COLLATE NOCASE))
           AND (?2 = 1 OR EXISTS (SELECT 1 FROM parts p WHERE p.score_id = s.id AND ",
        part_visible!(),
        ")) ORDER BY s.title COLLATE NOCASE LIMIT 1000"
    ))
    .bind(user.id)
    .bind(librarian_flag(&user))
    .bind(query.q.as_deref().and_then(fts_query))
    .bind(query.genre.filter(|g| !g.trim().is_empty()))
    .bind(query.tag.filter(|t| !t.trim().is_empty()))
    .fetch_all(&app.db.read)
    .await?;
    Ok(Json(with_parts(&app, &user, rows).await?))
}

pub async fn get_score(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<Json<Score>> {
    scores_by_ids(&app, &user, &[id]).await?.pop().map(Json).ok_or(AppError::NotFound)
}

struct CleanScore {
    title: String,
    subtitle: Option<String>,
    composer: Option<String>,
    arranger: Option<String>,
    genre: Option<String>,
    tags: String,
    notes: Option<String>,
}

fn clean_score(input: &ScoreInput) -> ApiResult<CleanScore> {
    let mut tags: Vec<String> = Vec::new();
    for tag in &input.tags {
        let tag = required("tag", tag, 40)?;
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            tags.push(tag);
        }
    }
    if tags.len() > 30 {
        return Err(AppError::BadRequest("at most 30 tags".into()));
    }
    Ok(CleanScore {
        title: required("title", &input.title, 200)?,
        subtitle: optional("subtitle", input.subtitle.as_deref(), 200)?,
        composer: optional("composer", input.composer.as_deref(), 200)?,
        arranger: optional("arranger", input.arranger.as_deref(), 200)?,
        genre: optional("genre", input.genre.as_deref(), 60)?,
        tags: serde_json::to_string(&tags).expect("tags serialise"),
        notes: optional("notes", input.notes.as_deref(), 4000)?,
    })
}

pub async fn create_score(
    State(app): State<AppState>,
    user: AuthUser,
    Json(input): Json<ScoreInput>,
) -> ApiResult<Json<Score>> {
    user.require_librarian()?;
    let s = clean_score(&input)?;
    let now = now();
    let id: Id = sqlx::query_scalar(
        "INSERT INTO scores (title, subtitle, composer, arranger, genre, tags, notes, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8) RETURNING id",
    )
    .bind(s.title)
    .bind(s.subtitle)
    .bind(s.composer)
    .bind(s.arranger)
    .bind(s.genre)
    .bind(s.tags)
    .bind(s.notes)
    .bind(now)
    .fetch_one(&app.db.write)
    .await?;
    get_score(State(app), user, Path(id)).await
}

pub async fn update_score(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(input): Json<ScoreInput>,
) -> ApiResult<Json<Score>> {
    user.require_librarian()?;
    let s = clean_score(&input)?;
    let done = sqlx::query(
        "UPDATE scores SET title = ?2, subtitle = ?3, composer = ?4, arranger = ?5, genre = ?6, tags = ?7,
                           notes = ?8, updated_at = ?9 WHERE id = ?1",
    )
    .bind(id)
    .bind(s.title)
    .bind(s.subtitle)
    .bind(s.composer)
    .bind(s.arranger)
    .bind(s.genre)
    .bind(s.tags)
    .bind(s.notes)
    .bind(now())
    .execute(&app.db.write)
    .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    get_score(State(app), user, Path(id)).await
}

/// Deletes the score with all parts and the annotations drawn on them.
/// Files stay in the blob store.
pub async fn delete_score(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    user.require_librarian()?;
    let done = sqlx::query("DELETE FROM scores WHERE id = ?1").bind(id).execute(&app.db.write).await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

struct CleanPart {
    label: String,
    voice: Option<i64>,
    transposition: Option<String>,
    notes: Option<String>,
    instruments: String,
}

fn clean_part(input: &PartInput) -> ApiResult<CleanPart> {
    if input.voice.is_some_and(|v| !(0..=99).contains(&v)) {
        return Err(AppError::BadRequest("the voice must be between 0 and 99".into()));
    }
    if input.instrument_ids.len() > 20 {
        return Err(AppError::BadRequest("at most 20 instruments per part".into()));
    }
    Ok(CleanPart {
        label: required("label", &input.label, 120)?,
        voice: input.voice,
        transposition: optional("transposition", input.transposition.as_deref(), 12)?,
        notes: optional("notes", input.notes.as_deref(), 2000)?,
        instruments: json_ids(&input.instrument_ids),
    })
}

async fn one_part(app: &App, user: &AuthUser, id: Id) -> ApiResult<Json<Part>> {
    visible_parts(app, user, None, Some(id)).await?.pop().map(Json).ok_or(AppError::NotFound)
}

pub async fn create_part(
    State(app): State<AppState>,
    user: AuthUser,
    Path(score_id): Path<Id>,
    Json(input): Json<PartInput>,
) -> ApiResult<Json<Part>> {
    user.require_librarian()?;
    let p = clean_part(&input)?;
    let mut tx = app.db.write.begin().await?;
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM scores WHERE id = ?1").bind(score_id).fetch_optional(&mut *tx).await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let id: Id = sqlx::query_scalar(
        "INSERT INTO parts (score_id, label, voice, transposition, notes, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(score_id)
    .bind(p.label)
    .bind(p.voice)
    .bind(p.transposition)
    .bind(p.notes)
    .bind(now())
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("INSERT OR IGNORE INTO part_instruments (part_id, instrument_id) SELECT ?1, value FROM json_each(?2)")
        .bind(id)
        .bind(p.instruments)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    one_part(&app, &user, id).await
}

pub async fn update_part(
    State(app): State<AppState>,
    user: AuthUser,
    Path(id): Path<Id>,
    Json(input): Json<PartInput>,
) -> ApiResult<Json<Part>> {
    user.require_librarian()?;
    let p = clean_part(&input)?;
    let mut tx = app.db.write.begin().await?;
    let done = sqlx::query("UPDATE parts SET label = ?2, voice = ?3, transposition = ?4, notes = ?5 WHERE id = ?1")
        .bind(id)
        .bind(p.label)
        .bind(p.voice)
        .bind(p.transposition)
        .bind(p.notes)
        .execute(&mut *tx)
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    sqlx::query("DELETE FROM part_instruments WHERE part_id = ?1").bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT OR IGNORE INTO part_instruments (part_id, instrument_id) SELECT ?1, value FROM json_each(?2)")
        .bind(id)
        .bind(p.instruments)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    one_part(&app, &user, id).await
}

pub async fn delete_part(State(app): State<AppState>, user: AuthUser, Path(id): Path<Id>) -> ApiResult<StatusCode> {
    user.require_librarian()?;
    let done = sqlx::query("DELETE FROM parts WHERE id = ?1").bind(id).execute(&app.db.write).await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Uploads a new file version of a part. The body is the raw file.
/// Uploading the current file again creates no new version.
pub async fn upload_file(
    State(app): State<AppState>,
    user: AuthUser,
    Path(part_id): Path<Id>,
    body: Body,
) -> ApiResult<Json<Part>> {
    user.require_librarian()?;
    let exists: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM parts WHERE id = ?1").bind(part_id).fetch_optional(&app.db.read).await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    let pending = app.blobs.receive(body, app.settings.max_upload_bytes).await.map_err(|e| match e {
        UploadError::TooLarge(limit) => AppError::PayloadTooLarge(limit),
        UploadError::Body(e) => AppError::BadRequest(format!("upload interrupted: {e}")),
        UploadError::Io(e) => AppError::internal(e),
    })?;
    let result = store_upload(&app, part_id, &pending).await;
    if result.is_err() {
        app.blobs.discard(&pending).await;
    }
    result?;
    one_part(&app, &user, part_id).await
}

async fn store_upload(app: &App, part_id: Id, pending: &crate::blobs::PendingBlob) -> ApiResult<()> {
    let mut head = vec![0u8; 1024];
    let n = tokio::fs::File::open(&pending.tmp_path).await?.read(&mut head).await?;
    let mime =
        processors::sniff_mime(&head[..n]).ok_or(AppError::UnsupportedMediaType("only PDF files are supported"))?;

    let known: Option<Id> = sqlx::query_scalar("SELECT id FROM files WHERE sha256 = ?1")
        .bind(&pending.sha256)
        .fetch_optional(&app.db.read)
        .await?;
    let file_id = match known {
        Some(id) => {
            app.blobs.commit(pending).await?;
            id
        }
        None => {
            let data = tokio::fs::read(&pending.tmp_path).await?;
            // A panic inside the renderer must not take the upload down.
            let processed = tokio::task::spawn_blocking(move || processors::process_pdf(data))
                .await
                .unwrap_or(Processed { mime, page_count: None, thumbnail_png: None });
            let thumbnail = match processed.thumbnail_png {
                Some(png) => Some(app.blobs.put_bytes(&png).await?),
                None => None,
            };
            app.blobs.commit(pending).await?;
            sqlx::query(
                "INSERT INTO files (sha256, mime, size_bytes, page_count, thumbnail_sha256, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (sha256) DO NOTHING",
            )
            .bind(&pending.sha256)
            .bind(processed.mime)
            .bind(pending.size as i64)
            .bind(processed.page_count)
            .bind(thumbnail)
            .bind(now())
            .execute(&app.db.write)
            .await?;
            sqlx::query_scalar("SELECT id FROM files WHERE sha256 = ?1")
                .bind(&pending.sha256)
                .fetch_one(&app.db.write)
                .await?
        }
    };

    let mut tx = app.db.write.begin().await?;
    let latest: Option<(i64, Id)> =
        sqlx::query_as("SELECT version, file_id FROM part_files WHERE part_id = ?1 ORDER BY version DESC LIMIT 1")
            .bind(part_id)
            .fetch_optional(&mut *tx)
            .await?;
    if latest.is_none_or(|(_, current)| current != file_id) {
        sqlx::query("INSERT INTO part_files (part_id, version, file_id, uploaded_at) VALUES (?1, ?2, ?3, ?4)")
            .bind(part_id)
            .bind(latest.map_or(1, |(v, _)| v + 1))
            .bind(file_id)
            .bind(now())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// All file versions of a part, newest first.
pub async fn list_versions(
    State(app): State<AppState>,
    user: AuthUser,
    Path(part_id): Path<Id>,
) -> ApiResult<Json<Vec<FileVersion>>> {
    ensure_part_readable(&app, &user, part_id).await?;
    type VersionRow = (i64, String, String, i64, Option<i64>, bool, i64);
    let rows: Vec<VersionRow> = sqlx::query_as(
        "SELECT pf.version, f.sha256, f.mime, f.size_bytes, f.page_count, f.thumbnail_sha256 IS NOT NULL, pf.uploaded_at
         FROM part_files pf JOIN files f ON f.id = pf.file_id WHERE pf.part_id = ?1 ORDER BY pf.version DESC",
    )
    .bind(part_id)
    .fetch_all(&app.db.read)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(version, sha256, mime, size_bytes, page_count, has_thumbnail, uploaded_at)| FileVersion {
                version,
                sha256,
                mime,
                size_bytes,
                page_count,
                has_thumbnail,
                uploaded_at,
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::fts_query;

    #[test]
    fn fts_query_quotes_words_and_drops_operators() {
        assert_eq!(fts_query("Fučík  march"), Some("\"Fučík\"* \"march\"*".into()));
        assert_eq!(fts_query("\" OR NEAR("), Some("\"OR\"* \"NEAR\"*".into()));
        assert_eq!(fts_query("  ()  "), None);
    }
}
