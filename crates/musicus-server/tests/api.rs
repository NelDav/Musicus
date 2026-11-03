//! End-to-end tests of the HTTP API against a temporary data directory.

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use musicus_api::{
    AnnotationSet, CreatedUser, Group, Id, Instrument, LoginResponse, PageConflict, Part, Programme, ProgrammeSummary,
    Score,
    ink::{self, InkObject, PageLayer, Stroke, Tool},
};
use musicus_server::{bootstrap_admin, open_state, routes, state::Settings};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Server {
    router: Router,
    _dir: tempfile::TempDir,
    admin_invite: String,
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&self.body)))
    }
}

enum Auth<'a> {
    None,
    Bearer(&'a str),
    /// Cookie authentication; `true` adds the CSRF header.
    Cookie(&'a str, bool),
}

impl Server {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings { cookie_secure: false, ..Settings::default() };
        let state = open_state(dir.path(), 64 * 1024 * 1024, settings).await.unwrap();
        let admin_invite = bootstrap_admin(&state).await.unwrap().expect("fresh database");
        assert!(bootstrap_admin(&state).await.unwrap().is_none());
        Server { router: routes::app(state, None), _dir: dir, admin_invite }
    }

    async fn send(&self, method: Method, uri: &str, auth: Auth<'_>, headers: &[(&str, &str)], body: Body) -> Reply {
        let mut req = Request::builder().method(method).uri(uri);
        match auth {
            Auth::None => {}
            Auth::Bearer(t) => req = req.header(header::AUTHORIZATION, format!("Bearer {t}")),
            Auth::Cookie(t, csrf) => {
                req = req.header(header::COOKIE, format!("musicus_session={t}"));
                if csrf {
                    req = req.header(musicus_api::CSRF_HEADER, "1");
                }
            }
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let response = self.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.into_body().collect().await.unwrap().to_bytes().to_vec();
        Reply { status, headers, body }
    }

    async fn json(&self, method: Method, uri: &str, token: &str, body: impl Serialize) -> Reply {
        self.send(
            method,
            uri,
            Auth::Bearer(token),
            &[("content-type", "application/json")],
            Body::from(serde_json::to_vec(&body).unwrap()),
        )
        .await
    }

    async fn get(&self, uri: &str, token: &str) -> Reply {
        self.send(Method::GET, uri, Auth::Bearer(token), &[], Body::empty()).await
    }

    async fn ok<T: DeserializeOwned>(&self, method: Method, uri: &str, token: &str, body: impl Serialize) -> T {
        let r = self.json(method.clone(), uri, token, body).await;
        assert!(r.status.is_success(), "{method} {uri}: {} {}", r.status, String::from_utf8_lossy(&r.body));
        r.json()
    }

    async fn login(&self, username: &str, password: &str) -> String {
        let r = self
            .send(
                Method::POST,
                "/api/auth/login",
                Auth::None,
                &[("content-type", "application/json")],
                Body::from(json!({"username": username, "password": password, "bearer": true}).to_string()),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        r.json::<LoginResponse>().token.unwrap()
    }

    /// Accepts an invite and returns the session cookie value.
    async fn accept_invite(&self, token: &str, password: &str) -> String {
        let r = self
            .send(
                Method::POST,
                &format!("/api/auth/invites/{token}"),
                Auth::None,
                &[("content-type", "application/json")],
                Body::from(json!({ "password": password }).to_string()),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        let cookie = r.headers[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
        cookie.split(';').next().unwrap().trim_start_matches("musicus_session=").to_owned()
    }

    async fn admin(&self) -> String {
        self.accept_invite(&self.admin_invite, "admin-password-1").await;
        self.login("admin", "admin-password-1").await
    }

    async fn member(&self, admin: &str, username: &str) -> (Id, String) {
        let created: CreatedUser = self
            .ok(
                Method::POST,
                "/api/users",
                admin,
                json!({"username": username, "display_name": username, "role": "member"}),
            )
            .await;
        self.accept_invite(&created.invite.token, "member-password-1").await;
        (created.user.id, self.login(username, "member-password-1").await)
    }

    async fn upload(&self, token: &str, part: Id, bytes: Vec<u8>) -> Reply {
        self.send(Method::PUT, &format!("/api/parts/{part}/file"), Auth::Bearer(token), &[], Body::from(bytes)).await
    }
}

/// A minimal valid PDF with `pages` empty A4 pages; `tag` makes contents unique.
fn tiny_pdf(pages: usize, tag: &str) -> Vec<u8> {
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
            (0..pages).map(|i| format!("{} 0 R", i + 3)).collect::<Vec<_>>().join(" ")
        ),
    ];
    for _ in 0..pages {
        objects.push("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] >>".to_owned());
    }
    let mut pdf = format!("%PDF-1.7\n%{tag}\n").into_bytes();
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for off in offsets {
        pdf.extend(format!("{off:010} 00000 n \n").bytes());
    }
    pdf.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
    pdf
}

fn layer(ids: &[u64]) -> Vec<u8> {
    let objects = ids
        .iter()
        .map(|&id| {
            InkObject::Stroke(Stroke::from_points(
                id,
                Tool::Pen,
                0xff0000ff,
                200,
                &[(100, 100, 128), (5000, 5200, 140)],
            ))
        })
        .collect();
    ink::encode(&PageLayer { objects })
}

/// An orchestra with two instruments, a group "Trumpets" with member anna,
/// one score with a trumpet and a flute part, both with files.
struct Library {
    admin: String,
    anna: String,
    anna_id: Id,
    trumpets: Id,
    score: Id,
    trumpet_part: Part,
    flute_part: Part,
}

async fn library(s: &Server) -> Library {
    let admin = s.admin().await;
    let trumpet: Instrument =
        s.ok(Method::POST, "/api/instruments", &admin, json!({"name": "Trumpet", "family": "Brass"})).await;
    let flute: Instrument = s.ok(Method::POST, "/api/instruments", &admin, json!({"name": "Flute"})).await;
    let group: Group = s.ok(Method::POST, "/api/groups", &admin, json!({"name": "Trumpets"})).await;
    let (anna_id, anna) = s.member(&admin, "anna").await;
    s.ok::<Group>(Method::PUT, &format!("/api/groups/{}/members", group.id), &admin, json!({"ids": [anna_id]})).await;
    let g: Group = s
        .ok(Method::PUT, &format!("/api/groups/{}/instrument-grants", group.id), &admin, json!({"ids": [trumpet.id]}))
        .await;
    assert_eq!(g.member_ids, vec![anna_id]);

    let score: Score = s
        .ok(
            Method::POST,
            "/api/scores",
            &admin,
            json!({"title": "Florentiner Marsch", "composer": "Julius Fučík", "genre": "March", "tags": ["Encore"]}),
        )
        .await;
    let tp: Part = s
        .ok(
            Method::POST,
            &format!("/api/scores/{}/parts", score.id),
            &admin,
            json!({"label": "2nd Trumpet in Bb", "instrument_ids": [trumpet.id], "voice": 2, "transposition": "Bb"}),
        )
        .await;
    let fp: Part = s
        .ok(
            Method::POST,
            &format!("/api/scores/{}/parts", score.id),
            &admin,
            json!({"label": "Flute", "instrument_ids": [flute.id]}),
        )
        .await;
    let tp: Part = s.upload(&admin, tp.id, tiny_pdf(4, "trumpet")).await.json();
    let fp: Part = s.upload(&admin, fp.id, tiny_pdf(2, "flute")).await.json();
    assert_eq!(tp.file.as_ref().unwrap().page_count, Some(4));
    Library { admin, anna, anna_id, trumpets: group.id, score: score.id, trumpet_part: tp, flute_part: fp }
}

#[tokio::test]
async fn login_sessions_and_csrf() {
    let s = Server::start().await;
    let cookie = s.accept_invite(&s.admin_invite, "admin-password-1").await;
    // The invite is single-use.
    let again =
        s.send(Method::GET, &format!("/api/auth/invites/{}", s.admin_invite), Auth::None, &[], Body::empty()).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);

    let me = s.send(Method::GET, "/api/auth/me", Auth::Cookie(&cookie, false), &[], Body::empty()).await;
    assert_eq!(me.json::<Value>()["role"], "admin");
    assert_eq!(
        s.send(Method::GET, "/api/auth/me", Auth::None, &[], Body::empty()).await.status,
        StatusCode::UNAUTHORIZED
    );

    // Cookie-authenticated writes need the CSRF header; bearer tokens do not.
    let body = || Body::from(json!({"name": "Horn"}).to_string());
    let ct = [("content-type", "application/json")];
    assert_eq!(
        s.send(Method::POST, "/api/instruments", Auth::Cookie(&cookie, false), &ct, body()).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        s.send(Method::POST, "/api/instruments", Auth::Cookie(&cookie, true), &ct, body()).await.status,
        StatusCode::OK
    );

    let wrong = || Body::from(json!({"username": "ADMIN", "password": "nope-nope", "bearer": true}).to_string());
    for _ in 0..10 {
        assert_eq!(
            s.send(Method::POST, "/api/auth/login", Auth::None, &ct, wrong()).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        s.send(Method::POST, "/api/auth/login", Auth::None, &ct, wrong()).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );

    // Logout ends exactly this session.
    let r = s.send(Method::POST, "/api/auth/logout", Auth::Cookie(&cookie, true), &[], Body::empty()).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        s.send(Method::GET, "/api/auth/me", Auth::Cookie(&cookie, false), &[], Body::empty()).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn members_only_reach_granted_parts() {
    let s = Server::start().await;
    let lib = library(&s).await;

    // Anna sees the score, but only the trumpet part.
    let scores: Vec<Score> = s.get("/api/scores", &lib.anna).await.json();
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].parts.iter().map(|p| p.id).collect::<Vec<_>>(), vec![lib.trumpet_part.id]);

    let trumpet_sha = &lib.trumpet_part.file.as_ref().unwrap().sha256;
    let flute_sha = &lib.flute_part.file.as_ref().unwrap().sha256;
    let file = s.get(&format!("/api/files/{trumpet_sha}"), &lib.anna).await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.headers[header::CONTENT_TYPE], "application/pdf");
    assert_eq!(file.headers[header::CACHE_CONTROL], "private, no-store");
    assert_eq!(file.body, tiny_pdf(4, "trumpet"));
    // Denied files look like missing ones.
    assert_eq!(s.get(&format!("/api/files/{flute_sha}"), &lib.anna).await.status, StatusCode::NOT_FOUND);
    assert_eq!(s.get(&format!("/api/files/{flute_sha}/thumbnail"), &lib.anna).await.status, StatusCode::NOT_FOUND);
    assert_eq!(
        s.get(&format!("/api/files/{trumpet_sha}/thumbnail"), &lib.anna).await.headers[header::CONTENT_TYPE],
        "image/png"
    );
    assert_eq!(
        s.get(&format!("/api/parts/{}/versions", lib.flute_part.id), &lib.anna).await.status,
        StatusCode::NOT_FOUND
    );

    // Range and conditional requests.
    let uri = format!("/api/files/{trumpet_sha}");
    let range = s.send(Method::GET, &uri, Auth::Bearer(&lib.anna), &[("range", "bytes=0-7")], Body::empty()).await;
    assert_eq!(range.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(range.body, b"%PDF-1.7");
    let etag = format!("\"{trumpet_sha}\"");
    let cond = s.send(Method::GET, &uri, Auth::Bearer(&lib.anna), &[("if-none-match", &etag)], Body::empty()).await;
    assert_eq!(cond.status, StatusCode::NOT_MODIFIED);

    // A score grant opens every part of that score.
    s.ok::<Group>(
        Method::PUT,
        &format!("/api/groups/{}/score-grants", lib.trumpets),
        &lib.admin,
        json!({"ids": [lib.score]}),
    )
    .await;
    assert_eq!(s.get(&format!("/api/files/{flute_sha}"), &lib.anna).await.status, StatusCode::OK);

    // Removing her from the group revokes everything on the next request.
    s.ok::<Group>(Method::PUT, &format!("/api/groups/{}/members", lib.trumpets), &lib.admin, json!({"ids": []})).await;
    assert_eq!(s.get(&format!("/api/files/{trumpet_sha}"), &lib.anna).await.status, StatusCode::NOT_FOUND);
    assert!(s.get("/api/scores", &lib.anna).await.json::<Vec<Score>>().is_empty());

    // Members cannot manage the library.
    let r = s.json(Method::POST, "/api/scores", &lib.anna, json!({"title": "x"})).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(s.get("/api/users", &lib.anna).await.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn uploads_are_versioned_and_validated() {
    let s = Server::start().await;
    let lib = library(&s).await;
    let part = lib.trumpet_part.id;

    assert_eq!(
        s.upload(&lib.admin, part, b"<?xml version='1.0'?>".to_vec()).await.status,
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    // Same content again: no new version.
    let same: Part = s.upload(&lib.admin, part, tiny_pdf(4, "trumpet")).await.json();
    assert_eq!(same.file.unwrap().version, 1);
    let v2: Part = s.upload(&lib.admin, part, tiny_pdf(4, "trumpet corrected")).await.json();
    assert_eq!(v2.file.as_ref().unwrap().version, 2);
    let versions: Vec<musicus_api::FileVersion> = s.get(&format!("/api/parts/{part}/versions"), &lib.anna).await.json();
    assert_eq!(versions.iter().map(|v| v.version).collect::<Vec<_>>(), vec![2, 1]);
    // Old versions stay readable for annotations drawn on them.
    let old = &lib.trumpet_part.file.unwrap().sha256;
    assert_eq!(s.get(&format!("/api/files/{old}"), &lib.anna).await.status, StatusCode::OK);
}

#[tokio::test]
async fn annotations_are_private_versioned_and_conflict_checked() {
    let s = Server::start().await;
    let lib = library(&s).await;
    let part = lib.trumpet_part.id;
    let set: AnnotationSet = s
        .ok(Method::POST, &format!("/api/parts/{part}/annotation-sets"), &lib.anna, json!({"name": "Christmas 2026"}))
        .await;
    assert!(set.is_default);
    assert_eq!(set.file_version, 1);
    let page = |n: i64| format!("/api/annotation-sets/{}/pages/{n}", set.id);
    let put = |uri: String, rev: &'static str, body: Vec<u8>| {
        let token = lib.anna.clone();
        let s = &s;
        async move { s.send(Method::PUT, &uri, Auth::Bearer(&token), &[("if-match", rev)], Body::from(body)).await }
    };

    let empty = s.get(&page(0), &lib.anna).await;
    assert_eq!((empty.status, empty.headers[header::ETAG].to_str().unwrap()), (StatusCode::NO_CONTENT, "\"0\""));
    assert_eq!(
        s.send(Method::PUT, &page(0), Auth::Bearer(&lib.anna), &[], Body::from(layer(&[1]))).await.status,
        StatusCode::PRECONDITION_REQUIRED
    );
    let saved = put(page(0), "\"0\"", layer(&[1])).await;
    assert_eq!(saved.status, StatusCode::OK);
    assert_eq!(saved.json::<Value>()["revision"], 1);
    // A second device that still believes the page is new gets a conflict.
    let conflict = put(page(0), "\"0\"", layer(&[2])).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
    assert_eq!(conflict.json::<PageConflict>().current_revision, 1);
    assert_eq!(put(page(0), "1", layer(&[1, 2])).await.status, StatusCode::OK);

    let got = s.get(&page(0), &lib.anna).await;
    assert_eq!(got.headers[header::ETAG], "\"2\"");
    assert_eq!(got.body, layer(&[1, 2]));
    assert_eq!(put(page(1), "0", vec![1, 0xff, 0xff]).await.status, StatusCode::UNPROCESSABLE_ENTITY);
    // The part has 4 pages.
    assert_eq!(put(page(4), "0", layer(&[1])).await.status, StatusCode::NOT_FOUND);

    // Nobody else sees the set, admins included.
    let (_, bob) = s.member(&lib.admin, "bob").await;
    s.ok::<Group>(
        Method::PUT,
        &format!("/api/groups/{}/members", lib.trumpets),
        &lib.admin,
        json!({"ids": [lib.anna_id]}),
    )
    .await;
    assert_eq!(s.get(&page(0), &lib.admin).await.status, StatusCode::NOT_FOUND);
    assert_eq!(s.get(&page(0), &bob).await.status, StatusCode::NOT_FOUND);
    assert!(
        s.get(&format!("/api/parts/{part}/annotation-sets"), &lib.admin).await.json::<Vec<AnnotationSet>>().is_empty()
    );

    // Duplicate for another concert; make it the default.
    let copy: AnnotationSet = s
        .ok(
            Method::POST,
            &format!("/api/parts/{part}/annotation-sets"),
            &lib.anna,
            json!({"name": "Spring 2027", "copy_from": set.id}),
        )
        .await;
    assert_eq!(copy.pages.len(), 1);
    assert!(!copy.is_default);
    let copy: AnnotationSet =
        s.ok(Method::PATCH, &format!("/api/annotation-sets/{}", copy.id), &lib.anna, json!({"is_default": true})).await;
    assert!(copy.is_default);
    let sets: Vec<AnnotationSet> = s.get(&format!("/api/parts/{part}/annotation-sets"), &lib.anna).await.json();
    assert_eq!(sets.iter().filter(|s| s.is_default).count(), 1);
    let dup = s
        .json(Method::POST, &format!("/api/parts/{part}/annotation-sets"), &lib.anna, json!({"name": "spring 2027 "}))
        .await;
    assert!(dup.status == StatusCode::OK || dup.status == StatusCode::CONFLICT);

    // A corrected upload with the same page count: the set can move along.
    s.upload(&lib.admin, part, tiny_pdf(4, "corrected")).await;
    let moved: AnnotationSet =
        s.ok(Method::POST, &format!("/api/annotation-sets/{}/migrate", set.id), &lib.anna, json!({})).await;
    assert_eq!(moved.file_version, 2);
    s.upload(&lib.admin, part, tiny_pdf(5, "longer")).await;
    let refused = s.json(Method::POST, &format!("/api/annotation-sets/{}/migrate", set.id), &lib.anna, json!({})).await;
    assert_eq!(refused.status, StatusCode::CONFLICT);

    // Losing access hides the annotations too.
    s.ok::<Group>(Method::PUT, &format!("/api/groups/{}/members", lib.trumpets), &lib.admin, json!({"ids": []})).await;
    assert_eq!(s.get(&page(0), &lib.anna).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn programmes_and_search() {
    let s = Server::start().await;
    let lib = library(&s).await;
    let other: Score = s.ok(Method::POST, "/api/scores", &lib.admin, json!({"title": "Flute Concerto"})).await;
    let programme: Programme = s
        .ok(Method::POST, "/api/programmes", &lib.admin, json!({"title": "Christmas concert", "date": "2026-12-24"}))
        .await;
    let p: Programme = s
        .ok(
            Method::PUT,
            &format!("/api/programmes/{}/items", programme.id),
            &lib.admin,
            json!({"ids": [lib.score, other.id, lib.score]}),
        )
        .await;
    assert_eq!(p.items.len(), 3);
    s.ok::<Programme>(Method::POST, "/api/programmes", &lib.admin, json!({"title": "Empty"})).await;
    let bad = s.json(Method::POST, "/api/programmes", &lib.admin, json!({"title": "x", "date": "24.12.2026"})).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // Anna sees only what she plays: the march twice (encore), no concerto.
    let list: Vec<ProgrammeSummary> = s.get("/api/programmes", &lib.anna).await.json();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].item_count, 2);
    let mine: Programme = s.get(&format!("/api/programmes/{}", programme.id), &lib.anna).await.json();
    assert_eq!(mine.items.iter().map(|i| i.position).collect::<Vec<_>>(), vec![0, 2]);
    assert_eq!(mine.items[0].score.parts.len(), 1);

    // Search ignores diacritics and matches prefixes.
    let hits: Vec<Score> = s.get("/api/scores?q=fucik%20flor", &lib.anna).await.json();
    assert_eq!(hits.len(), 1);
    assert!(s.get("/api/scores?q=concerto", &lib.anna).await.json::<Vec<Score>>().is_empty());
    assert_eq!(s.get("/api/scores?q=concerto", &lib.admin).await.json::<Vec<Score>>().len(), 1);
    assert_eq!(s.get("/api/scores?tag=encore", &lib.anna).await.json::<Vec<Score>>().len(), 1);
    assert_eq!(s.get("/api/scores?genre=polka", &lib.anna).await.json::<Vec<Score>>().len(), 0);
}

#[tokio::test]
async fn user_administration() {
    let s = Server::start().await;
    let admin = s.admin().await;
    let created: CreatedUser = s
        .ok(
            Method::POST,
            "/api/users",
            &admin,
            json!({"username": "Carla", "display_name": "Carla", "role": "librarian"}),
        )
        .await;
    assert_eq!(created.user.username, "carla");
    assert!(!created.user.has_password);
    let dup = s
        .json(Method::POST, "/api/users", &admin, json!({"username": "carla", "display_name": "C", "role": "member"}))
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);

    s.accept_invite(&created.invite.token, "carla-password").await;
    let carla = s.login("carla", "carla-password").await;
    assert_eq!(s.get("/api/groups", &carla).await.status, StatusCode::OK);
    assert_eq!(s.json(Method::POST, "/api/groups", &carla, json!({"name": "x"})).await.status, StatusCode::FORBIDDEN);

    // Disabling logs out everywhere.
    let r = s.json(Method::PATCH, &format!("/api/users/{}", created.user.id), &admin, json!({"disabled": true})).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(s.get("/api/auth/me", &carla).await.status, StatusCode::UNAUTHORIZED);
    let me: Value = s.get("/api/auth/me", &admin).await.json();
    let self_disable =
        s.json(Method::PATCH, &format!("/api/users/{}", me["id"]), &admin, json!({"disabled": true})).await;
    assert_eq!(self_disable.status, StatusCode::CONFLICT);

    // Password change keeps the current session and ends the others.
    let other = s.login("admin", "admin-password-1").await;
    let r = s
        .json(
            Method::POST,
            "/api/auth/password",
            &admin,
            json!({"current_password": "admin-password-1", "new_password": "new-password-2"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(s.get("/api/auth/me", &admin).await.status, StatusCode::OK);
    assert_eq!(s.get("/api/auth/me", &other).await.status, StatusCode::UNAUTHORIZED);
    let sessions: Vec<musicus_api::SessionInfo> = s.get("/api/auth/sessions", &admin).await.json();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].current);
}
