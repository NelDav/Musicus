//! Assumption: SQLite keeps up with a large, long-lived library — 2 million
//! parts and 1 million saved annotation pages (~6 GB) — and only the write
//! concurrency differs noticeably from PostgreSQL. The mirror for
//! PostgreSQL is `pg-proto/src/bin/pg_scale.rs`; both run the same workload.
//!
//! Usage: scale_bench <db-path> [mixed-seconds]
//! Seeds the database on first run (takes a few minutes), reuses it afterwards.

use musicus_proto::*;
use rusqlite::params;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant},
};

pub const WORDS: [&str; 50] = [
    "alpine", "autumn", "ballad", "blue", "bright", "carnival", "castle", "celtic", "dance", "dawn",
    "dream", "eagle", "evening", "festival", "fire", "forest", "freedom", "galop", "glory", "golden",
    "harbour", "heroes", "highland", "hymn", "journey", "jubilee", "king", "legend", "light", "march",
    "meadow", "moon", "mountain", "night", "ocean", "parade", "polka", "prelude", "rhapsody", "river",
    "royal", "serenade", "silver", "spring", "star", "summer", "thunder", "valley", "waltz", "winter",
];
pub const COMPOSERS: [&str; 40] = [
    "Fucik", "Sousa", "Strauss", "Holst", "Elgar", "Grieg", "Dvorak", "Smetana", "Brahms", "Mozart",
    "Haydn", "Bach", "Handel", "Verdi", "Puccini", "Rossini", "Bizet", "Ravel", "Debussy", "Satie",
    "Mahler", "Bruckner", "Wagner", "Schubert", "Schumann", "Chopin", "Liszt", "Berlioz", "Sibelius", "Nielsen",
    "Janacek", "Bartok", "Kodaly", "Ligeti", "Gershwin", "Bernstein", "Copland", "Williams", "Morricone", "Zimmer",
];

pub const SETS: i64 = 250_000;
pub const PAGES_PER_SET: i64 = 4;
pub const PAGE_BYTES: usize = 6_144;
pub const PARTS: i64 = 2_000_000;
pub const USERS: i64 = 5_000;

fn title(s: i64) -> String {
    let w = |m: i64, a: i64| WORDS[((s * m + a) % 50) as usize];
    format!("{} {} {}", w(7, 0), w(13, 3), w(31, 11))
}

fn seed_all(path: &PathBuf) -> rusqlite::Result<()> {
    let mut conn = open(path)?;
    let t = Instant::now();
    seed(&mut conn, &SeedSize { users: USERS, groups: 200, instruments: 150, scores: 100_000, parts_per_score: 20 })?;
    println!("seeded metadata in {:.1?}", t.elapsed());

    let t = Instant::now();
    conn.execute_batch("CREATE VIRTUAL TABLE score_fts USING fts5(title, composer, content='');")?;
    let tx = conn.transaction()?;
    {
        let mut upd = tx.prepare("UPDATE scores SET title = ?2, composer = ?3 WHERE id = ?1")?;
        let mut fts = tx.prepare("INSERT INTO score_fts (rowid, title, composer) VALUES (?1, ?2, ?3)")?;
        for s in 0..100_000i64 {
            let (ti, co) = (title(s), COMPOSERS[((s * 17) % 40) as usize]);
            upd.execute(params![s, ti, co])?;
            fts.execute(params![s, ti, co])?;
        }
    }
    tx.commit()?;
    println!("built full-text index in {:.1?}", t.elapsed());

    let t = Instant::now();
    let mut rng = XorShift::new(3);
    let mut blob = vec![0u8; PAGE_BYTES];
    for chunk in 0..(SETS / 10_000) {
        let tx = conn.transaction()?;
        {
            let mut set = tx.prepare("INSERT INTO annotation_sets (id, user_id, part_id, name) VALUES (?1, ?2, ?3, ?4)")?;
            let mut page = tx.prepare("INSERT INTO annotation_pages (set_id, page, revision, data) VALUES (?1, ?2, 1, ?3)")?;
            for i in chunk * 10_000..(chunk + 1) * 10_000 {
                set.execute(params![i, i % USERS, rng.below(PARTS), format!("set {i}")])?;
                for p in 0..PAGES_PER_SET {
                    // zstd output is incompressible, so random bytes model the real payload.
                    blob.iter_mut().for_each(|b| *b = rng.next_u64() as u8);
                    page.execute(params![i, p, blob])?;
                }
            }
        }
        tx.commit()?;
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); ANALYZE;")?;
    println!("inserted {} annotation pages in {:.1?}", SETS * PAGES_PER_SET, t.elapsed());
    Ok(())
}

fn timed(label: &str, n: u32, mut f: impl FnMut() -> rusqlite::Result<()>) -> rusqlite::Result<()> {
    let t = Instant::now();
    for _ in 0..n {
        f()?;
    }
    println!("{label:<44} {:>10.1?} per query", t.elapsed() / n);
    Ok(())
}

fn pct(mut v: Vec<Duration>, p: f64) -> Duration {
    if v.is_empty() {
        return Duration::ZERO;
    }
    v.sort();
    v[((v.len() as f64 * p).ceil() as usize).saturating_sub(1).min(v.len() - 1)]
}

