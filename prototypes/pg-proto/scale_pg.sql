-- The "long-lived library" used by pg_scale (mirror of musicus-proto's scale_bench):
-- 5 000 users, 2 000 000 parts, 1 000 000 annotation pages of 6 KB (~6 GB).
\set users 5000
\set groups 200
\set instruments 150
\set scores 100000
\ir schema_pg.sql

-- Same synthetic titles and composers as scale_bench.rs.
UPDATE scores SET
    title = w[(id * 7) % 50 + 1] || ' ' || w[(id * 13 + 3) % 50 + 1] || ' ' || w[(id * 31 + 11) % 50 + 1],
    composer = c[(id * 17) % 40 + 1]
FROM (SELECT
    ARRAY['alpine','autumn','ballad','blue','bright','carnival','castle','celtic','dance','dawn',
          'dream','eagle','evening','festival','fire','forest','freedom','galop','glory','golden',
          'harbour','heroes','highland','hymn','journey','jubilee','king','legend','light','march',
          'meadow','moon','mountain','night','ocean','parade','polka','prelude','rhapsody','river',
          'royal','serenade','silver','spring','star','summer','thunder','valley','waltz','winter'] AS w,
    ARRAY['Fucik','Sousa','Strauss','Holst','Elgar','Grieg','Dvorak','Smetana','Brahms','Mozart',
          'Haydn','Bach','Handel','Verdi','Puccini','Rossini','Bizet','Ravel','Debussy','Satie',
          'Mahler','Bruckner','Wagner','Schubert','Schumann','Chopin','Liszt','Berlioz','Sibelius','Nielsen',
          'Janacek','Bartok','Kodaly','Ligeti','Gershwin','Bernstein','Copland','Williams','Morricone','Zimmer'] AS c) v;
ALTER TABLE scores ADD COLUMN tsv tsvector
    GENERATED ALWAYS AS (to_tsvector('simple', title || ' ' || coalesce(composer, ''))) STORED;
CREATE INDEX scores_tsv ON scores USING gin (tsv);

-- Payloads are zstd output in reality, i.e. incompressible: disable TOAST compression.
ALTER TABLE annotation_pages ALTER COLUMN data SET STORAGE EXTERNAL;
INSERT INTO annotation_sets (id, user_id, part_id, name)
    SELECT i, i % 5000, floor(random() * 2000000)::bigint, 'set ' || i FROM generate_series(0, 249999) i;
INSERT INTO annotation_pages (set_id, page, revision, data)
    SELECT i, p, 1, decode(repeat(md5((i * 4 + p)::text), 384), 'hex')
    FROM generate_series(0, 249999) i, generate_series(0, 3) p;
VACUUM ANALYZE;
