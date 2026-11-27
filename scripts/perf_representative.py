#!/usr/bin/env python3
"""Run a command against an invocation-owned synthetic Rust catalog.

The supplied command receives MELODEE__DATABASE__URL. This fixture provides
catalog rows, one shared artwork file and media references, not physical audio
or a full workload.
Database creation and cleanup are delegated to test_database's ownership guard.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys
import tempfile
import time
import zlib

import test_database
from build_perf_server import command_output, source_fingerprints

ROOT = Path(__file__).resolve().parents[1]


def fixture_artwork() -> bytes:
    """A deterministic valid PNG shared by fixture references, generated in memory."""
    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload)))

    width = height = 16
    rows = b"".join(b"\0" + bytes((32, 96, 160)) * width for _ in range(height))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def install_fixture_artwork(root: Path) -> tuple[str, int]:
    """Populate only the invocation-owned asset root using the store's key layout."""
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_DIRECTORY"):
        raise OSError("fixture artwork requires descriptor-relative no-follow directory operations")
    content = fixture_artwork()
    digest = hashlib.sha256(content).hexdigest()
    key = f"{digest}-{len(content)}"
    directory_flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    root_fd = os.open(root, directory_flags)
    try:
        try:
            os.mkdir(digest[:2], mode=0o700, dir_fd=root_fd)
        except FileExistsError:
            pass
        directory_fd = os.open(digest[:2], directory_flags, dir_fd=root_fd)
        try:
            file_fd = os.open(key, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                              0o600, dir_fd=directory_fd)
            with os.fdopen(file_fd, "wb") as output:
                output.write(content)
        finally:
            os.close(directory_fd)
    finally:
        os.close(root_fd)
    return key, len(content)


def validate_migration_proof(binary: Path, proof: Path) -> str:
    if proof.is_symlink() or not proof.is_file() or proof.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid migration CLI provenance")
    record = json.loads(proof.read_text(encoding="utf-8"))
    if (not isinstance(record, dict) or record.get("target") != "migration-cli" or record.get("build_profile") != "release"
            or record.get("binary_sha256") != file_digest(binary)
            or record.get("commit") != command_output(["git", "rev-parse", "HEAD"], ROOT)
            or record.get("source_sha256") != source_fingerprints(ROOT)
            or record.get("provenance_producer_sha256") != file_digest(ROOT / "scripts/build_perf_cli.py")
            or record.get("fingerprint_helper_sha256") != file_digest(ROOT / "scripts/build_perf_server.py")):
        raise ValueError("migration CLI and source provenance differ")
    return file_digest(proof)


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


