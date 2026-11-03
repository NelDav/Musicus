//! One error type for all handlers, rendered as [`musicus_api::ApiError`] JSON.

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use musicus_api::ApiError;

pub type ApiResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("login required")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    /// A JSON body other than [`ApiError`], e.g. [`musicus_api::PageConflict`].
    #[error("conflict")]
    ConflictWith(serde_json::Value),
    #[error("{0}")]
    Unprocessable(String),
    #[error("request body larger than {0} bytes")]
    PayloadTooLarge(u64),
    #[error("{0}")]
    UnsupportedMediaType(&'static str),
    #[error("{0}")]
    PreconditionRequired(&'static str),
    #[error("too many attempts, try again in {0} minutes")]
    TooManyRequests(u64),
    #[error("internal error")]
    Internal(String),
}

impl AppError {
    pub fn internal(e: impl std::fmt::Display) -> Self {
        AppError::Internal(e.to_string())
    }

    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            AppError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Conflict(_) | AppError::ConflictWith(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::Unprocessable(_) => (StatusCode::UNPROCESSABLE_ENTITY, "unprocessable"),
            AppError::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            AppError::UnsupportedMediaType(_) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_media_type"),
            AppError::PreconditionRequired(_) => (StatusCode::PRECONDITION_REQUIRED, "precondition_required"),
            AppError::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        if let AppError::Internal(detail) = &self {
            tracing::error!(%detail, "internal error");
        }
        let mut response = match self {
            AppError::ConflictWith(body) => (status, Json(body)).into_response(),
            other => {
                let body = ApiError { error: code.into(), message: other.to_string() };
                (status, Json(body)).into_response()
            }
        };
        if status == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::RowNotFound => AppError::NotFound,
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                AppError::Conflict("an entry with this name already exists".into())
            }
            sqlx::Error::Database(db) if db.is_foreign_key_violation() => {
                AppError::Conflict("the entry is still in use or refers to something that does not exist".into())
            }
            _ => AppError::internal(e),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::internal(e)
    }
}

/// Trims a required text field and rejects empty or overlong values.
pub fn required(field: &str, value: &str, max_chars: usize) -> ApiResult<String> {
    let v = value.trim();
    if v.is_empty() {
        return Err(AppError::BadRequest(format!("{field} must not be empty")));
    }
    if v.chars().count() > max_chars {
        return Err(AppError::BadRequest(format!("{field} is longer than {max_chars} characters")));
    }
    Ok(v.to_owned())
}

/// Trims an optional text field; empty becomes `None`.
pub fn optional(field: &str, value: Option<&str>, max_chars: usize) -> ApiResult<Option<String>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) if v.chars().count() > max_chars => {
            Err(AppError::BadRequest(format!("{field} is longer than {max_chars} characters")))
        }
        Some(v) => Ok(Some(v.to_owned())),
    }
}
