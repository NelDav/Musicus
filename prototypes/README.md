# Musicus concept prototypes

Throw-away experiments that test the assumptions of the Musicus concept
(single Rust binary, SQLite, content-addressed file store, Dioxus client).
They are **not** production code. The concept itself, with all results, is in
the "Musicus – Conceptual Design" doc.

| Crate / file | Assumption tested | Run |
| --- | --- | --- |
| `musicus-proto/src/schema.sql` | Access model: groups → instruments → parts | used by the binaries below |
| `musicus-proto` `acl_bench` | SQL access check is fast enough without a cache | `cargo run --release --bin acl_bench` |
| `musicus-proto` `burst_server` + `burst_client` | One core serves "everyone opens the same piece" | `./run_burst.sh mem 0 1 3 8081` (mode, server cores, workers, MB, port) |
| `musicus-proto` `annot_size` | Vector annotations stay a few KB per page | `cargo run --release --bin annot_size` |
| `musicus-proto` `argon_burst` | Login hashing must be bounded on small servers | `cargo run --release --bin argon_burst` |
| `pdf-render` | hayro (pure-Rust PDF) renders fast enough | `cargo run --release -- file.pdf [page1.png]` |
| `pdf-render/scripts/make_sheet.py` | Synthetic engraved + scanned test parts | `python3 make_sheet.py <out-dir>` (needs reportlab, Pillow) |
| `pg-proto` | PostgreSQL variant: access check and burst over a pool | seed with `psql -f schema_pg.sql`, then `cargo run --release -- tcp 4` |
| `musicus-proto` `scale_bench` + `pg-proto` `pg_scale` | Large library (2 M parts, 1 M annotation pages, ~9 GB) in SQLite vs PostgreSQL | `scale_bench <db-file> 20`; `psql -f scale_pg.sql`, then `pg_scale 20` (env `READERS` = 0..8) |
| `pg-proto/docker-compose.example.yml` | Packaging sketch: server + PostgreSQL | `docker compose -f docker-compose.example.yml config` |
| `pdf-render-wasm` | WASM size of hayro | `cargo build --release --target wasm32-unknown-unknown` |

`pdf-render-wasm` is built without hayro's default features (no bundled fonts,
3.7 MB / 1.1 MB gzip). With default features it was 4.5 MB / 1.7 MB gzip.

## Results (4-core x86-64 container, server pinned to one core, loopback)

| Test | Result |
| --- | --- |
| 120 clients download the same 3 MB part at once (RAM cache) | p50 116 ms, max 199 ms, 27 MB RSS |
| Same, served from disk via `ServeFile` | p50 163 ms, max 263 ms, 25 MB RSS |
| 120 ETag revalidations (304) | max 10 ms |
| 120 simultaneous 8 KB annotation saves (single SQLite writer) | max 22 ms |
| Access check, 200 000 parts / 1 000 users | 24 µs |
| Argon2id verify (m = 19 MiB, t = 2, p = 1) | 21 ms |
| hayro, engraved / scanned A4 page at 180 dpi | 15 ms / 57 ms |
| Annotation page, 30 strokes × 60 points | 61 KB JSON → 7 KB postcard + delta + zstd |

## PostgreSQL variant (PostgreSQL 16 pinned to one core, sqlx pool)

| Test | Result |
| --- | --- |
| Access check, 200 000 parts, sequential | 0.22 ms (TCP and Unix socket alike) |
| 120 x (access check + file lookup) at once | max 26 ms (pool 4), 28-41 ms (pool 10) |
| 120 simultaneous 8 KB annotation upserts | max 38 ms (pool 4), 60-90 ms (pool 10) |
| PostgreSQL memory (PSS, all processes) | 28 MB idle, 67 MB (pool 4) / 78 MB (pool 10) under load |
| Database size for the same library | 109 MB (SQLite: 47 MB), data directory with WAL 356 MB |

`sqlx` and `rusqlite` cannot share a crate (both link the native sqlite3
library), so the PostgreSQL benchmark lives in its own crate.

## Large library: SQLite vs PostgreSQL (server and database on the same 2 cores)

| Measure | SQLite | PostgreSQL 16 |
| --- | --- | --- |
| Size on disk | 9.3 GB | 9.5 GB + 1.1 GB WAL |
| Load one 6 KB annotation page | 43 µs | 290 µs |
| Access check, 2 000 000 parts | 37 µs | 366 µs |
| Full-text search, 2 words | 0.7 ms | 1.3 ms |
| 4 writers only: saves/s (p99) | 2 500-2 800 (22-31 ms) | 3 200-3 500 (3-4 ms) |
| 8 readers + 4 writers: reads/s, saves/s (p99) | 21 400-21 800, 560-670 (63-85 ms) | 8 600-9 000, 1 700-1 800 (6 ms) |
| Full online backup | 46 s (`VACUUM INTO`) | 45 s (`pg_dump -Z0`) |
