//! The HTTP API under `/api`, plus the static web client.

pub mod annotations;
pub mod auth;
pub mod files;
pub mod groups;
pub mod library;
pub mod programmes;
pub mod users;

use crate::{error::AppError, state::AppState};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderName, HeaderValue, header},
    routing::{get, post, put},
};
use std::path::Path;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

pub fn api() -> Router<AppState> {
    Router::new()
        // Authentication and the user's own account
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/password", post(auth::change_password))
        .route("/auth/sessions", get(auth::list_sessions))
        .route("/auth/sessions/{id}", axum::routing::delete(auth::revoke_session))
        .route("/auth/invites/{token}", get(auth::invite_info).post(auth::accept_invite))
        // Users (admin)
        .route("/users", get(users::list).post(users::create))
        .route("/users/{id}", axum::routing::patch(users::update))
        .route("/users/{id}/invite", post(users::reinvite))
        // Instruments and groups
        .route("/instruments", get(groups::list_instruments).post(groups::create_instrument))
        .route("/instruments/{id}", put(groups::update_instrument).delete(groups::delete_instrument))
        .route("/groups", get(groups::list_groups).post(groups::create_group))
        .route("/groups/{id}", put(groups::rename_group).delete(groups::delete_group))
        .route("/groups/{id}/members", put(groups::set_members))
        .route("/groups/{id}/instrument-grants", put(groups::set_instrument_grants))
        .route("/groups/{id}/score-grants", put(groups::set_score_grants))
        // Library
        .route("/scores", get(library::list_scores).post(library::create_score))
        .route("/scores/{id}", get(library::get_score).put(library::update_score).delete(library::delete_score))
        .route("/scores/{id}/parts", post(library::create_part))
        .route("/parts/{id}", get(library::get_part).put(library::update_part).delete(library::delete_part))
        .route("/parts/{id}/file", put(library::upload_file).layer(DefaultBodyLimit::disable()))
        .route("/parts/{id}/versions", get(library::list_versions))
        .route("/files/{sha256}", get(files::get_file))
        .route("/files/{sha256}/thumbnail", get(files::get_thumbnail))
        // Programmes
        .route("/programmes", get(programmes::list).post(programmes::create))
        .route("/programmes/{id}", get(programmes::get).put(programmes::update).delete(programmes::delete))
        .route("/programmes/{id}/items", put(programmes::set_items))
        // Annotations
        .route("/parts/{id}/annotation-sets", get(annotations::list).post(annotations::create))
        .route("/annotation-sets/{id}", axum::routing::patch(annotations::update).delete(annotations::delete))
        .route("/annotation-sets/{id}/migrate", post(annotations::migrate))
        .route(
            "/annotation-sets/{id}/pages/{page}",
            get(annotations::get_page)
                .put(annotations::put_page)
                .layer(DefaultBodyLimit::max(musicus_api::ink::MAX_ENCODED_BYTES)),
        )
        .fallback(|| async { AppError::NotFound })
}

/// The whole application: API, optional web client, common headers.
pub fn app(state: AppState, web_dir: Option<&Path>) -> Router {
    let mut router = Router::new().nest("/api", api());
    if let Some(dir) = web_dir {
        // Unknown paths are client-side routes of the single-page app.
        let spa = ServeDir::new(dir).fallback(ServeFile::new(dir.join("index.html")));
        router = router.fallback_service(spa);
    }
    router
        .layer(SetResponseHeaderLayer::overriding(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::overriding(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY")))
        .layer(SetResponseHeaderLayer::overriding(header::REFERRER_POLICY, HeaderValue::from_static("same-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