fn main() -> rusqlite::Result<()> {
    let path = PathBuf::from(std::env::args().nth(1).expect("db path"));
    let mixed_secs: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    if !path.exists() {
        seed_all(&path)?;
    }
    let size = |p: &PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) as f64 / 1e9;
    println!("database file: {:.2} GB", size(&path));

    let conn = open(&path)?;
    let mut rng = XorShift::new(11);
    let mut read = conn.prepare("SELECT data FROM annotation_pages WHERE set_id = ?1 AND page = ?2")?;
    timed("load one annotation page (6 KB)", 20_000, || {
        let d: Vec<u8> = read.query_row(params![rng.below(SETS), rng.below(PAGES_PER_SET)], |r| r.get(0))?;
        assert_eq!(d.len(), PAGE_BYTES);
        Ok(())
    })?;
    let mut sets = conn.prepare("SELECT id, name FROM annotation_sets WHERE user_id = ?1 AND part_id = ?2")?;
    timed("list a user's sets for a part", 20_000, || {
        sets.query_map(params![rng.below(USERS), rng.below(PARTS)], |r| r.get::<_, i64>(0))?.count();
        Ok(())
    })?;
    let mut check = conn.prepare(CAN_READ_PART)?;
    timed("access check, 2 000 000 parts", 20_000, || {
        let _: bool = check.query_row(params![rng.below(USERS), rng.below(PARTS)], |r| r.get(0))?;
        Ok(())
    })?;
    let mut fts = conn.prepare(
        "SELECT s.id, s.title FROM score_fts f JOIN scores s ON s.id = f.rowid
         WHERE score_fts MATCH ?1 ORDER BY rank LIMIT 50",
    )?;
    let mut hits = 0usize;
    timed("full-text search, 2 words, top 50 of 100 000", 2_000, || {
        let q = format!("{} {}", WORDS[rng.below(50) as usize], WORDS[rng.below(50) as usize]);
        hits += fts.query_map([q], |r| r.get::<_, i64>(0))?.count();
        Ok(())
    })?;
    println!("  (average {} hits returned)", hits / 2_000);
    drop((read, sets, check, fts));

    // Mixed load: READERS (default 8) readers + 4 writers for N seconds on the same database.
    // Writers share one connection behind a mutex: the recommended single-writer queue.
    let readers: u64 = std::env::var("READERS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = Arc::new(Mutex::new(open(&path)?));
    let mut handles = Vec::new();
    for t in 0..12u64 {
        let (stop, writer, path, readers) = (stop.clone(), writer.clone(), path.clone(), readers);
        handles.push(std::thread::spawn(move || -> (bool, Vec<Duration>) {
            let mut rng = XorShift::new(100 + t);
            let mut lat = Vec::new();
            let is_writer = t >= 8;
            if !is_writer && t >= readers {
                return (false, Vec::new());
            }
            let conn = open(&path).unwrap();
            let mut blob = vec![0u8; PAGE_BYTES];
            while !stop.load(Ordering::Relaxed) {
                let start = Instant::now();
                if is_writer {
                    blob.iter_mut().for_each(|b| *b = rng.next_u64() as u8);
                    let w = writer.lock().unwrap();
                    w.execute(
                        "UPDATE annotation_pages SET revision = revision + 1, data = ?3 WHERE set_id = ?1 AND page = ?2",
                        params![rng.below(SETS), rng.below(PAGES_PER_SET), blob],
                    )
                    .unwrap();
                } else {
                    let _: Vec<u8> = conn
                        .prepare_cached("SELECT data FROM annotation_pages WHERE set_id = ?1 AND page = ?2")
                        .unwrap()
                        .query_row(params![rng.below(SETS), rng.below(PAGES_PER_SET)], |r| r.get(0))
                        .unwrap();
                }
                lat.push(start.elapsed());
            }
            (is_writer, lat)
        }));
    }
    std::thread::sleep(Duration::from_secs(mixed_secs));
    stop.store(true, Ordering::Relaxed);
    let (mut r, mut w) = (Vec::new(), Vec::new());
    for h in handles {
        let (is_writer, lat) = h.join().unwrap();
        if is_writer { w.extend(lat) } else { r.extend(lat) }
    }
    println!(
        "mixed {readers} readers + 4 writers, {mixed_secs} s: {:.0} reads/s (p99 {:.2?}), {:.0} page saves/s (p99 {:.2?})",
        r.len() as f64 / mixed_secs as f64, pct(r, 0.99), w.len() as f64 / mixed_secs as f64, pct(w, 0.99)
    );

    if std::env::var("SKIP_BACKUP").is_ok() {
        return Ok(());
    }
    let backup = path.with_extension("backup");
    let _ = std::fs::remove_file(&backup);
    let t = Instant::now();
    conn.execute("VACUUM INTO ?1", [backup.to_str().unwrap()])?;
    println!("online backup (VACUUM INTO): {:.1?}, {:.2} GB", t.elapsed(), size(&backup));
    std::fs::remove_file(&backup).unwrap();
    Ok(())
}