def quiesce_owned_fixture(admin: str, name: str, url: str, created: set[str],
                          cwd: Path, env: dict[str, str], timeout: int = 600) -> dict:
    """Vacuum only our fixture and observe a bounded quiet, stable size window."""
    if (name not in created or not test_database.OWNED.fullmatch(name)
            or test_database.database_url(admin, name) != url or not 3 <= timeout <= 600):
        raise ValueError("refusing quiescence outside the owned fixture")
    start = time.monotonic()
    deadline = start + timeout

    def query(statement: str) -> str:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("owned fixture quiescence timed out")
        result = subprocess.run(["psql", "--no-psqlrc", "--set=ON_ERROR_STOP=1", "--tuples-only",
                                 "--no-align", "--dbname", url, "--command", statement],
                                cwd=cwd, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, text=True, check=True, timeout=remaining)
        return result.stdout.strip()

    if query("SELECT current_database()") != name:
        raise ValueError("owned fixture database identity changed")
    query("VACUUM (ANALYZE)")
    statement = """SELECT json_build_object(
      'active', (SELECT count(*) FROM pg_stat_activity
                 WHERE datid=(SELECT oid FROM pg_database WHERE datname=current_database())
                   AND backend_type='autovacuum worker'),
      'database_size', pg_database_size(current_database()),
      'tables', (SELECT json_agg(json_build_array(name,bytes) ORDER BY bytes DESC,name)
                 FROM (SELECT relation.relname AS name,pg_total_relation_size(relation.oid)::bigint AS bytes
                       FROM pg_class relation JOIN pg_namespace namespace ON namespace.oid=relation.relnamespace
                       WHERE namespace.nspname=current_schema() AND relation.relkind='r'
                       ORDER BY bytes DESC,name LIMIT 14) sizes))::text"""
    previous = None
    stable = observations = 0
    while True:
        observation = json.loads(query(statement))
        if (not isinstance(observation, dict) or type(observation.get("active")) is not int
                or type(observation.get("database_size")) is not int or not isinstance(observation.get("tables"), list)):
            raise RuntimeError("invalid owned fixture quiescence observation")
        observations += 1
        physical = (observation["database_size"], observation["tables"])
        if observation["active"] == 0:
            stable = stable + 1 if physical == previous else 1
            previous = physical
        else:
            stable, previous = 0, None
        if stable >= 3:
            return {"mode": "vacuum_and_size_stability", "stable_observations": stable,
                    "observations": observations, "duration_seconds": round(time.monotonic() - start, 3)}
        if time.monotonic() + 1 >= deadline:
            raise TimeoutError("owned fixture quiescence timed out")
        time.sleep(1)


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
    artwork = fixture_artwork()
    artwork_key = f"{hashlib.sha256(artwork).hexdigest()}-{len(artwork)}"
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
INSERT INTO asset_files (asset_key, size_bytes) VALUES ('{artwork_key}', {len(artwork)});
INSERT INTO catalog_entity_images (entity_type, artist_id, image_mime_type, asset_key, uploaded_by, created_at, updated_at)
SELECT 'artist', id, 'image/png', '{artwork_key}', 'benchmark-fixture', now(), now()
FROM bench_artists WHERE n % 20 = 0;
INSERT INTO catalog_entity_images (entity_type, release_id, image_mime_type, asset_key, uploaded_by, created_at, updated_at)
SELECT 'release', id, 'image/png', '{artwork_key}', 'benchmark-fixture', now(), now()
FROM bench_releases WHERE n % 10 = 0;
INSERT INTO native_release_user_states (user_id, release_id, rating, created_at, updated_at)
SELECT u.id, r.id, (((r.n / 5) % 5) + 1)::smallint, now(), now()
FROM bench_releases r JOIN bench_users u ON u.n = (r.n % 100) + 1
WHERE r.n % 5 = 0;
INSERT INTO release_artists (id, release_id, artist_id, role, presentation_order, display_value, join_phrase, created_at)
SELECT uuidv7(), r.id, a.id, 'primary', 1, 'Artist ' || a.n, '', now()
FROM bench_releases r JOIN bench_artists a ON a.n = ((r.n - 1) % {artists} + 1);
-- Synthetic publication provenance exists only to exercise the actual ACL
-- predicates. The receipt payloads are placeholders, not import evidence.
CREATE TEMP TABLE bench_acl_groups AS
SELECT g n, uuidv7() id, uuidv7() public_id FROM generate_series(1, 2) g;
INSERT INTO native_library_access_groups (id, public_api_id, name, created_at, updated_at)
SELECT id, public_id, 'Benchmark group ' || n, now(), now() FROM bench_acl_groups;
INSERT INTO native_user_library_groups (user_id, group_id, created_at)
SELECT u.id, g.id, now() FROM bench_users u JOIN bench_acl_groups g
ON g.n = CASE WHEN u.n <= 50 THEN 1 ELSE 2 END;
CREATE TEMP TABLE bench_acl_libraries AS
SELECT g n, uuidv7() id FROM generate_series(1, 4) g;
INSERT INTO native_library_group_libraries (group_id, library_id, imported_at)
SELECT g.id, l.id, now() FROM bench_acl_libraries l JOIN bench_acl_groups g
ON g.n = CASE WHEN l.n <= 2 THEN 1 ELSE 2 END;
CREATE TEMP TABLE bench_acl_releases AS
SELECT r.n, r.id release_id, uuidv7() work_id, uuidv7() api_key, uuidv7() auth_id,
       uuidv7() manifest_id, uuidv7() import_id, l.id library_id,
       repeat(md5('plan:' || r.n), 2) plan_hash,
       repeat(md5('manifest:' || r.n), 2) manifest_hash,
       replace(r.id::text, '-', '') release_hex
FROM bench_releases r JOIN bench_acl_libraries l ON l.n = ((r.n / 5) % 4) + 1
WHERE r.n % 5 = 0;
INSERT INTO release_work_items
    (id, api_key, source_library_id, source_path, current_path, state, correlation_id,
     discovered_at, updated_at, created_at, completed_at)
SELECT work_id, api_key, library_id, 'benchmark/' || n, 'benchmark/' || n, 'indexed',
       uuidv7(), now(), now(), now(), now() FROM bench_acl_releases;
INSERT INTO release_work_item_manifests
    (work_item_id, schema_version, manifest_hash, manifest_payload, source_stable, generation_duration_ms, generated_at)
