//! JSON request and response bodies of the REST API.
//!
//! Timestamps are Unix seconds (UTC). Dates without a time (concert dates)
//! are ISO strings `YYYY-MM-DD`.

use serde::{Deserialize, Deserializer, Serialize};

pub type Id = i64;

/// Header that every state-changing request authenticated by the session
/// cookie must carry. Browsers do not send custom headers cross-site without
/// a CORS preflight, so this blocks cross-site request forgery.
pub const CSRF_HEADER: &str = "x-musicus-csrf";

/// Name of the session cookie set by the web login.
pub const SESSION_COOKIE: &str = "musicus_session";

/// Global role of a user. Access to sheets is granted to groups, not roles;
/// roles only decide who may manage what.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Reads the parts granted to their groups and annotates them.
    Member,
    /// Uploads files, edits metadata, programmes and grants.
    Librarian,
    /// Manages users and groups, plus everything a librarian may do.
    Admin,
}

impl Role {
    pub fn can_manage_library(self) -> bool {
        matches!(self, Role::Librarian | Role::Admin)
    }

    pub fn is_admin(self) -> bool {
        self == Role::Admin
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Member => "member",
            Role::Librarian => "librarian",
            Role::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "member" => Some(Role::Member),
            "librarian" => Some(Role::Librarian),
            "admin" => Some(Role::Admin),
            _ => None,
        }
    }
}

/// Error body of every non-2xx response.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApiError {
    /// Stable machine-readable code, e.g. `not_found`, `conflict`.
    pub error: String,
    /// Human-readable explanation.
    pub message: String,
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// `true` for native apps: the token is returned in the body and no
    /// cookie is set. Web clients leave this `false`.
    #[serde(default)]
    pub bearer: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LoginResponse {
    pub user: CurrentUser,
    /// Only present when the login asked for a bearer token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CurrentUser {
    pub id: Id,
    pub username: String,
    pub display_name: String,
    pub role: Role,
}

/// What the invite page shows before the user chooses a password.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InviteInfo {
    pub username: String,
    pub display_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AcceptInvite {
    pub password: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChangePassword {
    pub current_password: String,
    pub new_password: String,
}

/// One logged-in device of the current user.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub id: Id,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub expires_at: i64,
    pub user_agent: Option<String>,
    /// The session this request was made with.
    pub current: bool,
}

