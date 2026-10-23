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
