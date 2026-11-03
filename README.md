# Musicus

Musicus provides sheet music to the members of an orchestra or band. A
librarian uploads PDF parts. Access is granted to groups (e.g. group
"Trumpets" reads every part tagged with the instrument "Trumpet"). Every
member can draw private annotations on their parts, in several named sets
(e.g. one per concert), without changing the PDF.

This is the first implementation of **concept A** from the design document
"Musicus – Conceptual Design": one Rust binary (Axum + SQLite in WAL mode + a
content-addressed file store) that also serves a Dioxus web client.

```
crates/
  musicus-api/     shared types: JSON bodies and the annotation (ink) format
  musicus-server/  Axum server, SQLite migrations, file store, CLI
  musicus-web/     Dioxus 0.7 web client (WebAssembly) with a pdf.js bridge
e2e/               browser smoke test (Playwright)
prototypes/        throw-away benchmarks behind the concept
scripts/           fetch-pdfjs.sh
```

## Build and run

Prerequisites: Rust 1.88 or newer, the `wasm32-unknown-unknown` target and
the Dioxus CLI 0.7.10 (`dx`).

```sh
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version 0.7.10 --locked   # or a prebuilt `dx` release

# 1. pdf.js (pinned, checksum-verified) for the web client
./scripts/fetch-pdfjs.sh

# 2. the web client -> target/dx/musicus-web/release/web/public
dx bundle --platform web --package musicus-web --release

# 3. the server
cargo build --release -p musicus-server
./target/release/musicus-server serve \
    --data-dir ./data \
    --web-dir target/dx/musicus-web/release/web/public \
    --public-url http://localhost:8080
```

On the first start the server creates the user `admin` and logs a one-time
link to choose its password. Open it, then use **Manage** to add instruments,
scores, parts (upload the PDFs), groups and users. New users get an invite
link instead of a password; send it to them, and they choose their own.

For development, `dx build --platform web --package musicus-web` builds a
debug client into `target/dx/musicus-web/debug/web/public`. Release builds
run `wasm-opt`, which `dx` downloads on first use.

## Docker

The `Dockerfile` builds the web client and the server and puts them into a
small image based on distroless (no shell, runs as an unprivileged user,
data in the volume `/data`). `compose.yaml` runs it:

```sh
cp .env.example .env                  # optional: adjust settings
docker compose up -d --build
docker compose logs musicus           # the one-time admin setup link
```

The web client is then on http://localhost:8080. Tablets and phones on the
network need HTTPS (see below). With a domain that points to the machine,
set `MUSICUS_DOMAIN`, `MUSICUS_PUBLIC_URL`, `MUSICUS_BIND=127.0.0.1` and
`MUSICUS_TRUST_PROXY=true` in `.env`, then start with Caddy in front. Caddy
fetches and renews the Let's Encrypt certificate itself:

```sh
docker compose --profile https up -d --build
```

Backups while running: `docker compose exec musicus musicus-server backup --to
/data/backup-$(date +%F).sqlite`, then copy the volume's `blobs/` directory.
The image has a health check (`musicus-server healthcheck`, which calls
`GET /api/health`). For a Raspberry Pi, build on the Pi or use
`docker buildx build --platform linux/arm64`. The first build takes a while,
because it compiles the Dioxus CLI.


All options can be passed as flags or environment variables
(`musicus-server serve --help`).

| Variable | Default | Meaning |
| --- | --- | --- |
| `MUSICUS_DATA_DIR` | `data` | Holds `musicus.sqlite`, `blobs/` and `tmp/` |
| `MUSICUS_LISTEN` | `0.0.0.0:8080` | Address and port |
| `MUSICUS_WEB_DIR` | none | Built web client; without it only `/api` is served |
| `MUSICUS_PUBLIC_URL` | `http://localhost:8080` | Used in printed setup links |
| `MUSICUS_TLS_CERT`, `MUSICUS_TLS_KEY` | none | PEM files: serve HTTPS directly (rustls) |
| `MUSICUS_INSECURE_COOKIES` | off | Drop the cookie's `Secure` flag (plain-HTTP development on a LAN IP only) |
| `MUSICUS_CLIENT_CACHE` | `none` | `none`: browsers may not store sheets (`no-store`); `immutable`: cache privately for a year |
| `MUSICUS_RAM_CACHE_MB` | 128 | RAM for hot sheet files (tonight's programme) |
| `MUSICUS_MAX_UPLOAD_MB` | 200 | Largest upload |
| `MUSICUS_SESSION_DAYS` | 60 | Sliding session lifetime, so tablets stay logged in |
| `MUSICUS_TRUST_PROXY` | off | Read the client IP from `X-Forwarded-For` (behind a reverse proxy) |

Log levels follow `RUST_LOG` (default `info`).

### Deployment notes

- **HTTPS is required in practice.** Browsers only allow the screen wake lock
  and the `Secure` session cookie in secure contexts. Either pass a
  certificate or put Caddy/nginx in front. On a Raspberry Pi without internet
  access, use a real domain with a DNS-01 certificate.
- **Backups:** `musicus-server backup --data-dir data --to backup.sqlite`
  writes a consistent copy while the server runs. Then back up `data/blobs/`.
  Files there are immutable, so incremental tools (restic, rsync) work well.
- **Resources:** the release binary idles at about 11 MB RSS. The prototypes
  served 120 simultaneous downloads of a 3 MB part on one core in 0.2 s; the
  rehearsal-room Wi-Fi is the real limit, which is why the client downloads
  a whole programme ahead of time.

## How it works

- **Access control:** a member may read a part if one of their groups is
  granted one of the part's instruments, or the whole score (e.g. for the
  conductor). Librarians and admins read everything. Denied reads answer 404.
  The check is one indexed SQL query per request; revocations apply at once.
