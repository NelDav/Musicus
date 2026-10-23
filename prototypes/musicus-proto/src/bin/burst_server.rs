//! Assumption: one small Rust process can serve the "everyone opens the same
//! song at the same moment" burst, with an access check on every request,
//! without the database or the disk becoming the bottleneck.
//!
//! Usage: burst_server <disk|mem> <tokio-workers> <file-size-MB> [port]

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use musicus_proto::*;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};
use tower::ServiceExt;
use tower_http::services::ServeFile;

#[derive(Clone)]
struct App {
    db: PathBuf,
    blobs: PathBuf,
    mem_cache: bool,
    cache: Arc<RwLock<HashMap<String, Bytes>>>,
    writer: Arc<Mutex<Connection>>,
}

thread_local! {
    // One read connection per blocking thread; WAL lets them all read while
    // the single writer connection writes.
    static READER: RefCell<Option<Connection>> = const { RefCell::new(None) };
}

fn with_reader<T>(db: &PathBuf, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    READER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(open(db)?);
        }
        f(slot.as_ref().unwrap())
    })
}

fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_owned)
}

/// Session lookup + access check + file lookup in one blocking call.
fn authorize(conn: &Connection, token: &str, part: i64) -> rusqlite::Result<Option<(i64, String)>> {
    let now = 0i64; // the prototype does not model expiry
    let Some(user): Option<i64> = conn
        .prepare_cached("SELECT user_id FROM sessions WHERE token_hash = ?1 AND expires_at > ?2")?
        .query_row(params![token_hash(token), now], |r| r.get(0))
        .optional()?
    else {
        return Ok(None);
    };
    let allowed: bool = conn.prepare_cached(CAN_READ_PART)?.query_row(params![user, part], |r| r.get(0))?;
    if !allowed {
        return Ok(None);
    }
    let sha: String = conn
        .prepare_cached("SELECT f.sha256 FROM parts p JOIN files f ON f.id = p.file_id WHERE p.id = ?1")?
        .query_row([part], |r| r.get(0))?;
    Ok(Some((user, sha)))
}

async fn get_file(State(app): State<App>, Path(part): Path<i64>, req: Request) -> Response {
    let Some(token) = bearer(req.headers()) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let db = app.db.clone();
    let auth = tokio::task::spawn_blocking(move || with_reader(&db, |c| authorize(c, &token, part)))
        .await
        .unwrap();
    let sha = match auth {
        Ok(Some((_, sha))) => sha,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(), // do not reveal existence
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let etag = format!("\"{sha}\"");
    if req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let path = app.blobs.join(&sha[..2]).join(&sha);

    let mut resp = if app.mem_cache {
        let cached = app.cache.read().unwrap().get(&sha).cloned();
        let bytes = match cached {
            Some(b) => b,
            None => {
                let b = Bytes::from(tokio::fs::read(&path).await.unwrap());
                app.cache.write().unwrap().insert(sha.clone(), b.clone());
                b
            }
        };
        Response::new(Body::from(bytes))
    } else {
        // ServeFile streams from disk (page cache) and handles Range requests.
        ServeFile::new(&path).oneshot(req).await.unwrap().map(Body::new)
    };
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/pdf"));
    h.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    // Immutable because the URL content is addressed by hash; private because
    // it is access controlled.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    resp
}

async fn save_page(
    State(app): State<App>,
    Path((part, page)): Path<(i64, i64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let db = app.db.clone();
    let auth = tokio::task::spawn_blocking(move || with_reader(&db, |c| authorize(c, &token, part)))
        .await
        .unwrap();
    let user = match auth {
        Ok(Some((user, _))) => user,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let writer = app.writer.clone();
    let res = tokio::task::spawn_blocking(move || -> rusqlite::Result<i64> {
        let mut conn = writer.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO annotation_sets (user_id, part_id, name) VALUES (?1, ?2, 'default')",
            params![user, part],
        )?;
        let set: i64 = tx.query_row(
            "SELECT id FROM annotation_sets WHERE user_id = ?1 AND part_id = ?2 AND name = 'default'",
            params![user, part],
            |r| r.get(0),
        )?;
        let rev: i64 = tx.query_row(
            "INSERT INTO annotation_pages (set_id, page, revision, data) VALUES (?1, ?2, 1, ?3)
             ON CONFLICT (set_id, page) DO UPDATE SET revision = revision + 1, data = excluded.data
             RETURNING revision",
            params![set, page, body.as_ref()],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(rev)
    })
    .await
    .unwrap();
    match res {
        Ok(rev) => rev.to_string().into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn stats() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .filter(|l| l.starts_with("VmRSS") || l.starts_with("VmHWM") || l.starts_with("Threads"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn prepare(dir: &PathBuf, size_mb: usize) -> (PathBuf, PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let db = dir.join("musicus.sqlite");
    let mut conn = open(&db).unwrap();
    seed(
        &mut conn,
        &SeedSize { users: 300, groups: 20, instruments: 40, scores: 2000, parts_per_score: 15 },
    )
    .unwrap();

    // One real blob for part 0, readable by every user through a score grant.
    let mut rng = XorShift::new(1);
    let data: Vec<u8> = (0..size_mb * 1_000_000).map(|_| rng.next_u64() as u8).collect();
    let sha = hex::encode(Sha256::digest(&data));
    let blobs = dir.join("blobs");
    std::fs::create_dir_all(blobs.join(&sha[..2])).unwrap();
    std::fs::write(blobs.join(&sha[..2]).join(&sha), &data).unwrap();
    conn.execute_batch(&format!(
        "UPDATE files SET sha256 = '{sha}', size_bytes = {} WHERE id = 0;
         INSERT INTO groups (id, name) VALUES (1000, 'Everyone');
         INSERT INTO group_members SELECT 1000, id FROM users;
         INSERT INTO group_score_grants VALUES (1000, 0);",
        data.len()
    ))
    .unwrap();
    let tx = conn.transaction().unwrap();
    for u in 0..300i64 {
        tx.execute(
            "INSERT INTO sessions VALUES (?1, ?2, ?3)",
            params![token_hash(&format!("tok{u}")), u, i64::MAX],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    (db, blobs)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("mem");
    let workers: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2);
    let size_mb: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let port: u16 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(8080);

    let dir = std::env::temp_dir().join(format!("musicus-burst-{port}"));
    let (db, blobs) = prepare(&dir, size_mb);
    let app = App {
        writer: Arc::new(Mutex::new(open(&db).unwrap())),
        db,
        blobs,
        mem_cache: mode == "mem",
        cache: Default::default(),
    };
    let router = Router::new()
        .route("/api/parts/{id}/file", get(get_file))
        .route("/api/parts/{id}/annotations/default/pages/{page}", post(save_page))
        .route("/stats", get(stats))
        .with_state(app);

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .max_blocking_threads(8)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
            eprintln!("ready mode={mode} workers={workers} size={size_mb}MB port={port}");
            axum::serve(listener, router).await.unwrap();
        });
}
