-- Prototype schema used to test the access-control assumptions of the concept.
-- Not a production schema: no migrations, no audit columns, no tenant table.

PRAGMA foreign_keys = ON;

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    role          TEXT NOT NULL CHECK (role IN ('admin', 'librarian', 'member')),
    disabled      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE groups (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE                 -- e.g. "Trumpets"
);

CREATE TABLE group_members (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id  INTEGER NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    PRIMARY KEY (user_id, group_id)
) WITHOUT ROWID;

-- Controlled vocabulary instead of free text, so "Trp.", "Trumpet" and
-- "Trompete" cannot drift apart.
CREATE TABLE instruments (
    id     INTEGER PRIMARY KEY,
    name   TEXT NOT NULL UNIQUE,              -- e.g. "Trumpet"
    family TEXT NOT NULL                      -- e.g. "Brass"
);

-- Content-addressed blobs: the file on disk is named after its SHA-256.
CREATE TABLE files (
    id         INTEGER PRIMARY KEY,
    sha256     TEXT NOT NULL UNIQUE,
    mime       TEXT NOT NULL,                 -- application/pdf, later application/vnd.recordare.musicxml
    size_bytes INTEGER NOT NULL,
    page_count INTEGER
);

-- A score is the work/arrangement; a part is one sheet file of it.
CREATE TABLE scores (
    id       INTEGER PRIMARY KEY,
    title    TEXT NOT NULL,
    composer TEXT,
    arranger TEXT
);

CREATE TABLE parts (
    id       INTEGER PRIMARY KEY,
    score_id INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
    label    TEXT NOT NULL,                   -- e.g. "2nd Trumpet in Bb"
    file_id  INTEGER NOT NULL REFERENCES files(id)
);
CREATE INDEX parts_score ON parts(score_id);

-- A part can serve several instruments ("Trumpet 1 & 2", "Flute/Piccolo").
CREATE TABLE part_instruments (
    part_id       INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    instrument_id INTEGER NOT NULL REFERENCES instruments(id),
    PRIMARY KEY (instrument_id, part_id)
) WITHOUT ROWID;

-- The rule the user described: group "Trumpets" may read every part whose
-- instrument is "Trumpet".
CREATE TABLE group_instrument_grants (
    group_id      INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    instrument_id INTEGER NOT NULL REFERENCES instruments(id),
    PRIMARY KEY (group_id, instrument_id)
) WITHOUT ROWID;

-- Escape hatch: explicit grants of a whole score (conductor, librarian
-- helpers) or a single part (a substitute player) to a group.
CREATE TABLE group_score_grants (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    score_id INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, score_id)
) WITHOUT ROWID;

CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY,              -- SHA-256 of the opaque bearer token
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
) WITHOUT ROWID;

-- Annotation sets are private to (user, part). Each page layer is stored as
-- one compressed blob so that saving a page is a single-row upsert.
CREATE TABLE annotation_sets (
    id         INTEGER PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    part_id    INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,                 -- e.g. "Christmas concert 2026"
    UNIQUE (user_id, part_id, name)
);

CREATE TABLE annotation_pages (
    set_id   INTEGER NOT NULL REFERENCES annotation_sets(id) ON DELETE CASCADE,
    page     INTEGER NOT NULL,
    revision INTEGER NOT NULL,                -- optimistic concurrency / sync cursor
    data     BLOB NOT NULL,                   -- zstd(postcard(PageLayer))
    PRIMARY KEY (set_id, page)
) WITHOUT ROWID;
