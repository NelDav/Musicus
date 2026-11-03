//! Passwords, session tokens, the authenticated-user extractor and the
//! failed-login limiter.

use crate::{
    db::now,
    error::{ApiResult, AppError},
    state::{App, AppState},
};
use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::phc::PasswordHash};
use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, HeaderValue, Method, header, request::Parts},
};
use musicus_api::{CSRF_HEADER, Id, Role, SESSION_COOKIE};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Mutex, OnceLock},
};

pub const MIN_PASSWORD_CHARS: usize = 8;
pub const MAX_PASSWORD_CHARS: usize = 256;

/// Rejects passwords that are too short or absurdly long (hashing cost).
pub fn check_password_policy(password: &str) -> ApiResult<()> {
    let n = password.chars().count();
    if n < MIN_PASSWORD_CHARS {
        return Err(AppError::BadRequest(format!("the password needs at least {MIN_PASSWORD_CHARS} characters")));
    }
    if n > MAX_PASSWORD_CHARS {
        return Err(AppError::BadRequest(format!("the password may have at most {MAX_PASSWORD_CHARS} characters")));
    }
    Ok(())
}

/// Hashes with Argon2id (m = 19 MiB, t = 2, p = 1), bounded by the
/// application's hashing semaphore.
pub async fn hash_password(app: &App, password: &str) -> ApiResult<String> {
    let _permit = app.hashing.acquire().await.map_err(AppError::internal)?;
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        Argon2::default().hash_password(password.as_bytes()).map(|h| h.to_string()).map_err(AppError::internal)
    })
    .await
    .map_err(AppError::internal)?
}

/// Verifies a password. With `hash = None` (unknown user) it verifies
/// against a dummy hash, so response times do not reveal which usernames exist.
pub async fn verify_password(app: &App, hash: Option<&str>, password: &str) -> ApiResult<bool> {
    let _permit = app.hashing.acquire().await.map_err(AppError::internal)?;
    let known = hash.is_some();
    let hash = hash.map(str::to_owned).unwrap_or_else(|| dummy_hash().to_owned());
    let password = password.to_owned();
    let ok = tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
            .unwrap_or(false)
    })
    .await
    .map_err(AppError::internal)?;
    Ok(ok && known)
}

fn dummy_hash() -> &'static str {
    static DUMMY: OnceLock<String> = OnceLock::new();
    DUMMY.get_or_init(|| {
        Argon2::default().hash_password(b"musicus-dummy-password").expect("hashing a constant works").to_string()
    })
}

/// A fresh random token (hex, 256 bits) and the hash stored for it.
pub fn new_token() -> (String, Vec<u8>) {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).expect("the OS provides randomness");
    let token = hex::encode(raw);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Creates a session and returns its token.
pub async fn create_session(app: &App, user_id: Id, user_agent: Option<&str>) -> ApiResult<String> {
    let (token, hash) = new_token();
    let now = now();
    let user_agent = user_agent.map(|ua| ua.chars().take(200).collect::<String>());
    sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at, user_agent)
         VALUES (?1, ?2, ?3, ?3, ?4, ?5)",
    )
    .bind(hash)
    .bind(user_id)
    .bind(now)
    .bind(now + app.settings.session_ttl_secs)
    .bind(user_agent)
    .execute(&app.db.write)
    .await?;
    Ok(token)
}

/// `Set-Cookie` value carrying a session token (or clearing it).
pub fn session_cookie(app: &App, token: Option<&str>) -> HeaderValue {
    let secure = if app.settings.cookie_secure { "; Secure" } else { "" };
    let value = match token {
        Some(t) => format!(
            "{SESSION_COOKIE}={t}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
            app.settings.session_ttl_secs
        ),
        None => format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}"),
    };
    HeaderValue::from_str(&value).expect("token is hex")
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(SESSION_COOKIE)?.strip_prefix('=').map(str::to_owned))
        .filter(|t| !t.is_empty())
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(|t| t.trim().to_owned())
}

/// The user making the request. Extracting it rejects unauthenticated
/// requests with 401, and cookie-authenticated state-changing requests
/// without the CSRF header with 403.
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub id: Id,
    pub role: Role,
    pub session_id: Id,
}

impl AuthUser {
    pub fn is_librarian(&self) -> bool {
        self.role.can_manage_library()
    }

    pub fn require_librarian(&self) -> ApiResult<()> {
        if self.is_librarian() { Ok(()) } else { Err(AppError::Forbidden("only librarians may do this")) }
    }

