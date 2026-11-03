//! Serves sheet files and thumbnails by hash, after the access check.
//!
//! Plain requests are answered from the RAM cache (one shared copy for
//! every musician opening the same piece); `Range` requests stream from
//! disk via tower-http so a client can start with the first pages.

use crate::{
    access::librarian_flag,
    auth::AuthUser,
    blobs::is_sha256_hex,
    config::ClientCache,
    error::{ApiResult, AppError},
    part_visible,
    state::{App, AppState},
};
use axum::{
    body::{Body, Bytes},
    extract::{Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use tower::ServiceExt;
use tower_http::services::ServeFile;

/// `(mime, thumbnail sha)` of a file the user may read.
async fn readable_file(app: &App, user: &AuthUser, sha256: &str) -> ApiResult<(String, Option<String>)> {
    if !is_sha256_hex(sha256) {
        return Err(AppError::NotFound);
    }
    sqlx::query_as(concat!(
        "SELECT f.mime, f.thumbnail_sha256 FROM files f WHERE f.sha256 = ?3
           AND EXISTS (SELECT 1 FROM part_files pf JOIN parts p ON p.id = pf.part_id WHERE pf.file_id = f.id AND ",
        part_visible!(),
        ")"
    ))
    .bind(user.id)
    .bind(librarian_flag(user))
    .bind(sha256)
    .fetch_optional(&app.db.read)
    .await?
    .ok_or(AppError::NotFound)
}

pub async fn get_file(
    State(app): State<AppState>,
    user: AuthUser,
    Path(sha256): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (mime, _) = readable_file(&app, &user, &sha256).await?;
    serve_blob(&app, &sha256, &mime, req).await
}

pub async fn get_thumbnail(
    State(app): State<AppState>,
    user: AuthUser,
    Path(sha256): Path<String>,
    req: Request,
) -> ApiResult<Response> {
    let (_, thumbnail) = readable_file(&app, &user, &sha256).await?;
    let thumbnail = thumbnail.ok_or(AppError::NotFound)?;
    serve_blob(&app, &thumbnail, "image/png", req).await
}

async fn serve_blob(app: &App, sha256: &str, mime: &str, req: Request) -> ApiResult<Response> {
    let etag = format!("\"{sha256}\"");
    let cache_control = match app.settings.client_cache {
        ClientCache::None => "private, no-store",
        ClientCache::Immutable => "private, max-age=31536000, immutable",
    };
    let not_modified = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"));

    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else if req.headers().contains_key(header::RANGE) {
        let path = app.blobs.path(sha256);
        ServeFile::new(path).oneshot(req).await.map_err(AppError::internal)?.map(Body::new)
    } else {
        let bytes = match app.cache.get(sha256) {
            Some(b) => b,
            None => {
                let b = Bytes::from(tokio::fs::read(app.blobs.path(sha256)).await?);
                app.cache.insert(sha256, b.clone());
                b
            }
        };
        let mut r = Response::new(Body::from(bytes));
        r.headers_mut().insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        r
    };
    let headers = response.headers_mut();
    if !not_modified {
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(mime).map_err(AppError::internal)?);
    }
    headers.insert(header::ETAG, HeaderValue::from_str(&etag).map_err(AppError::internal)?);
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
    headers.remove(header::LAST_MODIFIED);
    Ok(response)
}
