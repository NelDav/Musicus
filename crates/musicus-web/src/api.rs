//! HTTP access to the Musicus REST API.
//!
//! The session lives in an HttpOnly cookie the browser sends by itself;
//! every request also carries the CSRF header the server requires for
//! cookie-authenticated writes.

use gloo_net::http::{Method, Request, RequestBuilder, Response};
use musicus_api::{ApiError, CSRF_HEADER};
use serde::{Serialize, de::DeserializeOwned};

#[derive(Debug, Clone, PartialEq)]
pub struct ApiErr {
    /// HTTP status, 0 for network errors.
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for ApiErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type ApiResult<T> = Result<T, ApiErr>;

fn network(e: impl std::fmt::Display) -> ApiErr {
    ApiErr { status: 0, message: format!("Network error: {e}") }
}

fn builder(method: Method, path: &str) -> RequestBuilder {
    RequestBuilder::new(path).method(method).header(CSRF_HEADER, "1")
}

async fn check(response: Response) -> ApiResult<Response> {
    if response.ok() {
        return Ok(response);
    }
    let status = response.status();
    let message = match response.json::<ApiError>().await {
        Ok(e) => capitalise(&e.message),
        Err(_) => format!("Request failed (HTTP {status})"),
    };
    if status == 401 {
        session_lost();
    }
    Err(ApiErr { status, message })
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// The session expired or was revoked: back to the login page (a full
/// reload clears all in-memory state).
fn session_lost() {
    if let Some(window) = web_sys::window() {
        let path = window.location().pathname().unwrap_or_default();
        if path != "/login" && !path.starts_with("/invite/") {
            let _ = window.location().set_href("/login");
        }
    }
}

pub async fn get<T: DeserializeOwned>(path: &str) -> ApiResult<T> {
    let r = check(builder(Method::GET, path).send().await.map_err(network)?).await?;
    r.json().await.map_err(network)
}

pub async fn send<B: Serialize, T: DeserializeOwned>(method: Method, path: &str, body: &B) -> ApiResult<T> {
    let request: Request = builder(method, path).json(body).map_err(network)?;
    let r = check(request.send().await.map_err(network)?).await?;
    r.json().await.map_err(network)
}

pub async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> ApiResult<T> {
    send(Method::POST, path, body).await
}

pub async fn put<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> ApiResult<T> {
    send(Method::PUT, path, body).await
}

pub async fn patch<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> ApiResult<T> {
    send(Method::PATCH, path, body).await
}

/// A request without a JSON answer (204).
pub async fn call(method: Method, path: &str) -> ApiResult<()> {
    check(builder(method, path).send().await.map_err(network)?).await.map(|_| ())
}

/// A JSON request without a JSON answer (204).
pub async fn call_json<B: Serialize>(method: Method, path: &str, body: &B) -> ApiResult<()> {
    let request: Request = builder(method, path).json(body).map_err(network)?;
    check(request.send().await.map_err(network)?).await.map(|_| ())
}

/// Uploads raw bytes (a sheet file).
pub async fn upload<T: DeserializeOwned>(path: &str, bytes: &[u8], content_type: &str) -> ApiResult<T> {
    let body = js_sys::Uint8Array::from(bytes);
    let request = builder(Method::PUT, path).header("content-type", content_type).body(body).map_err(network)?;
    let r = check(request.send().await.map_err(network)?).await?;
    r.json().await.map_err(network)
}

/// Outcome of loading an annotation page.
pub enum PageBytes {
    Found { bytes: Vec<u8>, revision: i64 },
    Missing,
}

pub async fn get_page(set_id: i64, page: i64) -> ApiResult<PageBytes> {
    let r =
        builder(Method::GET, &format!("/api/annotation-sets/{set_id}/pages/{page}")).send().await.map_err(network)?;
    if r.status() == 204 {
        return Ok(PageBytes::Missing);
    }
    let r = check(r).await?;
    let revision = r.headers().get("etag").and_then(|e| e.trim_matches('"').parse().ok()).unwrap_or(0);
    Ok(PageBytes::Found { bytes: r.binary().await.map_err(network)?, revision })
}

/// Outcome of saving an annotation page: the new revision, or a conflict
/// with a save from another device.
pub enum SaveOutcome {
    Saved(i64),
    Conflict,
}

pub async fn put_page(set_id: i64, page: i64, expected_revision: i64, bytes: &[u8]) -> ApiResult<SaveOutcome> {
    let body = js_sys::Uint8Array::from(bytes);
    let request = builder(Method::PUT, &format!("/api/annotation-sets/{set_id}/pages/{page}"))
        .header("content-type", "application/octet-stream")
        .header("if-match", &format!("\"{expected_revision}\""))
        .body(body)
        .map_err(network)?;
    let r = request.send().await.map_err(network)?;
    if r.status() == 409 {
        return Ok(SaveOutcome::Conflict);
    }
    let saved: musicus_api::SavedPage = check(r).await?.json().await.map_err(network)?;
    Ok(SaveOutcome::Saved(saved.revision))
}

pub fn file_url(sha256: &str) -> String {
    format!("/api/files/{sha256}")
}

pub fn thumbnail_url(sha256: &str) -> String {
    format!("/api/files/{sha256}/thumbnail")
}
