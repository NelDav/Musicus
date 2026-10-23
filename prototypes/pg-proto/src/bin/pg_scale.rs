//! PostgreSQL mirror of musicus-proto's `scale_bench`: the same queries and
//! the same mixed load on 2 000 000 parts and 1 000 000 annotation pages.
//!
//! Needs a database seeded with `scale_pg.sql`.
//! Usage: pg_scale [mixed-seconds]

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{
    sync::{Arc, atomic::{AtomicBool, Ordering}},
    time::{Duration, Instant},
};

const WORDS: [&str; 50] = [
    "alpine", "autumn", "ballad", "blue", "bright", "carnival", "castle", "celtic", "dance", "dawn",
    "dream", "eagle", "evening", "festival", "fire", "forest", "freedom", "galop", "glory", "golden",
    "harbour", "heroes", "highland", "hymn", "journey", "jubilee", "king", "legend", "light", "march",
    "meadow", "moon", "mountain", "night", "ocean", "parade", "polka", "prelude", "rhapsody", "river",
    "royal", "serenade", "silver", "spring", "star", "summer", "thunder", "valley", "waltz", "winter",
];
const SETS: i64 = 250_000;
const PAGES_PER_SET: i64 = 4;
const PAGE_BYTES: usize = 6_144;
const PARTS: i64 = 2_000_000;
const USERS: i64 = 5_000;

const CAN_READ_PART: &str = "
SELECT EXISTS (
    SELECT 1 FROM part_instruments pi
    JOIN group_instrument_grants g ON g.instrument_id = pi.instrument_id
    JOIN group_members m ON m.group_id = g.group_id
    WHERE pi.part_id = $2 AND m.user_id = $1
) OR EXISTS (
    SELECT 1 FROM parts p
    JOIN group_score_grants s ON s.score_id = p.score_id
    JOIN group_members m ON m.group_id = s.group_id
    WHERE p.id = $2 AND m.user_id = $1
)";

struct XorShift(u64);

impl XorShift {
    fn below(&mut self, n: i64) -> i64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x % n as u64) as i64
    }
}

fn pct(mut v: Vec<Duration>, p: f64) -> Duration {
    if v.is_empty() {
        return Duration::ZERO;
    }
    v.sort();
    v[((v.len() as f64 * p).ceil() as usize).saturating_sub(1).min(v.len() - 1)]
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let mixed_secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let opts = PgConnectOptions::new().username("musicus").database("musicus").port(5433).socket("/tmp");
    let pool = PgPoolOptions::new().max_connections(12).connect_with(opts).await?;
    let size: i64 = sqlx::query_scalar("SELECT pg_database_size('musicus')").fetch_one(&pool).await?;
    println!("database size: {:.2} GB", size as f64 / 1e9);

    let mut rng = XorShift(11);
    macro_rules! timed {
        ($label:expr, $n:expr, $body:expr) => {{
            let t = Instant::now();
            for _ in 0..$n {
                $body;
            }
            println!("{:<44} {:>10.1?} per query", $label, t.elapsed() / $n);
        }};
    }
    timed!("load one annotation page (6 KB)", 20_000u32, {
        let d: Vec<u8> = sqlx::query_scalar("SELECT data FROM annotation_pages WHERE set_id = $1 AND page = $2")
            .bind(rng.below(SETS)).bind(rng.below(PAGES_PER_SET) as i32).fetch_one(&pool).await?;
        assert_eq!(d.len(), PAGE_BYTES);
    });
    timed!("list a user's sets for a part", 20_000u32, {
        let _: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM annotation_sets WHERE user_id = $1 AND part_id = $2")
            .bind(rng.below(USERS)).bind(rng.below(PARTS)).fetch_all(&pool).await?;
    });
    timed!("access check, 2 000 000 parts", 20_000u32, {
        let _: bool = sqlx::query_scalar(CAN_READ_PART).bind(rng.below(USERS)).bind(rng.below(PARTS)).fetch_one(&pool).await?;
    });
    let mut hits = 0usize;
    timed!("full-text search, 2 words, top 50 of 100 000", 2_000u32, {
        let q = format!("{} & {}", WORDS[rng.below(50) as usize], WORDS[rng.below(50) as usize]);
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, title FROM scores, to_tsquery('simple', $1) q WHERE tsv @@ q
             ORDER BY ts_rank(tsv, q) DESC LIMIT 50",
        ).bind(q).fetch_all(&pool).await?;
        hits += rows.len();
    });
    println!("  (average {} hits returned)", hits / 2_000);

    let readers: u64 = std::env::var("READERS").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let stop = Arc::new(AtomicBool::new(false));
    let mut tasks = Vec::new();
    for t in 0..12u64 {
        let (pool, stop, readers) = (pool.clone(), stop.clone(), readers);
        tasks.push(tokio::spawn(async move {
            let mut rng = XorShift(100 + t);
            let is_writer = t >= 8;
            if !is_writer && t >= readers {
                return (false, Vec::new());
            }
            let mut lat = Vec::new();
            let mut blob = vec![0u8; PAGE_BYTES];
            while !stop.load(Ordering::Relaxed) {
                let start = Instant::now();
                if is_writer {
                    blob.iter_mut().for_each(|b| *b = rng.below(256) as u8);
                    sqlx::query("UPDATE annotation_pages SET revision = revision + 1, data = $3 WHERE set_id = $1 AND page = $2")
                        .bind(rng.below(SETS)).bind(rng.below(PAGES_PER_SET) as i32).bind(&blob)
                        .execute(&pool).await.unwrap();
                } else {
                    let _: Vec<u8> = sqlx::query_scalar("SELECT data FROM annotation_pages WHERE set_id = $1 AND page = $2")
                        .bind(rng.below(SETS)).bind(rng.below(PAGES_PER_SET) as i32)
                        .fetch_one(&pool).await.unwrap();
                }
                lat.push(start.elapsed());
            }
            (is_writer, lat)
        }));
    }
    tokio::time::sleep(Duration::from_secs(mixed_secs)).await;
    stop.store(true, Ordering::Relaxed);
    let (mut r, mut w) = (Vec::new(), Vec::new());
    for t in tasks {
        let (is_writer, lat) = t.await.unwrap();
        if is_writer { w.extend(lat) } else { r.extend(lat) }
    }
    println!(
        "mixed {readers} readers + 4 writers, {mixed_secs} s: {:.0} reads/s (p99 {:.2?}), {:.0} page saves/s (p99 {:.2?})",
        r.len() as f64 / mixed_secs as f64, pct(r, 0.99), w.len() as f64 / mixed_secs as f64, pct(w, 0.99)
    );
    Ok(())
}
