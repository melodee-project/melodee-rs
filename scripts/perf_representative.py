#!/usr/bin/env python3
"""Run a command against an invocation-owned synthetic Rust catalog.

The supplied command receives MELODEE__DATABASE__URL. This fixture provides
catalog rows and asset references, not physical media or a full workload.
Database creation and cleanup are delegated to test_database's ownership guard.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import subprocess
import sys
import time

import test_database

ROOT = Path(__file__).resolve().parents[1]


def fixture_sql(artists: int, releases: int, tracks_per_release: int) -> str:
    if min(artists, releases, tracks_per_release) < 1:
        raise ValueError("fixture sizes must be positive")
    return f"""
CREATE TEMP TABLE bench_artists AS
SELECT g n, uuidv7() id, uuidv7() public_id FROM generate_series(1, {artists}) g;
ALTER TABLE bench_artists ADD PRIMARY KEY (n);
INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name, artist_type, status, created_at, updated_at)
SELECT id, public_id, 'Artist ' || n, 'artist ' || n, 'Artist ' || n, 'group', 'verified', now(), now() FROM bench_artists;
-- Synthetic credentials are deliberately public and must only be used in this
-- invocation-owned disposable database. The password is benchmark-only-password.
CREATE TEMP TABLE bench_users AS
SELECT g n, uuidv7() id, uuidv7() public_id FROM generate_series(1, 100) g;
ALTER TABLE bench_users ADD PRIMARY KEY (n);
INSERT INTO native_users (id, public_api_id, username, normalized_username, email,
    normalized_email, password_hash, password_hash_algorithm, stream_signing_key,
    is_admin, created_at, updated_at)
SELECT id, public_id, 'benchmark-' || n, 'BENCHMARK-' || n,
       'benchmark-' || n || '@example.test', 'BENCHMARK-' || n || '@EXAMPLE.TEST',
       '$2b$10$ufNxeMgsL349N6cqv.5Pk.G00Cy5wHKSQM.8VQrEp7psXWcpQ1NpK',
       'bcrypt', 'disposable-benchmark-key', n = 1, now(), now() FROM bench_users;
INSERT INTO native_artist_user_states (user_id, artist_id, rating, starred, created_at, updated_at)
SELECT u.id, a.id, CASE WHEN (a.n + u.n) % 9 = 0 THEN 0 ELSE ((a.n + u.n) % 5 + 1)::smallint END,
       (a.n + u.n) % 7 = 0, now(), now()
FROM bench_users u CROSS JOIN bench_artists a
WHERE (a.n * 17 + u.n * 29) % 100 < 5;
CREATE TEMP TABLE bench_releases AS
SELECT g n, uuidv7() id, uuidv7() public_id FROM generate_series(1, {releases}) g;
ALTER TABLE bench_releases ADD PRIMARY KEY (n);
INSERT INTO releases (id, public_api_id, title, normalized_title, sort_title, display_artist, status, created_at, updated_at)
SELECT id, public_id, 'Release ' || n, 'release ' || n, 'Release ' || n,
       'Artist ' || ((n - 1) % {artists} + 1), 'active', now(), now() FROM bench_releases;
INSERT INTO native_release_user_states (user_id, release_id, rating, created_at, updated_at)
SELECT u.id, r.id, ((r.n % 5) + 1)::smallint, now(), now()
FROM bench_releases r JOIN bench_users u ON u.n = (r.n % 100) + 1
WHERE r.n % 5 = 0;
INSERT INTO release_artists (id, release_id, artist_id, role, presentation_order, display_value, join_phrase, created_at)
SELECT uuidv7(), r.id, a.id, 'primary', 1, 'Artist ' || a.n, '', now()
FROM bench_releases r JOIN bench_artists a ON a.n = ((r.n - 1) % {artists} + 1);
CREATE TEMP TABLE bench_tracks AS
SELECT uuidv7() track_id, uuidv7() public_id, uuidv7() audio_id, uuidv7() asset_id,
       r.id release_id, r.n release_n, t ordinal
