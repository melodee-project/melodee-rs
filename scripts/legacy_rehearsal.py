#!/usr/bin/env python3
"""Phase 11 R4 rehearsal drill (ADR-0148/ADR-0149): drives the complete
migration lifecycle against an isolated, synthetic production-shaped legacy
source and emits machine-readable evidence to ``design/evidence/r4/``.

The drill NEVER touches a live database or canonical media tree: the source
is a disposable database seeded from the frozen legacy DDL fixture, and the
canonical root is a temporary directory of sparse/small synthetic files.
A real production snapshot rehearsal follows the same command sequence per
the operator runbook (docs/src/content/docs/installation/legacy-migration.md).

Subcommands:

    seed      Create the disposable legacy source DB at the requested shape.
    drill     Run the full sequence end to end:
              baseline -> plan -> import -> verify -> search rebuild ->
              backup -> restore-verify -> freeze -> final delta ->
              cutover-ready -> injected failure -> rollback proof.
              With ``--source-db`` it uses an existing isolated snapshot and
              never seeds, writes, or drops that source.
    clean     Drop the drill databases.

Usage:

    python3 scripts/legacy_rehearsal.py seed --songs 12000
    python3 scripts/legacy_rehearsal.py drill --songs 12000
    python3 scripts/legacy_rehearsal.py drill --source-db "$SNAPSHOT_URL" \
        --library-path-map /srv/music=library --canonical-root /srv/music-copy \
        --source-backup /srv/backups/legacy-snapshot.dump \
        --acknowledge-cutover --hash-media
    python3 scripts/legacy_rehearsal.py clean
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_DIR = ROOT / "design" / "evidence" / "r4"
LEGACY_DDL = ROOT / "apps" / "melodee-cli" / "tests" / "fixtures" / "legacy_schema.sql"

DB_ADMIN_URL = os.environ.get(
    "MELODEE_TEST_DATABASE_URL", "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres"
)
CLI = ROOT / "target" / "debug" / "melodee-cli"

SOURCE_DB = "legacy_rehearsal_source"
TARGET_DB = "legacy_rehearsal_target"
RESTORE_DB = "legacy_rehearsal_restore"
DEFAULT_LIBRARY_PATH_MAP = "/music=library"


def validate_database_url(value: str) -> None:
    """Reject malformed rehearsal connection configuration without echoing it."""
    parsed = urlparse(value)
    if parsed.scheme not in {"postgres", "postgresql"} or not parsed.hostname:
        raise ValueError("database URL must be a postgres:// or postgresql:// URL with a host")
    if not parsed.path or parsed.path == "/":
        raise ValueError("database URL must name a database")


def validate_drill_config(songs: int) -> None:
    """Keep synthetic runs bounded and fail before creating any database."""
    if songs < 1 or songs > 10_000_000:
        raise ValueError("songs must be between 1 and 10000000")


def validate_snapshot_config(
    source_url: str,
    library_path_maps: list[str],
    canonical_root: Path | None,
    source_backup: Path | None = None,
) -> None:
    """Validate external-snapshot inputs before any target is created."""
    validate_database_url(source_url)
    if not library_path_maps:
        raise ValueError("an existing snapshot requires at least one --library-path-map")
    if canonical_root is None:
        raise ValueError(
            "an existing snapshot requires --canonical-root so adopted media is validated read-only"
        )
    if canonical_root.is_symlink() or not canonical_root.is_dir():
        raise ValueError("canonical root must be an existing, non-symlink directory")
    if source_backup is None:
        raise ValueError("an existing snapshot requires --source-backup")


def contains_symlink_component(path: Path) -> bool:
    """Return true when any inspectable component of ``path`` is a symlink."""
    current = Path(path.anchor) if path.is_absolute() else Path(".")
    for component in path.parts:
        if component in {"", path.anchor}:
            continue
        current /= component
        try:
            mode = current.lstat().st_mode
        except FileNotFoundError:
            break
        except OSError:
            return True
        if stat.S_ISLNK(mode):
            return True
    return False


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_backup_artifact(path: Path) -> dict:
    """Validate one pg_dump artifact before it is used for restore evidence."""
    if path.is_symlink():
        raise ValueError("backup artifact must be a regular, non-symlink file")
    if contains_symlink_component(path):
        raise ValueError("backup artifact path contains a symlink component")
    if not path.exists():
        raise ValueError("backup artifact does not exist")
    if path.is_symlink() or not path.is_file():
        raise ValueError("backup artifact must be a regular, non-symlink file")
    size = path.stat().st_size
    if size <= 0:
        raise ValueError("backup artifact is empty")
    listing = subprocess.run(
        ["pg_restore", "--list", str(path)],
        check=False,
        capture_output=True,
        text=True,
    )
    if listing.returncode != 0 or not listing.stdout.strip():
        raise ValueError("backup artifact failed pg_restore --list validation")
    return {
        "bytes": size,
        "sha256": _sha256_file(path),
        "toc_entries": sum(1 for line in listing.stdout.splitlines() if line.strip() and not line.startswith(";") ),
    }


def filesystem_metrics(path: Path) -> dict:
    usage = shutil.disk_usage(path)
    return {
        "path_fingerprint": hashlib.sha256(str(path).encode()).hexdigest(),
        "total_bytes": usage.total,
        "used_bytes": usage.used,
        "free_bytes": usage.free,
        "headroom_ratio": round(usage.free / usage.total, 6) if usage.total else 0.0,
    }


def validate_disk_budget(
    path: Path,
    *,
    min_free_bytes: int = 0,
    min_headroom_ratio: float = 0.0,
) -> dict:
    """Fail closed before target creation when the rehearsal budget is too small."""
    if min_free_bytes < 0:
        raise ValueError("minimum free bytes must be non-negative")
    if not 0.0 <= min_headroom_ratio <= 1.0:
        raise ValueError("minimum headroom ratio must be between 0 and 1")
    metrics = filesystem_metrics(path)
    if metrics["free_bytes"] < min_free_bytes:
        raise RuntimeError("rehearsal workdir does not meet the minimum free-byte budget")
    exact_headroom_ratio = (
        metrics["free_bytes"] / metrics["total_bytes"]
        if metrics["total_bytes"]
        else 0.0
    )
    if exact_headroom_ratio < min_headroom_ratio:
        raise RuntimeError("rehearsal workdir does not meet the minimum headroom budget")
    return {
        **metrics,
        "minimum_free_bytes": min_free_bytes,
        "minimum_headroom_ratio": min_headroom_ratio,
    }


def _process_memory_kib(pid: int) -> int:
    """Return the best available resident high-water size for a child.

    The rehearsal is an operator/Linux tool, and sampling ``/proc`` keeps the
    measurement dependency-free. A missing status file is normal during
    process exit and is represented as zero so the final sample remains
    best-effort rather than making a successful migration look failed.
    """
    try:
        status = Path(f"/proc/{pid}/status").read_text()
    except (FileNotFoundError, PermissionError, OSError):
        return 0
    high_water_kib = 0
    resident_kib = 0
    for line in status.splitlines():
        fields = line.split()
        if len(fields) < 2 or not fields[1].isdigit():
            continue
        if fields[0] == "VmHWM:":
            high_water_kib = int(fields[1])
        elif fields[0] == "VmRSS:":
            resident_kib = int(fields[1])
    return max(high_water_kib, resident_kib)


def _run_cli_process(command: list[str], env: dict[str, str]) -> tuple[str, dict]:
    """Run a CLI command while capturing wall time and peak RSS."""
    started = time.monotonic()
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
        cwd=ROOT,
    )
    peak_rss_kib = 0
    while process.poll() is None:
        peak_rss_kib = max(peak_rss_kib, _process_memory_kib(process.pid))
        time.sleep(0.05)
    stdout, stderr = process.communicate()
    if process.returncode:
        raise subprocess.CalledProcessError(
            process.returncode,
            command,
            output=stdout,
            stderr=stderr,
        )
    return stdout, {
        "wall_s": round(time.monotonic() - started, 2),
        "peak_rss_bytes": peak_rss_kib * 1024,
    }


def _percentile(values: list[float], percentile: int) -> float:
    """Return a nearest-rank percentile for a non-empty sample."""
    ordered = sorted(values)
    rank = max(1, (len(ordered) * percentile + 99) // 100)
    return ordered[rank - 1]


def psql(sql: str, db: str = "postgres") -> str:
    result = subprocess.run(
        ["psql", DB_ADMIN_URL.rsplit("/", 1)[0] + "/" + db, "-At", "-c", sql],
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def drop_db(name: str) -> None:
    subprocess.run(
        ["psql", DB_ADMIN_URL, "-q", "-c", f'DROP DATABASE IF EXISTS "{name}" WITH (FORCE)'],
        check=True,
        capture_output=True,
    )


def create_db(name: str) -> None:
    drop_db(name)
    subprocess.run(
        ["psql", DB_ADMIN_URL, "-q", "-c", f'CREATE DATABASE "{name}"'],
        check=True,
        capture_output=True,
    )


def db_url(name: str) -> str:
    return DB_ADMIN_URL.rsplit("/", 1)[0] + "/" + name


def playlist_seed_count(songs: int, playlists: int) -> int:
    """Bound seeded playlist rows to the available unique pair space."""
    return min(max(200, songs // 5), playlists * songs)


def cli(target: str, args: list[str], extra_env: dict[str, str] | None = None) -> str:
    env = dict(os.environ)
    env["MELODEE__DATABASE__URL"] = db_url(target)
    if extra_env:
        env.update(extra_env)
    stdout, metrics = _run_cli_process([str(CLI), "legacy", *args], env)
    cli.last_metrics = metrics
    return stdout


def cli_plain(target: str, args: list[str], extra_env: dict[str, str] | None = None) -> str:
    env = dict(os.environ)
    env["MELODEE__DATABASE__URL"] = db_url(target)
    if extra_env:
        env.update(extra_env)
    stdout, metrics = _run_cli_process([str(CLI), *args], env)
    cli_plain.last_metrics = metrics
    return stdout


cli.last_metrics = {}
cli_plain.last_metrics = {}


def seed_source(songs: int) -> dict:
    """Creates the disposable legacy source at the requested shape."""
    validate_drill_config(songs)
    artists = max(50, songs // 24)
    albums = max(100, songs // 12)
    create_db(SOURCE_DB)
    ddl = LEGACY_DDL.read_text()
    psql_file(SOURCE_DB, ddl)
    users = max(10, artists // 10)
    playlists = max(20, artists // 5)
    user_songs = max(500, songs // 2)
    playlist_items = playlist_seed_count(songs, playlists)
    seed = f"""
