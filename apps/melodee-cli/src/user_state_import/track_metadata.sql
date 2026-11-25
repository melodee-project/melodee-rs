-- Match referenced legacy tracks to active catalog rows. Keep edition and version
-- text intact. Count both sides before choosing a target so duplicates stay visible.
WITH legacy AS MATERIALIZED (
    SELECT e."publicId" AS source,
           btrim(regexp_replace(lower(normalize(e.artist, NFKC)), '[[:space:]]+', ' ', 'g')) AS artist,
           btrim(regexp_replace(lower(normalize(e.release, NFKC)), '[[:space:]]+', ' ', 'g')) AS release,
           btrim(regexp_replace(lower(normalize(e.title, NFKC)), '[[:space:]]+', ' ', 'g')) AS title
    FROM jsonb_to_recordset($1::jsonb) AS e("publicId" uuid, artist text, release text, title text)
), wanted AS (
    SELECT artist, release, title, count(*) AS source_count
    FROM legacy WHERE artist <> '' AND release <> '' AND title <> ''
    GROUP BY artist, release, title
), editions AS MATERIALIZED (
    SELECT r.id, k.artist, k.release
    FROM (SELECT DISTINCT artist, release FROM wanted) k
    JOIN releases r
      ON btrim(regexp_replace(lower(normalize(r.display_artist, NFKC)), '[[:space:]]+', ' ', 'g')) = k.artist
     AND btrim(regexp_replace(lower(normalize(r.title, NFKC)), '[[:space:]]+', ' ', 'g')) = k.release
    WHERE r.status = 'active'
), matches AS (
    SELECT w.artist, w.release, w.title, w.source_count, count(*) AS target_count,
           CASE WHEN count(*) = 1 THEN min(t.public_api_id::text)::uuid END AS target,
           CASE WHEN count(*) = 1 THEN min(t.id::text)::uuid END AS internal_id
    FROM editions r JOIN tracks t ON t.release_id = r.id
    JOIN wanted w ON w.artist = r.artist AND w.release = r.release
     AND w.title = btrim(regexp_replace(lower(normalize(t.title, NFKC)), '[[:space:]]+', ' ', 'g'))
    GROUP BY w.artist, w.release, w.title, w.source_count
)
SELECT l.source, m.target, m.internal_id, m.source_count, m.target_count
FROM legacy l JOIN matches m USING (artist, release, title)