FROM bench_releases r CROSS JOIN generate_series(1, {tracks_per_release}) t;
BEGIN;
INSERT INTO audio_identities (id, status, created_at, updated_at)
SELECT audio_id, 'verified', now(), now() FROM bench_tracks;
INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size, mime_type, codec, sample_rate_hz, channels, duration_ms, created_at)
SELECT asset_id, 'bench/' || release_n || '/' || ordinal || '.flac',
       repeat(md5(release_n || ':' || ordinal), 2), 40000000, 'audio/flac', 'flac', 44100, 2, 240000, now() FROM bench_tracks;
INSERT INTO tracks (id, public_api_id, release_id, audio_identity_id, playable_media_asset_id, ordinal, title, normalized_title, sort_title, display_artist, disc_number, track_number, duration_ms, created_at, updated_at)
SELECT track_id, public_id, release_id, audio_id, asset_id, ordinal,
       'Track ' || ordinal, 'track ' || ordinal, 'Track ' || ordinal,
       'Artist ' || ((release_n - 1) % {artists} + 1), 1, ordinal, 240000, now(), now() FROM bench_tracks;
INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal, created_at)
SELECT track_id, asset_id, 'playable', 1, now() FROM bench_tracks;
COMMIT;
INSERT INTO native_track_user_states (user_id, track_id, rating, created_at, updated_at)
SELECT u.id, t.track_id, ((t.release_n + t.ordinal) % 5 + 1)::smallint, now(), now()
FROM bench_tracks t JOIN bench_users u ON u.n = ((t.release_n + t.ordinal) % 100) + 1
WHERE (t.release_n + t.ordinal) % 10 = 0;
INSERT INTO audio_identity_evidence (id, source_audio_identity_id, track_id, evidence_kind, polarity, confidence, source, payload, created_at)
SELECT uuidv7(), audio_id, track_id, 'acoustic_fingerprint', 'supports', 9000,
       'synthetic-benchmark', jsonb_build_object('fingerprint_digest', repeat(md5(release_n || ':' || ordinal), 2)), now()
FROM bench_tracks;
INSERT INTO catalog_genre_terms (track_id, release_id, surface, display_value, source_order)
SELECT t.track_id, t.release_id, surface.name,
       CASE WHEN t.release_n % 10 < 7 THEN 'Rock'
            WHEN t.release_n % 10 < 9 THEN 'Jazz' ELSE 'Other ' || (t.release_n % 50) END, 1
FROM bench_tracks t CROSS JOIN (VALUES ('native'), ('opensubsonic')) surface(name);
ANALYZE;
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artists", type=int, default=35000)
    parser.add_argument("--releases", type=int, default=50000)
    parser.add_argument("--tracks-per-release", type=int, default=12)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("provide a workload command after --")
    try:
        sql = fixture_sql(args.artists, args.releases, args.tracks_per_release)
        admin = os.environ.get("MELODEE_TEST_DATABASE_URL", test_database.DEFAULT_ADMIN)
        created: set[str] = set()
        try:
            name = test_database.create(admin, created)
            test_database.migrate(admin, name)
            url = test_database.database_url(admin, name)
            start = time.monotonic()
            subprocess.run(["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", url],
                           input=sql, text=True, cwd=ROOT, check=True, stdout=subprocess.DEVNULL)
            print(f"Seeded {args.artists} artists, {args.releases} releases, "
                  f"{args.releases * args.tracks_per_release} tracks in {time.monotonic() - start:.1f}s", flush=True)
            env = os.environ.copy()
            env["MELODEE__DATABASE__URL"] = url
            # The benchmark must also opt in to the exact database created by
            # this invocation before issuing mutating production operations.
            env["MELODEE_PERF_OWNED_DATABASE"] = name
            return subprocess.run(command, cwd=ROOT, env=env, check=False).returncode
        finally:
            for name in list(created):
                test_database.drop_owned(admin, name, created)
    except (ValueError, OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Representative fixture failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