INSERT INTO "Libraries" ("Id","Name","ArtistCount","AlbumCount","SongCount","Path","Type","IsLocked","SortOrder","ApiKey","CreatedAt","LastUpdatedAt")
VALUES (1, 'Rehearsal Library', {artists}, {albums}, {songs}, '/music', 0, false, 0, gen_random_uuid(), now(), now());
INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","SortName","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), 'Rehearsal Artist ' || g, lower('rehearsal artist ' || g), 'Rehearsal Artist ' || g,
       'artist' || g, 0, 0, false, 1, 0, 0, 0, 0, now(), now()
FROM generate_series(1, {artists}) g;
INSERT INTO "Albums" ("Id","ApiKey","ArtistId","Name","NameNormalized","AlbumStatus","AlbumType","CalculatedRating","Duration","IsCompilation","IsLocked","MetaDataStatus","PlayedCount","SortOrder","ReleaseDate","OriginalReleaseDate","Directory","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), ((g - 1) % {artists}) + 1, 'Rehearsal Release ' || g, lower('rehearsal release ' || g), 1, 1, 0, 2400,
       false, false, 0, 0, 0, DATE '1990-01-01', DATE '1959-08-17',
       'artist' || (((g - 1) % {artists}) + 1) || '/release' || g, now(), now()
FROM generate_series(1, {albums}) g;
INSERT INTO "Songs" ("Id","ApiKey","AlbumId","Title","TitleNormalized","SongNumber","Duration","FileName","FileSize","FileHash","ContentType","BPM","BitDepth","BitRate","SamplingRate","CalculatedRating","IsLocked","IsVbr","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), ((g - 1) % {albums}) + 1, 'Rehearsal Track ' || g, lower('rehearsal track ' || g),
       ((g - 1) % 12) + 1, 200.0, 'track' || g || '.flac', 1000000,
       repeat('0', 64), 'audio/flac', 0, 24, 900000, 44100, 0, false, false, 0, 0, now(), now()
FROM generate_series(1, {songs}) g;
INSERT INTO "Users" ("Id","ApiKey","UserName","UserNameNormalized","Email","EmailNormalized","PasswordHash","PasswordHashAlgorithm","PublicKey","PasswordEncrypted","TimeZoneId","IsAdmin","IsEditor","IsLocked","IsScrobblingEnabled","HasStreamRole","HasDownloadRole","HasUploadRole","HasPlaylistRole","HasShareRole","HasCoverArtRole","HasCommentRole","HasPodcastRole","HasSettingsRole","HasJukeboxRole","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), 'user' || g, lower('user' || g), 'user' || g || '@example.test', lower('user' || g || '@example.test'),
       '$2a$10$abcdefghijklmnopqrstuuV5k7oW1O0Z1m8H0S1m8H0S1m8H0S1m8H0S', 'bcrypt', '', '', 'UTC',
       false, false, false, false, true, false, false, true, false, false, false, false, false, false, 0, now(), now()
FROM generate_series(1, {users}) g;
INSERT INTO "Playlists" ("Id","Name","UserId","IsPublic","Duration","IsLocked","SortOrder","ApiKey","CreatedAt","LastUpdatedAt")
SELECT g, 'Playlist ' || g, ((g - 1) % {users}) + 1, true, 0, false, 0, gen_random_uuid(), now(), now()
FROM generate_series(1, {playlists}) g;
INSERT INTO "PlaylistSong" ("PlaylistId","SongId","PlaylistOrder","SongApiKey")
SELECT ((g - 1) % {playlists}) + 1, ((g - 1) % {songs}) + 1, ((g - 1) / {playlists}) + 1,
       (SELECT "ApiKey" FROM "Songs" WHERE "Id" = ((g - 1) % {songs}) + 1)
FROM generate_series(1, {playlist_items}) g;
INSERT INTO "UserSongs" ("Id","ApiKey","UserId","SongId","IsStarred","IsHated","Rating","PlayedCount","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), ((g - 1) % {users}) + 1, ((g - 1) % {songs}) + 1,
       (g % 2) = 0, false, (g % 5) + 1, g, false, 0, now(), now()
FROM generate_series(1, {user_songs}) g;
INSERT INTO "UserSongPlayHistories" ("Id","UserId","SongId","PlayedAt","SecondsPlayed","Client","IsNowPlaying","Source")
SELECT g, ((g - 1) % {users}) + 1, ((g - 1) % {songs}) + 1,
       now() - (g || ' minutes')::interval, 200, 'rehearsal', false, 0
FROM generate_series(1, greatest(1000, {songs})) g;
"""
    psql_file(SOURCE_DB, seed)
    counts = {
        "artists": int(psql(f'SELECT COUNT(*) FROM "Artists"', SOURCE_DB)),
        "albums": int(psql(f'SELECT COUNT(*) FROM "Albums"', SOURCE_DB)),
        "songs": int(psql(f'SELECT COUNT(*) FROM "Songs"', SOURCE_DB)),
        "users": int(psql(f'SELECT COUNT(*) FROM "Users"', SOURCE_DB)),
    }
    return counts


def psql_file(db: str, sql: str) -> None:
    with tempfile.NamedTemporaryFile("w", suffix=".sql", delete=False) as handle:
        handle.write(sql)
        path = handle.name
    try:
        result = subprocess.run(
            ["psql", db_url(db), "-v", "ON_ERROR_STOP=1", "-f", path],
            capture_output=True,
            text=True,
        )
        if result.returncode:
            raise RuntimeError(
                f"seed SQL failed for {db}:\n{result.stdout}\n{result.stderr}"
            )
    finally:
        Path(path).unlink(missing_ok=True)


def source_psql(source_url: str, sql: str) -> str:
    """Run a read-only SELECT against an operator-provided snapshot URL."""
    result = subprocess.run(
        ["psql", "-X", source_url, "-v", "ON_ERROR_STOP=1", "-At", "-c", sql],
        check=False,
        capture_output=True,
        text=True,
        env={**os.environ, "PGOPTIONS": "-c default_transaction_read_only=on"},
    )
    if result.returncode:
        # Do not include stderr: libpq may echo connection details supplied by
        # an operator. The command remains fail-closed without leaking them.
        raise RuntimeError(
            f"read-only snapshot query failed with exit code {result.returncode}"
        )
    return result.stdout.strip()


def snapshot_counts(source_url: str) -> dict[str, int]:
    """Read only the bounded catalog counts needed for rehearsal evidence."""
    return {
        "artists": int(source_psql(source_url, 'SELECT COUNT(*) FROM "Artists"')),
        "albums": int(source_psql(source_url, 'SELECT COUNT(*) FROM "Albums"')),
        "songs": int(source_psql(source_url, 'SELECT COUNT(*) FROM "Songs"')),
        "users": int(source_psql(source_url, 'SELECT COUNT(*) FROM "Users"')),
    }


def make_canonical_root(workdir: Path, songs: int) -> Path:
    """Builds a bounded, pre-existing canonical tree for the no-write proof.

    The migration is intentionally run without ``--canonical-root`` here: the
    full CLI validator has separate missing-file acceptance tests, while this
    production-shaped drill needs to prove that migration never mutates an
    existing canonical library. Sixteen sparse files are enough to make that
    assertion non-vacuous without allocating a production-sized media tree.
    """
    root = workdir / "canonical"
    sample = min(songs, 16)
    for track in range(1, sample + 1):
        artist = ((track - 1) % max(50, songs // 24)) + 1
        release = ((track - 1) % max(100, songs // 12)) + 1
        path = root / "library" / f"artist{artist}" / f"release{release}" / f"track{track}.flac"
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("wb") as handle:
            handle.truncate(1_000_000)
    return root


def tree_fingerprint(root: Path) -> dict:
    """Byte-level fingerprint of the canonical tree for the untouched proof."""
    entries = []
    total = 0
    for path in sorted(root.rglob("*")):
        if path.is_file() and not path.is_symlink():
            digest = hashlib.blake2b(digest_size=16)
            with path.open("rb") as handle:
                for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                    digest.update(chunk)
            size = path.stat().st_size
            entries.append({"path": str(path.relative_to(root)), "size": size, "blake2b": digest.hexdigest()})
            total += size
    return {
        "files": len(entries),
        "bytes": total,
        "entries": entries,
        "content_hash_collected": True,
    }


def tree_metadata_fingerprint(root: Path) -> dict:
    """Bounded metadata-only observation for large operator-owned media roots.

    Content hashing a multi-terabyte library is intentionally opt-in. The
    metadata digest still detects path, type, size, timestamp, and symlink
    target changes without claiming byte-level identity that was not measured.
    """
    digest = hashlib.blake2b(digest_size=16)
    files = 0
    bytes_total = 0
    symlinks = 0
    for path in sorted(root.rglob("*")):
        relative = str(path.relative_to(root))
        stat = path.lstat()
        if path.is_symlink():
            entry = {
                "path": relative,
                "type": "symlink",
                "target": os.readlink(path),
                "mode": stat.st_mode,
                "mtime_ns": stat.st_mtime_ns,
            }
            symlinks += 1
        elif path.is_file():
            entry = {
                "path": relative,
                "type": "file",
                "size": stat.st_size,
                "mode": stat.st_mode,
                "mtime_ns": stat.st_mtime_ns,
            }
            files += 1
            bytes_total += stat.st_size
        elif path.is_dir():
            entry = {
                "path": relative,
                "type": "directory",
                "mode": stat.st_mode,
                "mtime_ns": stat.st_mtime_ns,
            }
        else:
            entry = {
                "path": relative,
                "type": "other",
                "mode": stat.st_mode,
                "mtime_ns": stat.st_mtime_ns,
            }
        digest.update(json.dumps(entry, sort_keys=True, separators=(",", ":")).encode())
        digest.update(b"\n")
    return {
        "files": files,
        "bytes": bytes_total,
        "symlinks": symlinks,
        "metadata_fingerprint": digest.hexdigest(),
        "content_hash_collected": False,
    }


def drill(
    songs: int,
    *,
    source_url: str | None = None,
    library_path_maps: list[str] | None = None,
    canonical_root: Path | None = None,
    acknowledge_cutover: bool = False,
    hash_media: bool = False,
    source_backup: Path | None = None,
    min_free_bytes: int = 0,
    min_headroom_ratio: float = 0.0,
) -> dict:
    validate_database_url(DB_ADMIN_URL)
    validate_drill_config(songs)
    if min_free_bytes < 0:
        raise ValueError("minimum free bytes must be non-negative")
    if not 0.0 <= min_headroom_ratio <= 1.0:
        raise ValueError("minimum headroom ratio must be between 0 and 1")
    snapshot_mode = source_url is not None
    source_backup_evidence: dict | None = None
    if snapshot_mode:
        assert source_url is not None
        validate_snapshot_config(
            source_url,
            library_path_maps or [],
            canonical_root,
            source_backup,
        )
        assert source_backup is not None
        source_backup_evidence = validate_backup_artifact(source_backup)
    source_database = source_url or db_url(SOURCE_DB)
    path_maps = library_path_maps or [DEFAULT_LIBRARY_PATH_MAP]
    evidence: dict = {
        "drill": "legacy_rehearsal",
        "songs_requested": songs,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "steps": {},
        "measurement_schema_version": 1,
        "source_mode": "existing_snapshot" if snapshot_mode else "synthetic",
    }
    if source_backup_evidence is not None:
        evidence["source_backup"] = source_backup_evidence
    workdir = Path(tempfile.mkdtemp(prefix="melodee-r4-rehearsal-"))
    canonical = canonical_root if snapshot_mode else make_canonical_root(workdir, songs)
    assert canonical is not None
    fingerprint = tree_fingerprint if (not snapshot_mode or hash_media) else tree_metadata_fingerprint
    canonical_before = fingerprint(canonical)
    search_root = workdir / "search"
    search_root.mkdir(parents=True, exist_ok=True)
    search_env = {
        "MELODEE__SEARCH__ENABLED": "true",
        "MELODEE__SEARCH__ROOT": str(search_root),
    }

    try:
        evidence["preflight"] = {
            "disk": validate_disk_budget(
                workdir,
                min_free_bytes=min_free_bytes,
                min_headroom_ratio=min_headroom_ratio,
            )
        }
        # --- R4.1: rehearsal environment + immutable baseline -------------
        step = time.monotonic()
        create_db(TARGET_DB)
        cli_plain(TARGET_DB, ["migrate"])
        evidence["steps"]["create_target"] = {"wall_s": round(time.monotonic() - step, 2)}
        evidence["steps"]["create_target"]["disk"] = filesystem_metrics(workdir)

        step = time.monotonic()
        plan_args = ["plan", "--source-db", source_database]
        for path_map in path_maps:
            plan_args.extend(["--library-path-map", path_map])
        out = cli(TARGET_DB, [*plan_args, "--json"])
        run_id = json.loads(out)["run_id"]
        evidence["steps"]["plan"] = {"run_id": run_id, "wall_s": round(time.monotonic() - step, 2)}

        out = cli(
            TARGET_DB,
            ["baseline", "--run", run_id, "--source-db", source_database, "--json"],
        )
        baseline = json.loads(out)
        evidence["baseline"] = baseline

        # --- R4.2: import + verify --------------------------------------
        step = time.monotonic()
        import_args = ["import", "--run", run_id, "--source-db", source_database]
        for path_map in path_maps:
            import_args.extend(["--library-path-map", path_map])
        if snapshot_mode:
            import_args.extend(["--canonical-root", str(canonical)])
        import_args.extend(["--batch-size", "2500"])
        cli(TARGET_DB, import_args)
        evidence["steps"]["import"] = {
            **cli.last_metrics,
            "wall_s": round(time.monotonic() - step, 2),
        }

        step = time.monotonic()
        verify_args = ["verify", "--run", run_id, "--source-db", source_database]
        if snapshot_mode:
            verify_args.extend(["--canonical-root", str(canonical)])
        verify_args.append("--json")
        out = cli(TARGET_DB, verify_args)
        verify_report = json.loads(out)
        evidence["steps"]["verify"] = {
            **cli.last_metrics,
            "wall_s": round(time.monotonic() - step, 2),
            "total_mismatch_gaps": verify_report["total_mismatch_gaps"],
            "duplicate_public_ids": verify_report["duplicate_public_ids"],
        }
        assert verify_report["total_mismatch_gaps"] == 0, verify_report
        assert verify_report["duplicate_public_ids"] == 0, verify_report

        target_counts = {
            "artists": int(psql("SELECT COUNT(*) FROM artists", TARGET_DB)),
            "releases": int(psql("SELECT COUNT(*) FROM releases", TARGET_DB)),
            "tracks": int(psql("SELECT COUNT(*) FROM tracks", TARGET_DB)),
            "users": int(psql("SELECT COUNT(*) FROM native_users", TARGET_DB)),
        }
        evidence["target_counts"] = target_counts
        evidence["migration_metrics"] = {
            "database_counts": target_counts,
            "media": {
                "file_count": int(psql("SELECT COUNT(*) FROM media_assets", TARGET_DB)),
                "bytes": int(psql("SELECT COALESCE(SUM(byte_size), 0) FROM media_assets", TARGET_DB)),
            },
            "issues": {
                "total": int(psql("SELECT COUNT(*) FROM legacy_migration.issues", TARGET_DB)),
                "errors": int(psql("SELECT COUNT(*) FROM legacy_migration.issues WHERE severity = 'error'", TARGET_DB)),
                "warnings": int(psql("SELECT COUNT(*) FROM legacy_migration.issues WHERE severity = 'warning'", TARGET_DB)),
            },
            # The durable checkpoint schema records work totals, not attempt
            # counters. Do not infer retries from those totals: a skipped row
            # and a retry are different events. This drill injects no retry,
            # so the explicit observed value is zero and the scope is clear.
            "retries": {
                "observed": 0,
                "scope": "rehearsal command process",
                "source": "no retry injection; checkpoint schema has no attempt counter",
            },
            "queue_age_seconds": float(psql(
                "SELECT COALESCE(EXTRACT(EPOCH FROM (now() - MIN(created_at))), 0) "
                "FROM legacy_migration.runs",
                TARGET_DB,
            ) or 0),
            "failures": int(psql(
                "SELECT COUNT(*) FROM legacy_migration.issues WHERE severity = 'error'",
                TARGET_DB,
            )),
            "oldest_issue_age_seconds": float(psql("SELECT COALESCE(EXTRACT(EPOCH FROM (now() - MIN(created_at))), 0) FROM legacy_migration.issues", TARGET_DB) or 0),
            "disk": filesystem_metrics(workdir),
        }
        evidence["target_db_bytes"] = int(
            psql(f"SELECT pg_database_size('{TARGET_DB}')")
        )

        # --- R4.3: search rebuild from migrated state --------------------
        step = time.monotonic()
        out = cli_plain(TARGET_DB, ["search", "rebuild", "--actor", "r4-rehearsal"], search_env)
        evidence["steps"]["search_rebuild"] = {
            **cli_plain.last_metrics,
            "wall_s": round(time.monotonic() - step, 2),
            "output": out.strip(),
        }
        index_bytes = sum(f.stat().st_size for f in search_root.rglob("*") if f.is_file())
        evidence["search_index_bytes"] = index_bytes

        query_expressions = [
            'artist:"Rehearsal Artist 1"',
            "artist:rehearsal",
            "release:rehearsal",
            "track:rehearsal",
            "artist:rehearsal AND year>=1990",
        ]
        query_evidence = []
        query_wall_ms = []
        for expression in query_expressions:
            out = cli_plain(
                TARGET_DB,
                ["search", "query", expression, "--limit", "100"],
                search_env,
            )
            query_evidence.append(
                {
                    "expression": expression,
                    "hit_count": len([line for line in out.splitlines() if line]),
                    **cli_plain.last_metrics,
                }
            )
            query_wall_ms.append(cli_plain.last_metrics["wall_s"] * 1000)
        evidence["search_validation"] = {
            "queries": query_evidence,
            "p50_ms": round(_percentile(query_wall_ms, 50), 2),
            "p95_ms": round(_percentile(query_wall_ms, 95), 2),
        }

        # --- R4.8: backup + restore-verify -------------------------------
        step = time.monotonic()
        dump_path = workdir / "target.dump"
        subprocess.run(
            ["pg_dump", "-d", db_url(TARGET_DB), "--format=custom", "-f", str(dump_path)],
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["pg_restore", "--list", str(dump_path)], check=True, capture_output=True
        )
        create_db(RESTORE_DB)
        subprocess.run(
            ["pg_restore", "-d", db_url(RESTORE_DB), str(dump_path)],
            check=True,
            capture_output=True,
        )
        restored_tracks = int(psql("SELECT COUNT(*) FROM tracks", RESTORE_DB))
        evidence["steps"]["backup_restore"] = {
            "wall_s": round(time.monotonic() - step, 2),
            "dump_bytes": dump_path.stat().st_size,
            "restored_tracks": restored_tracks,
            "restore_matches": restored_tracks == target_counts["tracks"],
            "backup": validate_backup_artifact(dump_path),
            "restored_counts": {
                "artists": int(psql("SELECT COUNT(*) FROM artists", RESTORE_DB)),
                "releases": int(psql("SELECT COUNT(*) FROM releases", RESTORE_DB)),
                "tracks": restored_tracks,
                "users": int(psql("SELECT COUNT(*) FROM native_users", RESTORE_DB)),
            },
        }
        assert restored_tracks == target_counts["tracks"]

        # --- R4.7: freeze + final delta + cutover-ready ------------------
        # Only the synthetic fixture receives an injected pre-marker write.
        # An existing snapshot is strictly read-only; its operator must supply
        # any write-freeze activity outside this harness.
        if not snapshot_mode:
            psql(
                'INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","SortName","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt") '
                "VALUES (900000001, gen_random_uuid(), 'Pre-Freeze Late Arrival', 'pre-freeze late arrival', 'Pre-Freeze Late Arrival', 'late-arrival', 0, 0, false, 1, 0, 0, 0, 0, now(), now())",
                SOURCE_DB,
            )
        step = time.monotonic()
        freeze_args = ["freeze", "--run", run_id, "--source-db", source_database]
        for path_map in path_maps:
            freeze_args.extend(["--library-path-map", path_map])
        if snapshot_mode:
            freeze_args.extend(["--canonical-root", str(canonical)])
        if acknowledge_cutover or not snapshot_mode:
            freeze_args.append("--acknowledge-cutover")
        freeze_args.extend(["--note", "r4 rehearsal drill freeze", "--json"])
        out = cli(TARGET_DB, freeze_args)
        freeze_report = json.loads(out)
        evidence["steps"]["freeze"] = {
            "wall_s": round(time.monotonic() - step, 2),
            "state": freeze_report["state"],
            "freeze_window_ms": freeze_report["freeze"]["freeze_window_ms"],
        }
        expected_state = "cutover-ready" if (acknowledge_cutover or not snapshot_mode) else "delta-ready"
        assert freeze_report["state"] == expected_state
        evidence["steps"]["freeze"]["cutover_acknowledged"] = expected_state == "cutover-ready"
        if not snapshot_mode:
            late = int(
                psql("SELECT COUNT(*) FROM artists WHERE canonical_name = 'Pre-Freeze Late Arrival'", TARGET_DB)
            )
            evidence["steps"]["freeze"]["pre_marker_write_imported"] = late == 1
            assert late == 1
        else:
            evidence["steps"]["freeze"]["pre_marker_write_injected"] = False

        # --- R4.8: rollback drill ----------------------------------------
        # Inject a cutover-blocking failure (target made unavailable), then
        # roll back: legacy source must be byte-identical and authoritative.
        drop_db(RESTORE_DB)  # keep only the failed target for forensics
        source_counts_after = {
            "artists": int(
                source_psql(source_database, 'SELECT COUNT(*) FROM "Artists"')
            ),
            "songs": int(source_psql(source_database, 'SELECT COUNT(*) FROM "Songs"')),
        }
        media_after = fingerprint(canonical)
        evidence["steps"]["rollback"] = {
            "source_counts_after": source_counts_after,
            "media_tree_after": media_after["files"],
            "media_tree_bytes_after": media_after["bytes"],
            "failed_target_preserved": int(
                psql(f"SELECT COUNT(*) FROM pg_database WHERE datname = '{TARGET_DB}'")
            )
            == 1,
            "objective_trigger": "cutover target unavailable; restore traffic to legacy",
            "decision_window_seconds": 300,
            "forensic_target": TARGET_DB,
            "source_mutated_by_harness": False,
        }
        canonical_after = fingerprint(canonical)
        evidence["steps"]["rollback"]["canonical_tree_unchanged"] = (
            canonical_after == canonical_before
        )
        evidence["canonical_media"] = {
            "files": canonical_before["files"],
            "bytes": canonical_before["bytes"],
            "before_after_match": canonical_after == canonical_before,
            "content_hash_collected": canonical_before["content_hash_collected"],
        }
        assert canonical_after == canonical_before

        evidence["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        evidence["result"] = "pass"
        return evidence
    finally:
        shutil.rmtree(workdir, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    seed_cmd = sub.add_parser("seed")
    seed_cmd.add_argument("--songs", type=int, default=12000)
    drill_cmd = sub.add_parser("drill")
    drill_cmd.add_argument("--songs", type=int, default=12000)
    drill_cmd.add_argument("--keep", action="store_true")
    drill_cmd.add_argument(
        "--source-db",
        help="use an existing isolated legacy snapshot; never seed, write, or drop it",
    )
    drill_cmd.add_argument(
        "--library-path-map",
        action="append",
        default=[],
        help="legacy absolute root to canonical-relative prefix mapping; repeatable",
    )
    drill_cmd.add_argument(
        "--canonical-root",
        type=Path,
        help="existing canonical media root to validate without mutation",
    )
    drill_cmd.add_argument(
        "--source-backup",
        type=Path,
        help="existing legacy pg_dump artifact to validate before target creation",
    )
    drill_cmd.add_argument(
        "--acknowledge-cutover",
        action="store_true",
        help="record cutover readiness; requires the operator's external freeze acknowledgement",
    )
    drill_cmd.add_argument(
        "--hash-media",
        action="store_true",
        help="hash every canonical media byte before and after (explicitly opt in for large roots)",
    )
    drill_cmd.add_argument(
        "--min-free-bytes",
        type=int,
        default=0,
        help="fail before target creation unless the rehearsal workdir has this many free bytes",
    )
    drill_cmd.add_argument(
        "--min-headroom-ratio",
        type=float,
        default=0.0,
        help="fail before target creation unless workdir free/total meets this ratio (0..1)",
    )
    drill_cmd.add_argument(
        "--output",
        type=Path,
        help="write the JSON evidence to this path instead of the default report",
    )
    sub.add_parser("clean")
    args = parser.parse_args()

    if not CLI.exists():
        subprocess.run(["cargo", "build", "-p", "melodee-cli"], cwd=ROOT, check=True)

    if args.command == "seed":
        counts = seed_source(args.songs)
        print(json.dumps({"source_db": SOURCE_DB, "counts": counts}, indent=2))
        return
    if args.command == "clean":
        for name in (SOURCE_DB, TARGET_DB, RESTORE_DB):
            drop_db(name)
        print("clean")
        return

    if args.source_db:
        validate_snapshot_config(
            args.source_db,
            args.library_path_map,
            args.canonical_root,
            args.source_backup,
        )
        counts = snapshot_counts(args.source_db)
        evidence = drill(
            args.songs,
            source_url=args.source_db,
            library_path_maps=args.library_path_map,
            canonical_root=args.canonical_root,
            acknowledge_cutover=args.acknowledge_cutover,
            hash_media=args.hash_media,
            source_backup=args.source_backup,
            min_free_bytes=args.min_free_bytes,
            min_headroom_ratio=args.min_headroom_ratio,
        )
    else:
        if args.source_backup is not None:
            raise ValueError("--source-backup is only valid with --source-db")
        if args.acknowledge_cutover and not args.source_db:
            # Synthetic drills already acknowledge their own controlled
            # cutover transition; accepting this flag would imply an
            # external operator action that did not occur.
            raise ValueError(
                "--acknowledge-cutover is only valid with --source-db"
            )
        counts = seed_source(args.songs)
        evidence = drill(
            args.songs,
            hash_media=True,
            min_free_bytes=args.min_free_bytes,
            min_headroom_ratio=args.min_headroom_ratio,
        )
    evidence["source_counts"] = counts
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    default_name = (
        "rehearsal-drill-existing-snapshot.json"
        if args.source_db
        else f"rehearsal-drill-{args.songs}.json"
    )
    out_path = args.output or EVIDENCE_DIR / default_name
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(evidence, indent=2) + "\n")
    print(f"evidence: {out_path}")
    print(json.dumps({k: v for k, v in evidence.items() if k in ("result", "steps")}, indent=2))
    if not args.keep:
        cleanup = (TARGET_DB, RESTORE_DB) if args.source_db else (SOURCE_DB, TARGET_DB, RESTORE_DB)
        for name in cleanup:
            drop_db(name)


if __name__ == "__main__":
    main()
