//! Assumption: replacing in-process SQLite with PostgreSQL (a separate
//! process, reached over a socket and a connection pool) keeps the
//! per-request access check and the burst cheap.
//!
//! Needs a database seeded with `schema_pg.sql`.
//! Usage: pg_bench <tcp|unix> <pool-size>

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{sync::Arc, time::{Duration, Instant}};
use tokio::sync::Barrier;

/// Same rule as `CAN_READ_PART`, with Postgres placeholders.
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

fn summary(label: &str, mut lat: Vec<Duration>) {
    lat.sort();
    let pct = |p: f64| lat[((lat.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    println!("{label:<44} p50={:>9.2?} p95={:>9.2?} max={:>9.2?}", pct(0.5), pct(0.95), lat[lat.len() - 1]);
}

#[tokio::main]
async fn main() -> Result<(), sqlx::Error> {
    let args: Vec<String> = std::env::args().collect();
    let transport = args.get(1).map(String::as_str).unwrap_or("tcp");
    let pool_size: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);

    let base = PgConnectOptions::new().username("musicus").database("musicus").port(5433);
    let opts = if transport == "unix" { base.socket("/tmp") } else { base.host("127.0.0.1") };
    let pool = PgPoolOptions::new().max_connections(pool_size).min_connections(pool_size).connect_with(opts).await?;

    // 1. Sequential point checks: the latency one request pays.
    let mut rng = XorShift::new(7);
    let n = 20_000;
    let t = Instant::now();
    for _ in 0..n {
        let _: bool = sqlx::query_scalar(CAN_READ_PART)
            .bind(rng.below(1000))
            .bind(rng.below(200_000))
            .fetch_one(&pool)
            .await?;
    }
    println!("{transport}, pool {pool_size}: point check {:?} per check (sequential)", t.elapsed() / n);

    // 2. Burst: 120 musicians open part 0 at once (check + file lookup).
    let clients = 120;
    for (label, save) in [("burst: 120 x (access check + file lookup)", false), ("burst: 120 x annotation page upsert (8 KB)", true)] {
        let barrier = Arc::new(Barrier::new(clients));
        let mut tasks = Vec::new();
        for u in 0..clients as i64 {
            let (pool, barrier) = (pool.clone(), barrier.clone());
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                let start = Instant::now();
                if save {
                    let mut tx = pool.begin().await.unwrap();
                    let set: i64 = sqlx::query_scalar(
                        "INSERT INTO annotation_sets (user_id, part_id, name) VALUES ($1, 0, 'default')
                         ON CONFLICT (user_id, part_id, name) DO UPDATE SET name = excluded.name RETURNING id",
                    )
                    .bind(u)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                    let _: i64 = sqlx::query_scalar(
                        "INSERT INTO annotation_pages (set_id, page, revision, data) VALUES ($1, $2, 1, $3)
                         ON CONFLICT (set_id, page) DO UPDATE SET revision = annotation_pages.revision + 1, data = excluded.data
                         RETURNING revision",
                    )
                    .bind(set)
                    .bind((u % 4) as i32)
                    .bind(vec![(u % 251) as u8; 8000])
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
                    tx.commit().await.unwrap();
                } else {
                    let ok: bool = sqlx::query_scalar(CAN_READ_PART).bind(u).bind(0i64).fetch_one(&pool).await.unwrap();
                    assert!(ok);
                    let _: String = sqlx::query_scalar(
                        "SELECT f.sha256 FROM parts p JOIN files f ON f.id = p.file_id WHERE p.id = $1",
                    )
                    .bind(0i64)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                }
                start.elapsed()
            }));
        }
        let mut lat = Vec::new();
        for t in tasks {
            lat.push(t.await.unwrap());
        }
        summary(label, lat);
    }
    // Keep the pool open so that the backends' memory can be measured.
    if let Ok(secs) = std::env::var("HOLD_SECS") {
        tokio::time::sleep(Duration::from_secs(secs.parse().unwrap())).await;
    }
    Ok(())
}

/// Same deterministic PRNG as in musicus-proto.
struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn below(&mut self, n: i64) -> i64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x % n as u64) as i64
    }
}
