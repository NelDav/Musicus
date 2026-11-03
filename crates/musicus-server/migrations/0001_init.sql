-- Musicus schema, version 1.
-- Times are Unix seconds. Sheet files live in the blob store on disk; this
-- database only references them by SHA-256.

CREATE TABLE users (
    id            INTEGER PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT NOT NULL,
    -- NULL until the user has accepted their invite.
    password_hash TEXT,
    role          TEXT NOT NULL CHECK (role IN ('member', 'librarian', 'admin')),
    disabled      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL
);

-- One-time links to set a password (first login or reset).
CREATE TABLE invites (
    token_hash BLOB PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE INDEX invites_user ON invites(user_id);

-- Opaque session tokens; only their SHA-256 is stored.
CREATE TABLE sessions (
    id           INTEGER PRIMARY KEY,
    token_hash   BLOB NOT NULL UNIQUE,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    expires_at   INTEGER NOT NULL,
    user_agent   TEXT
);
CREATE INDEX sessions_user ON sessions(user_id);

CREATE TABLE groups (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE group_members (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id  INTEGER NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    PRIMARY KEY (user_id, group_id)
) WITHOUT ROWID;
CREATE INDEX group_members_group ON group_members(group_id);

-- Controlled vocabulary, so "Trp.", "Trumpet" and "Trompete" cannot drift apart.
CREATE TABLE instruments (
    id     INTEGER PRIMARY KEY,
    name   TEXT NOT NULL UNIQUE COLLATE NOCASE,
    family TEXT
);

-- Immutable, content-addressed blobs (blobs/ab/abcdef...).
CREATE TABLE files (
    id               INTEGER PRIMARY KEY,
    sha256           TEXT NOT NULL UNIQUE,
    mime             TEXT NOT NULL,
    size_bytes       INTEGER NOT NULL,
    page_count       INTEGER,
    thumbnail_sha256 TEXT,
    created_at       INTEGER NOT NULL
);

-- A score is the work or arrangement.
CREATE TABLE scores (
    id         INTEGER PRIMARY KEY,
    title      TEXT NOT NULL,
    subtitle   TEXT,
    composer   TEXT,
    arranger   TEXT,
    genre      TEXT,
    -- JSON array of strings.
    tags       TEXT NOT NULL DEFAULT '[]',
    notes      TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Full-text search over the score metadata. remove_diacritics makes
-- "Fucik" find "Fučík".
CREATE VIRTUAL TABLE score_fts USING fts5(
    title, subtitle, composer, arranger, genre, tags,
    content = 'scores', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER scores_fts_insert AFTER INSERT ON scores BEGIN
    INSERT INTO score_fts (rowid, title, subtitle, composer, arranger, genre, tags)
    VALUES (new.id, new.title, new.subtitle, new.composer, new.arranger, new.genre, new.tags);
END;
CREATE TRIGGER scores_fts_delete AFTER DELETE ON scores BEGIN
    INSERT INTO score_fts (score_fts, rowid, title, subtitle, composer, arranger, genre, tags)
    VALUES ('delete', old.id, old.title, old.subtitle, old.composer, old.arranger, old.genre, old.tags);
END;
CREATE TRIGGER scores_fts_update AFTER UPDATE ON scores BEGIN
    INSERT INTO score_fts (score_fts, rowid, title, subtitle, composer, arranger, genre, tags)
    VALUES ('delete', old.id, old.title, old.subtitle, old.composer, old.arranger, old.genre, old.tags);
    INSERT INTO score_fts (rowid, title, subtitle, composer, arranger, genre, tags)
    VALUES (new.id, new.title, new.subtitle, new.composer, new.arranger, new.genre, new.tags);
END;

-- A part is one sheet of a score, e.g. "2nd Trumpet in Bb".
CREATE TABLE parts (
    id            INTEGER PRIMARY KEY,
    score_id      INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
    label         TEXT NOT NULL,
    voice         INTEGER,
    transposition TEXT,
    notes         TEXT,
    created_at    INTEGER NOT NULL
);
CREATE INDEX parts_score ON parts(score_id);

-- A part can serve several instruments ("Flute / Piccolo").
CREATE TABLE part_instruments (
    part_id       INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    instrument_id INTEGER NOT NULL REFERENCES instruments(id) ON DELETE RESTRICT,
    PRIMARY KEY (instrument_id, part_id)
) WITHOUT ROWID;
CREATE INDEX part_instruments_part ON part_instruments(part_id);

-- File versions of a part. The highest version is the current one; older
-- versions stay readable for annotation sets drawn on them.
CREATE TABLE part_files (
    part_id     INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    version     INTEGER NOT NULL,
    file_id     INTEGER NOT NULL REFERENCES files(id),
    uploaded_at INTEGER NOT NULL,
    PRIMARY KEY (part_id, version)
) WITHOUT ROWID;
CREATE INDEX part_files_file ON part_files(file_id);

-- Group "Trumpets" may read every part whose instrument is "Trumpet".
CREATE TABLE group_instrument_grants (
    group_id      INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    instrument_id INTEGER NOT NULL REFERENCES instruments(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, instrument_id)
) WITHOUT ROWID;
CREATE INDEX group_instrument_grants_instrument ON group_instrument_grants(instrument_id);

-- A group may read every part of a score (conductor, section leaders).
CREATE TABLE group_score_grants (
    group_id INTEGER NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    score_id INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, score_id)
) WITHOUT ROWID;
CREATE INDEX group_score_grants_score ON group_score_grants(score_id);

CREATE TABLE programmes (
    id         INTEGER PRIMARY KEY,
    title      TEXT NOT NULL,
    date       TEXT,
    notes      TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE programme_items (
    programme_id INTEGER NOT NULL REFERENCES programmes(id) ON DELETE CASCADE,
    position     INTEGER NOT NULL,
    score_id     INTEGER NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
    PRIMARY KEY (programme_id, position)
) WITHOUT ROWID;

-- Private annotation sets: one user, one part, one file version.
CREATE TABLE annotation_sets (
    id           INTEGER PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    part_id      INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    file_id      INTEGER NOT NULL REFERENCES files(id),
    name         TEXT NOT NULL,
    programme_id INTEGER REFERENCES programmes(id) ON DELETE SET NULL,
    is_default   INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    UNIQUE (user_id, part_id, name)
);

-- One layer per page: zstd(version byte + postcard(PageLayer)).
CREATE TABLE annotation_pages (
    set_id     INTEGER NOT NULL REFERENCES annotation_sets(id) ON DELETE CASCADE,
    page       INTEGER NOT NULL,
    revision   INTEGER NOT NULL,
    data       BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (set_id, page)
) WITHOUT ROWID;
