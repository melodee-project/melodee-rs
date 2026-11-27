#!/usr/bin/env python3
"""Run a command against an invocation-owned synthetic Rust catalog.

The supplied command receives MELODEE__DATABASE__URL. This fixture provides
catalog rows and asset references, not physical media or a full workload.
Database creation and cleanup are delegated to test_database's ownership guard.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

import test_database

ROOT = Path(__file__).resolve().parents[1]


def fixture_environment(url: str, name: str, root: Path, admin: str,
                        migration_sha256: str, seed_timeout: int | None,
                        fixture_sha256: str, database_helper_sha256: str) -> dict[str, str]:
    """Only explicit benchmark and tool settings cross into fixture children."""
    allowed = {"PATH", "HOME", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "SYSTEMROOT", "WINDIR",
               "TEMP", "TMP", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN",
               "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "CARGO_BUILD_JOBS", "RUST_TEST_THREADS",
               "MELODEE_PERF_RECOMMENDATION_ROUNDS", "MELODEE_PERF_MERGE_TRACKS",
               "MELODEE_PERF_ISOLATION_CANARY"}
    env = {key: value for key, value in os.environ.items() if key in allowed}
    env.update({"MELODEE__DATABASE__URL": url, "MELODEE_PERF_OWNED_DATABASE": name,
                "MELODEE__STORAGE__ASSET_ROOT": str(root), "MELODEE_TEST_DATABASE_URL": admin,
                "MELODEE_PERF_FIXTURE_PID": str(os.getpid()),
                "MELODEE_PERF_MIGRATION_CLI_SHA256": migration_sha256,
                "MELODEE_PERF_FIXTURE_SHA256": fixture_sha256,
                "MELODEE_PERF_DATABASE_HELPER_SHA256": database_helper_sha256})
    if seed_timeout is not None:
        env["MELODEE_PERF_SEED_TIMEOUT_SECONDS"] = str(seed_timeout)
    return env


def file_digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def run_owned_child(command: list[str], cwd: Path, env: dict[str, str]) -> int:
    """Stop and reap only the child session created by this invocation."""
    child = subprocess.Popen(command, cwd=cwd, env=env, start_new_session=os.name == "posix")
    try:
        return child.wait()
    except BaseException:
        if child.poll() is None:
            try:
                if os.name == "posix":
                    if os.getpgid(child.pid) != child.pid or os.getsid(child.pid) != child.pid:
                        raise RuntimeError("owned child topology changed; refusing group signal")
                    os.killpg(child.pid, signal.SIGTERM)
                else:
                    child.terminate()
            except ProcessLookupError:
                pass
        try:
            child.wait(timeout=45)
        except subprocess.TimeoutExpired:
            if os.name == "posix":
                if os.getpgid(child.pid) != child.pid or os.getsid(child.pid) != child.pid:
                    raise RuntimeError("owned child topology changed; refusing group kill")
                os.killpg(child.pid, signal.SIGKILL)
            else:
                child.kill()
            child.wait(timeout=10)
        raise


def default_migration_cli() -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
    if not target.is_absolute():
        target = ROOT / target
    if os.environ.get("CARGO_BUILD_TARGET"):
        target /= os.environ["CARGO_BUILD_TARGET"]
    return target / "debug" / ("melodee-cli.exe" if os.name == "nt" else "melodee-cli")


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
INSERT INTO native_user_roles (user_id, role, created_at)
SELECT id, 'HasStreamRole', now() FROM bench_users;
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
CREATE TEMP TABLE bench_track_artists AS
SELECT uuidv7() credit_id, t.track_id, a.id artist_id, a.n artist_n
FROM bench_tracks t JOIN bench_artists a ON a.n = ((t.release_n - 1) % {artists} + 1);
INSERT INTO track_artists (id, track_id, artist_id, role, presentation_order, display_value, join_phrase, created_at)
SELECT credit_id, track_id, artist_id, 'primary', 1, 'Artist ' || artist_n, '', now()
FROM bench_track_artists;
INSERT INTO artist_track_associations (artist_id, track_id, association_type, participation_kind,
                                       source_key, confidence, is_performance, created_at)
SELECT artist_id, track_id, 'primary_artist', 'direct', credit_id, 6000, true, now()
FROM bench_track_artists;
COMMIT;
INSERT INTO release_catalog_aggregates (release_id, track_count, duration_ms, media_bytes, revision, updated_at)
SELECT id, {tracks_per_release}, {tracks_per_release} * 240000::bigint,
       {tracks_per_release} * 40000000::bigint, 1, now() FROM bench_releases;
INSERT INTO artist_catalog_aggregates (artist_id, release_count, billed_track_count,
    performance_track_count, all_credit_track_count, revision, updated_at)
SELECT a.id, count(r.id), count(r.id) * {tracks_per_release}, count(r.id) * {tracks_per_release},
       count(r.id) * {tracks_per_release}, 1, now()
FROM bench_artists a LEFT JOIN bench_releases r ON ((r.n - 1) % {artists} + 1) = a.n
GROUP BY a.id;
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
    parser.add_argument("--migration-cli", type=Path,
                        help="use this prebuilt CLI to migrate only the invocation-owned database")
    parser.add_argument("--seed-timeout", type=int,
                        help="limit fixture seeding in seconds; a timeout is reported as censored")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("provide a workload command after --")
    if args.seed_timeout is not None and args.seed_timeout < 1:
        parser.error("--seed-timeout must be positive")
    try:
        sql = fixture_sql(args.artists, args.releases, args.tracks_per_release)
        migration_cli = (args.migration_cli or default_migration_cli()).resolve(strict=True)
        if not migration_cli.is_file():
            raise ValueError("migration CLI must be a regular file")
        migration_sha256 = file_digest(migration_cli)
        fixture_sha256 = file_digest(Path(__file__).resolve())
        database_helper_sha256 = file_digest(ROOT / "scripts" / "test_database.py")
        admin = os.environ.get("MELODEE_TEST_DATABASE_URL", test_database.DEFAULT_ADMIN)
        created: set[str] = set()
        try:
            name = test_database.create(admin, created)
            url = test_database.database_url(admin, name)
            with tempfile.TemporaryDirectory(prefix="melodee-perf-assets-") as asset_root:
                asset_path = Path(asset_root).resolve()
                # dotenvy must not walk into an unrelated ancestor .env.
                (asset_path / ".env").touch(exist_ok=False)
                env = fixture_environment(url, name, asset_path, admin, migration_sha256, args.seed_timeout,
                                          fixture_sha256, database_helper_sha256)
                subprocess.run([str(migration_cli), "migrate"], cwd=asset_path, env=env,
                               check=True, stdout=subprocess.DEVNULL)
                start = time.monotonic()
                try:
                    subprocess.run(["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--dbname", url],
                                   input=sql, text=True, cwd=asset_path, env=env, check=True,
                                   stdout=subprocess.DEVNULL, timeout=args.seed_timeout)
                except subprocess.TimeoutExpired:
                    print(json.dumps({"status": "seed_timeout_censored", "seconds_at_least": args.seed_timeout,
                                      "artists_target": args.artists, "releases_target": args.releases,
                                      "tracks_target": args.releases * args.tracks_per_release}), flush=True)
                    return 124
                print(f"Seeded {args.artists} artists, {args.releases} releases, "
                      f"{args.releases * args.tracks_per_release} tracks in {time.monotonic() - start:.1f}s", flush=True)
                return run_owned_child(command, cwd=ROOT, env=env)
        finally:
            for name in list(created):
                test_database.drop_owned(admin, name, created)
    except (ValueError, OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Representative fixture failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    previous_term = signal.getsignal(signal.SIGTERM)
    previous_int = signal.getsignal(signal.SIGINT)
    def interrupt(signum, _frame):
        # Ensure repeated signals do not interrupt registered-child/database cleanup.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if signum == signal.SIGINT:
            raise KeyboardInterrupt
        raise InterruptedError("termination requested")
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("Fixture interrupted; owned cleanup attempted", file=sys.stderr)
        raise SystemExit(130) from None
    finally:
        signal.signal(signal.SIGTERM, previous_term)
        signal.signal(signal.SIGINT, previous_int)