    pub fn require_admin(&self) -> ApiResult<()> {
        if self.role.is_admin() { Ok(()) } else { Err(AppError::Forbidden("only admins may do this")) }
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Self, Self::Rejection> {
        let (token, via_cookie) = match bearer_token(&parts.headers) {
            Some(t) => (t, false),
            None => (cookie_token(&parts.headers).ok_or(AppError::Unauthorized)?, true),
        };
        let safe = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS);
        if via_cookie && !safe && !parts.headers.contains_key(CSRF_HEADER) {
            return Err(AppError::Forbidden("missing CSRF header"));
        }
        let now = now();
        let row: Option<(Id, Id, i64, String)> = sqlx::query_as(
            "SELECT s.id, s.user_id, s.last_seen_at, u.role FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled = 0",
        )
        .bind(token_hash(&token))
        .bind(now)
        .fetch_optional(&app.db.read)
        .await?;
        let (session_id, user_id, last_seen, role) = row.ok_or(AppError::Unauthorized)?;
        // Slide the expiry at most once an hour, so reading sheets does not
        // turn into a database write per request.
        if now - last_seen > 3600 {
            sqlx::query("UPDATE sessions SET last_seen_at = ?2, expires_at = ?3 WHERE id = ?1")
                .bind(session_id)
                .bind(now)
                .bind(now + app.settings.session_ttl_secs)
                .execute(&app.db.write)
                .await?;
        }
        let role = Role::parse(&role).ok_or_else(|| AppError::internal("invalid role in database"))?;
        Ok(AuthUser { id: user_id, role, session_id })
    }
}

/// The client address, for rate limiting.
pub struct ClientIp(pub String);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &AppState) -> Result<Self, Self::Rejection> {
        if app.settings.trust_proxy
            && let Some(ip) =
                parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()).and_then(|v| v.split(',').next())
        {
            return Ok(ClientIp(ip.trim().to_owned()));
        }
        let ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0.ip().to_string())
            .unwrap_or_else(|| "unknown".into());
        Ok(ClientIp(ip))
    }
}

/// Counts failed logins per account and per client address in memory.
#[derive(Default)]
pub struct LoginLimiter {
    failures: Mutex<HashMap<String, Vec<i64>>>,
}

const LIMIT_WINDOW_SECS: i64 = 15 * 60;
const LIMIT_MAX_FAILURES: usize = 10;

impl LoginLimiter {
    /// Fails with 429 if any key had too many recent failures.
    pub fn check(&self, keys: &[String]) -> ApiResult<()> {
        let now = now();
        let map = self.failures.lock().expect("limiter lock");
        for key in keys {
            if let Some(times) = map.get(key) {
                let recent: Vec<&i64> = times.iter().filter(|t| now - **t < LIMIT_WINDOW_SECS).collect();
                if recent.len() >= LIMIT_MAX_FAILURES {
                    let wait = (LIMIT_WINDOW_SECS - (now - *recent[0])).max(60);
                    return Err(AppError::TooManyRequests(((wait + 59) / 60) as u64));
                }
            }
        }
        Ok(())
    }

    pub fn record_failure(&self, keys: &[String]) {
        let now = now();
        let mut map = self.failures.lock().expect("limiter lock");
        for key in keys {
            let times = map.entry(key.clone()).or_default();
            times.retain(|t| now - *t < LIMIT_WINDOW_SECS);
            times.push(now);
        }
        if map.len() > 10_000 {
            map.retain(|_, times| times.iter().any(|t| now - *t < LIMIT_WINDOW_SECS));
        }
    }

    pub fn clear(&self, key: &str) {
        self.failures.lock().expect("limiter lock").remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_session_cookie_among_others() {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("a=1; musicus_session=abc; b=2"));
        assert_eq!(cookie_token(&h).as_deref(), Some("abc"));
        h.insert(header::COOKIE, HeaderValue::from_static("musicus_session_x=1"));
        assert_eq!(cookie_token(&h), None);
    }

    #[test]
    fn limiter_blocks_after_ten_failures() {
        let l = LoginLimiter::default();
        let keys = vec!["user:anna".to_owned()];
        for _ in 0..LIMIT_MAX_FAILURES {
            assert!(l.check(&keys).is_ok());
            l.record_failure(&keys);
        }
        assert!(matches!(l.check(&keys), Err(AppError::TooManyRequests(_))));
        l.clear("user:anna");
        assert!(l.check(&keys).is_ok());
    }
}
