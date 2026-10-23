-- PostgreSQL variant of schema.sql, plus a synthetic seed. The default size is
-- the "large library" (1 000 users, 200 000 parts); scale_pg.sql overrides the
-- psql variables below. Prototype only.

\if :{?users} \else \set users 1000 \endif
\if :{?groups} \else \set groups 60 \endif
\if :{?instruments} \else \set instruments 80 \endif
\if :{?scores} \else \set scores 10000 \endif

DROP SCHEMA IF EXISTS public CASCADE;
CREATE SCHEMA public;

CREATE TABLE users (
    id            bigint PRIMARY KEY,
    username      text NOT NULL UNIQUE,
    display_name  text NOT NULL,
    password_hash text NOT NULL,
    role          text NOT NULL CHECK (role IN ('admin', 'librarian', 'member')),
    disabled      boolean NOT NULL DEFAULT false
);
CREATE TABLE groups (id bigint PRIMARY KEY, name text NOT NULL UNIQUE);
CREATE TABLE group_members (
    group_id bigint NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id  bigint NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    PRIMARY KEY (user_id, group_id)
);
CREATE TABLE instruments (id bigint PRIMARY KEY, name text NOT NULL UNIQUE, family text NOT NULL);
CREATE TABLE files (
    id bigint PRIMARY KEY, sha256 text NOT NULL UNIQUE, mime text NOT NULL,
    size_bytes bigint NOT NULL, page_count int
);
CREATE TABLE scores (id bigint PRIMARY KEY, title text NOT NULL, composer text, arranger text);
CREATE TABLE parts (
    id bigint PRIMARY KEY,
    score_id bigint NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
    label text NOT NULL,
    file_id bigint NOT NULL REFERENCES files(id)
);
CREATE INDEX parts_score ON parts(score_id);
CREATE TABLE part_instruments (
    part_id bigint NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    instrument_id bigint NOT NULL REFERENCES instruments(id),
    PRIMARY KEY (instrument_id, part_id)
);
CREATE TABLE group_instrument_grants (
    group_id bigint NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    instrument_id bigint NOT NULL REFERENCES instruments(id),
    PRIMARY KEY (group_id, instrument_id)
);
CREATE TABLE group_score_grants (
    group_id bigint NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    score_id bigint NOT NULL REFERENCES scores(id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, score_id)
);
CREATE TABLE annotation_sets (
    id bigserial PRIMARY KEY,
    user_id bigint NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    part_id bigint NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    name text NOT NULL,
    UNIQUE (user_id, part_id, name)
);
CREATE TABLE annotation_pages (
    set_id bigint NOT NULL REFERENCES annotation_sets(id) ON DELETE CASCADE,
    page int NOT NULL,
    revision bigint NOT NULL,
    data bytea NOT NULL,
    PRIMARY KEY (set_id, page)
);

SELECT setseed(0.42);
INSERT INTO instruments SELECT i, 'Instrument ' || i, 'Family ' || (i % 5) FROM generate_series(0, :instruments - 1) i;
INSERT INTO groups SELECT g, 'Group ' || g FROM generate_series(0, :groups - 1) g;
INSERT INTO group_instrument_grants
    SELECT DISTINCT g, floor(random() * :instruments)::bigint FROM generate_series(0, :groups - 1) g, generate_series(1, 2) k;
INSERT INTO users SELECT u, 'user' || u, 'user' || u, 'x', 'member' FROM generate_series(0, :users - 1) u;
INSERT INTO group_members
    SELECT DISTINCT floor(random() * :groups)::bigint, u FROM generate_series(0, :users - 1) u, generate_series(1, 2) k;
INSERT INTO scores SELECT s, 'Score ' || s FROM generate_series(0, :scores - 1) s;
INSERT INTO files SELECT p, lpad(to_hex(p), 64, '0'), 'application/pdf', 3000000, 4 FROM generate_series(0, :scores * 20 - 1) p;
INSERT INTO parts SELECT p, p / 20, 'Part ' || (p % 20), p FROM generate_series(0, :scores * 20 - 1) p;
INSERT INTO part_instruments
    SELECT DISTINCT p, floor(random() * :instruments)::bigint FROM generate_series(0, :scores * 20 - 1) p, generate_series(1, 2) k;
INSERT INTO group_score_grants
    SELECT DISTINCT floor(random() * :groups)::bigint, s FROM generate_series(0, :scores - 1) s WHERE random() < 0.02;
-- Everyone may read part 0 (used by the concurrent test).
INSERT INTO groups VALUES (1000, 'Everyone');
INSERT INTO group_members SELECT 1000, id FROM users;
INSERT INTO group_score_grants VALUES (1000, 0);
ANALYZE;