SELECT work_id, 3, manifest_hash, '{{}}'::jsonb, true, 0, now() FROM bench_acl_releases;
INSERT INTO release_publication_authorizations
    (authorization_id, work_item_id, release_id, authorization_name, authorization_version,
     validation_profile_name, validation_profile_version, validation_outcome,
     validation_payload, source_disposition, authorized_at)
SELECT auth_id, work_id, release_id, 'phase3.structural-publication', 1,
       'benchmark', 1, 'passed', '{{}}'::jsonb, 'retained', now() FROM bench_acl_releases;
INSERT INTO release_publication_plans
    (work_item_id, authorization_id, release_id, layout_version, input_manifest_hash,
     target_relative_path, manifest_relative_path, plan_hash, plan_payload, created_at)
SELECT work_id, auth_id, release_id, 1, manifest_hash,
       'releases/' || substr(release_hex, 1, 2) || '/' || substr(release_hex, 3, 2) || '/rel_' || release_hex,
       'releases/' || substr(release_hex, 1, 2) || '/' || substr(release_hex, 3, 2) || '/rel_' || release_hex || '/manifest.json',
       plan_hash, '{{}}'::jsonb, now() FROM bench_acl_releases;
INSERT INTO release_publication_receipts
    (work_item_id, release_id, plan_hash, sealed_manifest_hash, source_disposition, published_at)
SELECT work_id, release_id, plan_hash, manifest_hash, 'retained', now() FROM bench_acl_releases;
INSERT INTO catalog_imports
    (id, work_item_id, release_id, manifest_row_id, manifest_id, manifest_schema_version,
     semantic_manifest_hash, publication_plan_hash, sealed_manifest_hash, resolver_version, imported_at)
SELECT r.import_id, r.work_id, r.release_id, m.id, r.manifest_id, 3,
       r.manifest_hash, r.plan_hash, r.manifest_hash, 'benchmark', now()
FROM bench_acl_releases r JOIN release_work_item_manifests m ON m.work_item_id = r.work_id;
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
-- Alternate references exercise real catalog associations without requiring
-- a physical audio file for each database-only fixture row.
CREATE TEMP TABLE bench_alternates AS
SELECT track_id, uuidv7() asset_id, release_n, ordinal FROM bench_tracks
WHERE (release_n + ordinal) % 20 = 0;
INSERT INTO media_assets (id, canonical_relative_path, byte_hash, byte_size, mime_type, codec, sample_rate_hz, channels, duration_ms, created_at)
SELECT asset_id, 'bench/' || release_n || '/' || ordinal || '-alternate.mp3',
       repeat(md5('alternate:' || release_n || ':' || ordinal), 2),
       8000000, 'audio/mpeg', 'mp3', 44100, 2, 240000, now() FROM bench_alternates;
INSERT INTO track_media_assets (track_id, media_asset_id, purpose, ordinal, created_at)
SELECT track_id, asset_id, 'alternate', 1, now() FROM bench_alternates;
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
SELECT u.id, t.track_id, (((t.release_n + t.ordinal) / 10) % 5 + 1)::smallint, now(), now()
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
    parser.add_argument("--migration-cli-provenance", type=Path,
                        help="verify this CLI build record before and after migration")
    parser.add_argument("--seed-timeout", type=int,
                        help="limit fixture seeding in seconds; a timeout is reported as censored")
    parser.add_argument("--quiesce-fixture", action="store_true",
                        help="vacuum the owned fixture and wait for stable physical sizes before workload")
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
        migration_provenance_sha = (validate_migration_proof(migration_cli, args.migration_cli_provenance)
                                    if args.migration_cli_provenance else None)
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
                if migration_provenance_sha is not None:
                    env["MELODEE_PERF_MIGRATION_PROVENANCE_SHA256"] = migration_provenance_sha
                subprocess.run([str(migration_cli), "migrate"], cwd=asset_path, env=env,
                               check=True, stdout=subprocess.DEVNULL)
                install_fixture_artwork(asset_path)
                if (args.migration_cli_provenance is not None
                        and validate_migration_proof(migration_cli, args.migration_cli_provenance) != migration_provenance_sha):
                    raise RuntimeError("migration proof changed during migration")
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
                if args.quiesce_fixture:
                    try:
                        quiescence = quiesce_owned_fixture(admin, name, url, created, asset_path, env)
                    except (TimeoutError, subprocess.TimeoutExpired):
                        print(json.dumps({"status": "fixture_quiescence_timeout_censored"}), flush=True)
                        return 124
                    env["MELODEE_PERF_FIXTURE_QUIESCENCE"] = json.dumps(quiescence, separators=(",", ":"))
                    print(json.dumps({"fixture_quiescence": quiescence}), flush=True)
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
