//! Who may read which part.
//!
//! Access is granted to groups only and is additive: a member may read a
//! part if one of their groups is granted one of the part's instruments, or
//! the part's whole score. Librarians and admins may read everything.
//! Denied reads answer 404, so the library does not reveal what exists.

use crate::{auth::AuthUser, error::ApiResult, state::App};
use musicus_api::Id;

/// SQL condition "the user may read part `p`" (needs columns `p.id` and
/// `p.score_id`). Binds: `?1` = user id, `?2` = 1 for librarians, else 0.
#[macro_export]
macro_rules! part_visible {
    () => {
        "(?2 = 1
          OR EXISTS (SELECT 1 FROM part_instruments acl_pi
                     JOIN group_instrument_grants acl_g ON acl_g.instrument_id = acl_pi.instrument_id
                     JOIN group_members acl_m ON acl_m.group_id = acl_g.group_id
                     WHERE acl_pi.part_id = p.id AND acl_m.user_id = ?1)
          OR EXISTS (SELECT 1 FROM group_score_grants acl_s
                     JOIN group_members acl_m2 ON acl_m2.group_id = acl_s.group_id
                     WHERE acl_s.score_id = p.score_id AND acl_m2.user_id = ?1))"
    };
}

pub fn librarian_flag(user: &AuthUser) -> i64 {
    user.is_librarian() as i64
}

/// The per-request check before serving a part, its files or annotations.
pub async fn can_read_part(app: &App, user: &AuthUser, part_id: Id) -> ApiResult<bool> {
    let found: Option<i64> = sqlx::query_scalar(concat!("SELECT 1 FROM parts p WHERE p.id = ?3 AND ", part_visible!()))
        .bind(user.id)
        .bind(librarian_flag(user))
        .bind(part_id)
        .fetch_optional(&app.db.read)
        .await?;
    Ok(found.is_some())
}

/// Like [`can_read_part`], but fails with 404.
pub async fn ensure_part_readable(app: &App, user: &AuthUser, part_id: Id) -> ApiResult<()> {
    if can_read_part(app, user, part_id).await? { Ok(()) } else { Err(crate::error::AppError::NotFound) }
}
