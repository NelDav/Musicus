//! Shared helpers for the Musicus concept prototypes.
//!
//! These are throw-away experiments to test assumptions of the concept
//! (SQLite as the only database, access checks per request, burst downloads),
//! not production code.

use rusqlite::{Connection, params};
use std::path::Path;

pub const SCHEMA: &str = include_str!("schema.sql");

/// Tiny deterministic PRNG so that runs are reproducible without pulling in `rand`.
pub struct XorShift(u64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn below(&mut self, n: i64) -> i64 {
        (self.next_u64() % n as u64) as i64
    }
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Opens a connection with the pragmas the concept recommends for a small server.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA cache_size = -16000;",
    )?;
    Ok(conn)
}

pub struct SeedSize {
    pub users: i64,
    pub groups: i64,
    pub instruments: i64,
    pub scores: i64,
    pub parts_per_score: i64,
}

/// Fills an empty database with a synthetic music library.
///
/// Every group is granted 1–3 instruments, every user is in 1–2 groups,
/// every part has 1–2 instruments, 2 % of scores get an extra score grant.
pub fn seed(conn: &mut Connection, size: &SeedSize) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    let mut rng = XorShift::new(42);
    let tx = conn.transaction()?;
    {
        let mut ins = tx.prepare("INSERT INTO instruments (id, name, family) VALUES (?1, ?2, ?3)")?;
        for i in 0..size.instruments {
            ins.execute(params![i, format!("Instrument {i}"), format!("Family {}", i % 5)])?;
        }
        let mut grp = tx.prepare("INSERT INTO groups (id, name) VALUES (?1, ?2)")?;
        let mut gig = tx.prepare("INSERT OR IGNORE INTO group_instrument_grants VALUES (?1, ?2)")?;
        for g in 0..size.groups {
            grp.execute(params![g, format!("Group {g}")])?;
            for _ in 0..(1 + rng.below(3)) {
                gig.execute(params![g, rng.below(size.instruments)])?;
            }
        }
        let mut usr = tx.prepare(
            "INSERT INTO users (id, username, display_name, password_hash, role) VALUES (?1, ?2, ?2, 'x', 'member')",
        )?;
        let mut mem = tx.prepare("INSERT OR IGNORE INTO group_members VALUES (?1, ?2)")?;
        for u in 0..size.users {
            usr.execute(params![u, format!("user{u}")])?;
            for _ in 0..(1 + rng.below(2)) {
                mem.execute(params![rng.below(size.groups), u])?;
            }
        }
        let mut file = tx.prepare(
            "INSERT INTO files (id, sha256, mime, size_bytes, page_count) VALUES (?1, ?2, 'application/pdf', 3000000, 4)",
        )?;
        let mut score = tx.prepare("INSERT INTO scores (id, title) VALUES (?1, ?2)")?;
        let mut part = tx.prepare("INSERT INTO parts (id, score_id, label, file_id) VALUES (?1, ?2, ?3, ?1)")?;
        let mut pin = tx.prepare("INSERT OR IGNORE INTO part_instruments VALUES (?1, ?2)")?;
        let mut sg = tx.prepare("INSERT OR IGNORE INTO group_score_grants VALUES (?1, ?2)")?;
        let mut pid = 0i64;
        for s in 0..size.scores {
            score.execute(params![s, format!("Score {s}")])?;
            if rng.below(50) == 0 {
                sg.execute(params![rng.below(size.groups), s])?;
            }
            for k in 0..size.parts_per_score {
                file.execute(params![pid, format!("{pid:064x}")])?;
                part.execute(params![pid, s, format!("Part {k}")])?;
                for _ in 0..(1 + rng.below(2)) {
                    pin.execute(params![pid, rng.below(size.instruments)])?;
                }
                pid += 1;
            }
        }
    }
    tx.commit()?;
    conn.execute_batch("ANALYZE;")?;
    Ok(())
}

/// The point check executed before every file download.
pub const CAN_READ_PART: &str = "
SELECT EXISTS (
    SELECT 1 FROM part_instruments pi
    JOIN group_instrument_grants g ON g.instrument_id = pi.instrument_id
    JOIN group_members m ON m.group_id = g.group_id
    WHERE pi.part_id = ?2 AND m.user_id = ?1
) OR EXISTS (
    SELECT 1 FROM parts p
    JOIN group_score_grants s ON s.score_id = p.score_id
    JOIN group_members m ON m.group_id = s.group_id
    WHERE p.id = ?2 AND m.user_id = ?1
)";

/// The library listing: every part a user may see.
pub const VISIBLE_PARTS: &str = "
SELECT pi.part_id FROM group_members m
JOIN group_instrument_grants g ON g.group_id = m.group_id
JOIN part_instruments pi ON pi.instrument_id = g.instrument_id
WHERE m.user_id = ?1
UNION
SELECT p.id FROM group_members m
JOIN group_score_grants s ON s.group_id = m.group_id
JOIN parts p ON p.score_id = s.score_id
WHERE m.user_id = ?1";
