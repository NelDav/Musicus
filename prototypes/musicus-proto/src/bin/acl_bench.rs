//! Assumption: SQLite answers the per-request access check fast enough that it
//! does not need a cache, even for a large library.

use musicus_proto::*;
use std::time::Instant;

fn main() -> rusqlite::Result<()> {
    let dir = std::env::temp_dir().join("musicus-acl-bench");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("acl.sqlite");

    for (label, size) in [
        ("small band", SeedSize { users: 60, groups: 15, instruments: 30, scores: 500, parts_per_score: 15 }),
        ("large library", SeedSize { users: 1000, groups: 60, instruments: 80, scores: 10_000, parts_per_score: 20 }),
    ] {
        let _ = std::fs::remove_file(&db);
        let mut conn = open(&db)?;
        let t = Instant::now();
        seed(&mut conn, &size)?;
        let parts = size.scores * size.parts_per_score;
        let db_bytes = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
        println!(
            "\n== {label}: {} users, {} groups, {} scores, {parts} parts (seeded in {:.1?}, db {:.1} MB)",
            size.users, size.groups, size.scores, t.elapsed(), db_bytes as f64 / 1e6
        );

        let mut rng = XorShift::new(7);
        let mut check = conn.prepare(CAN_READ_PART)?;
        let n = 100_000;
        let t = Instant::now();
        let mut allowed = 0i64;
        for _ in 0..n {
            let ok: bool = check.query_row((rng.below(size.users), rng.below(parts)), |r| r.get(0))?;
            allowed += ok as i64;
        }
        let per = t.elapsed() / n;
        println!("point check  : {per:?} per check ({:.1} % allowed)", allowed as f64 * 100.0 / n as f64);

        let mut list = conn.prepare(VISIBLE_PARTS)?;
        let n = 1000;
        let t = Instant::now();
        let mut rows = 0usize;
        for _ in 0..n {
            rows += list.query_map([rng.below(size.users)], |r| r.get::<_, i64>(0))?.count();
        }
        println!("library list : {:?} per user ({} parts visible on average)", t.elapsed() / n, rows / n as usize);

        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {CAN_READ_PART}"))?
            .query_map((1, 1), |r| r.get::<_, String>(3))?
            .collect::<Result<_, _>>()?;
        println!("plan         : {}", plan.join(" | "));
    }
    Ok(())
}