- **Files** are stored as `blobs/ab/<sha256>` and served at
  `/api/files/<sha256>` with `ETag`, `Range` support and a RAM cache.
  Uploading a corrected PDF creates a new version of the part. Old versions
  stay readable for annotations drawn on them, and a set can be moved to the
  new version when the page count is unchanged.
- **Annotations** are vector objects (pen and highlighter strokes with
  pressure, stamps, text) in page-relative coordinates. They are encoded with
  postcard plus delta coding (about 7 KB for a typical page) and stored
  zstd-compressed per page. Saves use `If-Match` revisions. A conflicting
  save from another device answers 409, and the client merges by object id.
  Pages are saved 1.5 s after the last stroke, on page turns, and (with a
  `keepalive` request) when the tab is closed.
- **The client** keeps nothing on the device. Opening a programme downloads
  all its sheets into memory, so page turns and the next piece need no
  network. The reader turns pages by tap, arrow/Page keys or a Bluetooth foot
  pedal, keeps the screen awake, and ignores fingers while a pen is used.
- **Accounts:** Argon2id (19 MiB, t = 2) with a semaphore bounding parallel
  hashing. Opaque session tokens are stored hashed: an `HttpOnly` cookie plus
  a CSRF header for the web, bearer tokens for future apps. Failed logins are
  limited (10 per 15 minutes per account and per IP).

## API overview

All endpoints are under `/api`; the JSON bodies are defined in
`crates/musicus-api/src/dto.rs`.

| Area | Endpoints |
| --- | --- |
| Session | `POST auth/login`, `POST auth/logout`, `GET auth/me`, `POST auth/password`, `GET auth/sessions`, `DELETE auth/sessions/{id}`, `GET/POST auth/invites/{token}` |
| Users (admin) | `GET/POST users`, `PATCH users/{id}`, `POST users/{id}/invite` |
| Instruments | `GET/POST instruments`, `PUT/DELETE instruments/{id}` |
| Groups | `GET/POST groups`, `PUT/DELETE groups/{id}`, `PUT groups/{id}/members`, `PUT groups/{id}/instrument-grants`, `PUT groups/{id}/score-grants` |
| Library | `GET scores?q=&genre=&tag=`, `POST scores`, `GET/PUT/DELETE scores/{id}`, `POST scores/{id}/parts`, `GET/PUT/DELETE parts/{id}`, `PUT parts/{id}/file` (raw PDF body), `GET parts/{id}/versions` |
| Files | `GET files/{sha256}`, `GET files/{sha256}/thumbnail` |
| Health | `GET health` (no login; checks the database) |
| Programmes | `GET/POST programmes`, `GET/PUT/DELETE programmes/{id}`, `PUT programmes/{id}/items` |
| Annotations | `GET/POST parts/{id}/annotation-sets`, `PATCH/DELETE annotation-sets/{id}`, `POST annotation-sets/{id}/migrate`, `GET/PUT annotation-sets/{id}/pages/{page}` (binary, `If-Match`) |

## Tests

```sh
cargo test                     # shared types, server unit and API tests
cargo clippy --all-targets
cargo clippy -p musicus-web --target wasm32-unknown-unknown
```

`e2e/smoke.mjs` drives the whole application in Chromium. It covers admin
setup, upload, groups, invites, search, programme prefetch, reading, drawing,
saving, reload and page turns:

```sh
(cd e2e && npm install && npx playwright install chromium)
./target/debug/musicus-server serve --data-dir /tmp/musicus-e2e \
    --web-dir target/dx/musicus-web/debug/web/public --listen 127.0.0.1:8099
node e2e/smoke.mjs <invite-url-from-the-log> some.pdf /tmp/shots
```

## Status of the first version

Implemented from the concept: everything above, including the core metadata
fields, concert programmes, file versions, thumbnails and page counts via
hayro, the backup command and optional built-in TLS.

Not yet implemented (in the concept, marked optional or later):

- optional grant kinds: per voice, per programme, with an expiry date;
- the "later" metadata fields: difficulty, duration, publisher, rights status, archive number and similar;
- OpenID Connect login and e-mail delivery of invite links;
- shared (read-only) annotation sets for a section, a "conductor push" of the next piece, half-page turns;
- per-user encryption of annotations at rest;
- MusicXML, and the offline app with encrypted local storage
  (the hash-addressed files and per-page revisions are already in place for it).

Deviations from the concept:

- pdf.js is shipped as its *legacy* build: the modern build needs JavaScript
  APIs that older tablets lack.
- Reading a page that has no annotations yet answers 204 (with
  `ETag: "0"`) instead of 404, so browsers log no errors on every page turn.