// ---------------------------------------------------------------------------
// Users (admin)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UserSummary {
    pub id: Id,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub disabled: bool,
    /// `false` until the user has accepted their invite.
    pub has_password: bool,
    pub group_ids: Vec<Id>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CreateUser {
    pub username: String,
    pub display_name: String,
    pub role: Role,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct UpdateUser {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
}

/// A one-time link token. The client builds the URL `/invite/<token>`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Invite {
    pub user_id: Id,
    pub token: String,
    pub expires_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CreatedUser {
    pub user: UserSummary,
    pub invite: Invite,
}

// ---------------------------------------------------------------------------
// Instruments and groups
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Instrument {
    pub id: Id,
    pub name: String,
    pub family: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InstrumentInput {
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Group {
    pub id: Id,
    pub name: String,
    pub member_ids: Vec<Id>,
    /// Members may read every part tagged with one of these instruments.
    pub instrument_ids: Vec<Id>,
    /// Members may read every part of these scores.
    pub score_ids: Vec<Id>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GroupInput {
    pub name: String,
}

/// Replaces a whole list (members, grants, programme items).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct IdList {
    pub ids: Vec<Id>,
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ScoreInput {
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub composer: Option<String>,
    #[serde(default)]
    pub arranger: Option<String>,
    /// e.g. march, polka, concert, sacred, pop.
    #[serde(default)]
    pub genre: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// A score with the parts the requesting user may read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Score {
    pub id: Id,
    pub title: String,
    pub subtitle: Option<String>,
    pub composer: Option<String>,
    pub arranger: Option<String>,
    pub genre: Option<String>,
    pub tags: Vec<String>,
    pub notes: Option<String>,
    pub parts: Vec<Part>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PartInput {
    /// e.g. "2nd Trumpet in Bb".
    pub label: String,
    pub instrument_ids: Vec<Id>,
    /// Voice / chair number: 2 for "2nd Trumpet".
    #[serde(default)]
    pub voice: Option<i64>,
    /// Key of a transposing instrument, e.g. "Bb", "Eb", "F".
    #[serde(default)]
    pub transposition: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Part {
    pub id: Id,
    pub score_id: Id,
    pub label: String,
    pub instrument_ids: Vec<Id>,
    pub voice: Option<i64>,
    pub transposition: Option<String>,
    pub notes: Option<String>,
    /// The current file version, `None` until a file is uploaded.
    pub file: Option<FileVersion>,
}

/// One uploaded file version of a part. Files are immutable and addressed
/// by their SHA-256; `GET /api/files/{sha256}` serves the bytes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileVersion {
    pub version: i64,
    pub sha256: String,
    pub mime: String,
    pub size_bytes: i64,
    pub page_count: Option<i64>,
    pub has_thumbnail: bool,
    pub uploaded_at: i64,
}

// ---------------------------------------------------------------------------
// Concert programmes
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ProgrammeInput {
    pub title: String,
    /// `YYYY-MM-DD`.
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProgrammeSummary {
    pub id: Id,
    pub title: String,
    pub date: Option<String>,
    pub notes: Option<String>,
    /// Number of pieces the requesting user has at least one part for.
    pub item_count: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Programme {
    pub id: Id,
    pub title: String,
    pub date: Option<String>,
    pub notes: Option<String>,
    /// Pieces in concert order; only those the user may read.
    pub items: Vec<ProgrammeItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProgrammeItem {
    pub position: i64,
    pub score: Score,
}

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

/// A named, private set of annotations of one user on one file version of a
/// part, e.g. "Christmas concert 2026".
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AnnotationSet {
    pub id: Id,
    pub part_id: Id,
    /// The file version the annotations were drawn on.
    pub file_sha256: String,
    pub file_version: i64,
    pub name: String,
    pub programme_id: Option<Id>,
    pub is_default: bool,
    pub created_at: i64,
    pub updated_at: i64,
    /// Pages that have a saved layer, with their current revision.
    pub pages: Vec<PageRevision>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRevision {
    /// Zero-based page index.
    pub page: i64,
    pub revision: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NewAnnotationSet {
    pub name: String,
    #[serde(default)]
    pub programme_id: Option<Id>,
    /// Duplicate the pages of this set (same user and part) into the new one.
    #[serde(default)]
    pub copy_from: Option<Id>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct UpdateAnnotationSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Some(None)` (JSON `null`) unlinks the programme; absent leaves it.
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub programme_id: Option<Option<Id>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

/// Response of a successful page save (`PUT .../pages/{page}`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedPage {
    pub revision: i64,
}

/// Body of a 409 response to a page save: someone (another device of the
/// same user) saved in between. The client merges and retries.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageConflict {
    pub current_revision: i64,
}

fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programme_link_distinguishes_absent_and_null() {
        let absent: UpdateAnnotationSet = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.programme_id, None);
        let null: UpdateAnnotationSet = serde_json::from_str(r#"{"programme_id":null}"#).unwrap();
        assert_eq!(null.programme_id, Some(None));
        let set: UpdateAnnotationSet = serde_json::from_str(r#"{"programme_id":7}"#).unwrap();
        assert_eq!(set.programme_id, Some(Some(7)));
        assert_eq!(serde_json::to_string(&null).unwrap(), r#"{"programme_id":null}"#);
    }

    #[test]
    fn roles_round_trip() {
        for role in [Role::Member, Role::Librarian, Role::Admin] {
            assert_eq!(Role::parse(role.as_str()), Some(role));
            let json = serde_json::to_string(&role).unwrap();
            assert_eq!(json, format!("\"{}\"", role.as_str()));
        }
        assert!(Role::Admin.can_manage_library() && !Role::Member.can_manage_library());
    }
}
