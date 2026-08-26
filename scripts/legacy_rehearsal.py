#!/usr/bin/env python3
"""Phase 11 R4 rehearsal drill (ADR-0148/ADR-0149): drives the complete
migration lifecycle against an isolated, synthetic production-shaped legacy
source and emits machine-readable evidence to ``target/verification/migration/``.

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
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
EVIDENCE_DIR = ROOT / "target" / "verification" / "migration"
LEGACY_DDL = ROOT / "apps" / "melodee-cli" / "tests" / "fixtures" / "legacy_schema.sql"

DB_ADMIN_URL = os.environ.get(
    "MELODEE_TEST_DATABASE_URL", "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres"
)
CLI = ROOT / "target" / "debug" / "melodee-cli"

SOURCE_DB = "legacy_rehearsal_source"
TARGET_DB = "legacy_rehearsal_target"
RESTORE_DB = "legacy_rehearsal_restore"
DEFAULT_LIBRARY_PATH_MAP = "/music=library"
BASELINE_ARTIFACT = "melodee.legacy-baseline"
BASELINE_SCHEMA_VERSION = 1
FROZEN_SOURCE_TABLE_COUNT = 52
HEX_DIGEST_RE = re.compile(r"^[0-9a-f]{64}$")
SEARCH_TOKEN_RE = re.compile(r"[^\W_]+", re.UNICODE)


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


def validate_library_path_maps(values: list[str]) -> None:
    """Validate path mappings before the rehearsal creates any target state.

    This intentionally mirrors ``parse_library_path_maps`` in the Rust CLI.
    The CLI remains the authority for migration semantics; this early check
    only prevents an operator typo from creating a target before the CLI can
    reject the same input.
    """
    seen_legacy_roots: set[str] = set()
    for value in values:
        if "=" not in value:
            raise ValueError(f"invalid --library-path-map {value!r}: expected A=B")
        legacy, raw_prefix = value.split("=", 1)
        if "\x00" in legacy or "\x00" in raw_prefix:
            raise ValueError(f"invalid --library-path-map {value!r}: NUL is not allowed")
        legacy = legacy.strip().rstrip("/")
        raw_prefix = raw_prefix.strip()
        if raw_prefix.startswith("/") or "\\" in raw_prefix:
            raise ValueError(
                f"invalid --library-path-map {value!r}: relative prefix must not be absolute or contain backslashes"
            )
        prefix = raw_prefix.strip("/")
        if not legacy or not legacy.startswith("/"):
            raise ValueError(
                f"invalid --library-path-map {value!r}: legacy path must be absolute"
            )
        if any(component == ".." for component in legacy.split("/")):
            raise ValueError(
                f"invalid --library-path-map {value!r}: legacy path may not contain '..'"
            )
        if prefix and any(component in {"", ".", ".."} for component in prefix.split("/")):
            raise ValueError(
                f"invalid --library-path-map {value!r}: relative prefix has unsafe components"
            )
        if legacy in seen_legacy_roots:
            raise ValueError(
                f"invalid --library-path-map {value!r}: duplicate legacy root"
            )
        seen_legacy_roots.add(legacy)


def validate_baseline_artifact(value: object) -> dict:
    """Validate the stable, redacted baseline artifact emitted by the CLI."""
    if not isinstance(value, dict):
        raise ValueError("baseline artifact must be a JSON object")
    if value.get("artifact") != BASELINE_ARTIFACT:
        raise ValueError("baseline artifact name is unsupported")
    if value.get("schema_version") != BASELINE_SCHEMA_VERSION:
        raise ValueError("baseline artifact schema version is unsupported")
    if not isinstance(value.get("captured_at"), str) or not value["captured_at"]:
        raise ValueError("baseline artifact captured_at is missing")
    if value.get("table_count") != FROZEN_SOURCE_TABLE_COUNT:
        raise ValueError("baseline artifact does not cover the frozen source table set")

    schema_fingerprint = value.get("schema_fingerprint")
    source_identity = value.get("source_identity")
    if not isinstance(schema_fingerprint, str) or not schema_fingerprint:
        raise ValueError("baseline schema fingerprint is missing")
    if (
        not isinstance(source_identity, str)
        or not source_identity.startswith("pg-source-sha256:")
        or ":schema:" not in source_identity
        or source_identity.split(":schema:", 1)[1] != schema_fingerprint
    ):
        raise ValueError("baseline source identity is malformed")
    snapshot_marker = value.get("snapshot_marker")
    if (
        not isinstance(snapshot_marker, str)
        or not snapshot_marker.startswith("legacy-marker-v2:")
        or not HEX_DIGEST_RE.fullmatch(snapshot_marker.removeprefix("legacy-marker-v2:"))
    ):
        raise ValueError("baseline snapshot marker is malformed")

    read_only = value.get("source_read_only")
    if not isinstance(read_only, dict) or any(
        not isinstance(read_only.get(key), bool)
        for key in ("transaction_read_only", "default_transaction_read_only", "in_recovery")
    ):
        raise ValueError("baseline source read-only facts are malformed")
    if not read_only["transaction_read_only"] or not read_only["default_transaction_read_only"]:
        raise ValueError("baseline source is not transaction read-only")

    domains = value.get("domains")
    if not isinstance(domains, list) or not domains:
        raise ValueError("baseline domains are missing")
    domain_names: set[str] = set()
    for domain in domains:
        if not isinstance(domain, dict) or not isinstance(domain.get("domain"), str):
            raise ValueError("baseline domain entry is malformed")
        name = domain["domain"]
        if not name or name in domain_names:
            raise ValueError("baseline domain names must be non-empty and unique")
        domain_names.add(name)
        rows = domain.get("rows")
        if not isinstance(rows, int) or isinstance(rows, bool) or rows < 0:
            raise ValueError("baseline domain row count is malformed")
        for bound in ("min_pk", "max_pk"):
            if domain.get(bound) is not None and not isinstance(domain[bound], int):
                raise ValueError("baseline domain primary-key bounds are malformed")

    anomalies = value.get("anomalies")
    if not isinstance(anomalies, list):
        raise ValueError("baseline anomalies are malformed")
    for anomaly in anomalies:
        if (
            not isinstance(anomaly, dict)
            or not isinstance(anomaly.get("code"), str)
            or not anomaly["code"]
            or not isinstance(anomaly.get("count"), int)
            or isinstance(anomaly["count"], bool)
            or anomaly["count"] < 0
        ):
            raise ValueError("baseline anomaly entry is malformed")

    media = value.get("media")
    if (
        not isinstance(media, dict)
        or not isinstance(media.get("file_count"), int)
        or isinstance(media["file_count"], bool)
        or media["file_count"] < 0
        or not isinstance(media.get("total_bytes"), int)
        or isinstance(media["total_bytes"], bool)
        or media["total_bytes"] < 0
    ):
        raise ValueError("baseline media totals are malformed")

    configuration = value.get("configuration_fingerprint")
    if (
        not isinstance(configuration, dict)
        or configuration.get("algorithm") != "sha256-v1"
        or not isinstance(configuration.get("settings_keys"), list)
        or not isinstance(configuration.get("settings"), list)
        or not isinstance(configuration.get("library_root_count"), int)
        or isinstance(configuration["library_root_count"], bool)
        or configuration["library_root_count"] < 0
    ):
        raise ValueError("baseline configuration fingerprint is malformed")
    settings_keys = configuration["settings_keys"]
    if any(not isinstance(key, str) or not key for key in settings_keys):
        raise ValueError("baseline configuration setting keys are malformed")
    settings = configuration["settings"]
    if [entry.get("key") for entry in settings if isinstance(entry, dict)] != settings_keys:
        raise ValueError("baseline configuration setting keys do not match fingerprints")
    for setting in settings:
        if not isinstance(setting, dict) or not isinstance(setting.get("key"), str):
            raise ValueError("baseline configuration setting is malformed")
        if setting.get("redacted") is True:
            if set(setting) != {"key", "redacted"}:
                raise ValueError("redacted baseline setting contains extra material")
        elif set(setting) == {"key", "sha256"} and isinstance(setting["sha256"], str) and HEX_DIGEST_RE.fullmatch(setting["sha256"]):
            continue
        else:
            raise ValueError("baseline configuration setting is not redacted or hashed")
    if len(settings_keys) != len(set(settings_keys)):
        raise ValueError("baseline configuration setting keys are not unique")
    return value


def validate_search_status(value: object, expected_counts: dict[str, int]) -> dict:
    """Validate the CLI diagnostic after a rebuild without accepting ambiguity."""
    if not isinstance(value, dict):
        raise ValueError("search status must be a JSON object")
    projection = value.get("projection")
    if not isinstance(projection, dict):
        raise ValueError("search status projection is missing")
    generation = value.get("generation")
    if not isinstance(generation, dict):
        raise ValueError("search status has no active generation")
    if value.get("generation_error_code") is not None or value.get("generation_error") is not None:
        raise ValueError("search status reports a generation error")
    if projection.get("latest_rebuild_status") != "activated":
        raise ValueError("search status does not report an activated rebuild")
    for key in ("artists", "releases", "tracks"):
        actual = generation.get(key)
        expected = expected_counts.get(key)
        if not isinstance(actual, int) or isinstance(actual, bool) or actual != expected:
            raise ValueError(f"search status {key} count does not match migrated authority")
    total = generation.get("total")
    expected_total = sum(expected_counts.get(key, 0) for key in ("artists", "releases", "tracks"))
    if not isinstance(total, int) or isinstance(total, bool) or total != expected_total:
        raise ValueError("search status total does not match migrated authority")
    if not isinstance(generation.get("schema_version"), int) or generation["schema_version"] < 1:
        raise ValueError("search status schema version is malformed")
    return value


def _mql_quote(value: str) -> str:
    """Quote a catalog value for a bounded MQL field expression."""
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def _fingerprint_search_value(value: str) -> str:
    """Represent a snapshot-derived search value without retaining its text."""
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()


def _snapshot_artist_names(db: str) -> list[str]:
    """Read only searchable Artist names from the migrated PostgreSQL authority."""
    raw = psql(
        "SELECT COALESCE(json_agg(canonical_name ORDER BY canonical_name, public_api_id)::text, '[]') "
        "FROM artists WHERE status IS DISTINCT FROM 'merged'",
        db,
    )
    try:
        values = json.loads(raw)
    except json.JSONDecodeError as error:
        raise RuntimeError("migrated Artist authority returned malformed search data") from error
    if (
        not isinstance(values, list)
        or any(not isinstance(value, str) or not value.strip() for value in values)
    ):
        raise RuntimeError("migrated Artist authority returned invalid search values")
    return [value.strip() for value in values]


def build_search_probe(snapshot_mode: bool, db: str = TARGET_DB) -> dict:
    """Build search probes from synthetic fixtures or migrated authority.

    Snapshot mode must never silently turn an unexercised query class into a
    passing result. It therefore requires representative Unicode, punctuation,
    and repeated-token data and keeps the values out of emitted evidence.
    """
    if not snapshot_mode:
        return {
            "prefix": "reh",
            "prefix_expected": "Rehearsal Artist ",
            "queries": [
                ("exact", 'artist:"Rehearsal Artist 3"', 1, "Rehearsal Artist 3"),
                ("normalized", "artist:REHEARSAL", 1, "Rehearsal"),
                ("unicode", "artist:東京事変", 1, "東京事変"),
                ("punctuation", 'artist:"AC/DC Rehearsal"', 1, "AC/DC Rehearsal"),
                ("broad", "rehearsal", 1, "Rehearsal"),
                ("low_selectivity", "artist:Rehearsal", 10, "Rehearsal"),
            ],
        }

    names = _snapshot_artist_names(db)
    safe_names = [
        name
        for name in names
        if not any(ord(character) < 32 for character in name)
    ]
    if not safe_names:
        raise RuntimeError("migrated catalog has no usable Artist names for search validation")

    unicode_name = next(
        (name for name in safe_names if any(ord(character) > 127 for character in name)),
        None,
    )
    if unicode_name is None:
        raise RuntimeError("migrated catalog has no Unicode Artist value for search validation")
    punctuation_name = next(
        (
            name
            for name in safe_names
            if any(
                not character.isalnum() and not character.isspace()
                for character in name
            )
        ),
        None,
    )
    if punctuation_name is None:
        raise RuntimeError(
            "migrated catalog has no punctuation Artist value for search validation"
        )

    token_names: dict[str, set[str]] = {}
    for name in safe_names:
        for token in SEARCH_TOKEN_RE.findall(name):
            folded = token.casefold()
            if len(folded) >= 2:
                token_names.setdefault(folded, set()).add(name)
    repeated = sorted(
        ((len(found), token, sorted(found)) for token, found in token_names.items()),
        reverse=True,
    )
    if not repeated or repeated[0][0] < 2:
        raise RuntimeError(
            "migrated catalog has no repeated Artist token for low-selectivity search validation"
        )
    _, common_token, common_names = repeated[0]
    normalized_name = next(
        (name for name in safe_names if any(character.isalpha() for character in name)),
        safe_names[0],
    )
    normalized_token = next(
        (
            token
            for token in SEARCH_TOKEN_RE.findall(normalized_name)
            if len(token) >= 2
        ),
        normalized_name,
    )
    prefix_name = safe_names[0]
    prefix = prefix_name[: min(3, len(prefix_name))]
    if not prefix:
        raise RuntimeError("migrated catalog has an empty Artist autocomplete prefix")

    return {
        "prefix": prefix,
        "prefix_expected": prefix_name,
        "queries": [
            ("exact", f"artist:{_mql_quote(safe_names[0])}", 1, safe_names[0]),
            (
                "normalized",
                f"artist:{_mql_quote(normalized_token.upper())}",
                1,
                normalized_name,
            ),
            ("unicode", f"artist:{_mql_quote(unicode_name)}", 1, unicode_name),
            (
                "punctuation",
                f"artist:{_mql_quote(punctuation_name)}",
                1,
                punctuation_name,
            ),
            ("broad", common_token, 1, common_names[0]),
            (
                "low_selectivity",
                f"artist:{common_token}",
                min(10, len(common_names)),
                common_names[0],
            ),
        ],
    }


def validate_snapshot_config(
    source_url: str,
    library_path_maps: list[str],
    canonical_root: Path | None,
    source_backup: Path | None = None,
) -> None:
    """Validate external-snapshot inputs before any target is created."""
    validate_database_url(source_url)
    validate_library_path_maps(library_path_maps)
    if not library_path_maps:
        raise ValueError("an existing snapshot requires at least one --library-path-map")
    if canonical_root is None:
        raise ValueError(
            "an existing snapshot requires --canonical-root so adopted media is validated read-only"
        )
    if contains_symlink_component(canonical_root):
        raise ValueError("canonical root path contains a symlink component")
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


def open_regular_file_nofollow(path: Path) -> tuple[int, os.stat_result]:
    """Open an operator-supplied artifact through held directory handles.

    The descriptor is deliberately kept open until ``pg_restore`` has
    inspected the artifact.  A pathname check followed by a pathname passed
    to a subprocess would allow a concurrent replacement to change which
    bytes are validated and restored.
    """
    if os.name != "posix":
        raise ValueError("race-resistant backup validation is unsupported on this platform")
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    directory = getattr(os, "O_DIRECTORY", 0)
    cloexec = getattr(os, "O_CLOEXEC", 0)
    nonblock = getattr(os, "O_NONBLOCK", 0)
    if not nofollow or not directory:
        raise ValueError("backup validation requires no-follow directory support")
    absolute = path if path.is_absolute() else Path.cwd() / path
    components = absolute.parts
    if not components or any(component == ".." for component in components):
        raise ValueError("backup artifact path must not contain traversal")
    flags = os.O_RDONLY | cloexec | nofollow | directory
    current = os.open(absolute.anchor or ".", flags)
    try:
        remaining = components[1:] if absolute.anchor else components
        for index, component in enumerate(remaining):
            is_leaf = index == len(remaining) - 1
            next_flags = os.O_RDONLY | cloexec | nofollow
            if is_leaf:
                next_flags |= nonblock
            else:
                next_flags |= directory
            next_fd = os.open(component, next_flags, dir_fd=current)
            os.close(current)
            current = next_fd
        metadata = os.fstat(current)
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("backup artifact must be a regular, non-symlink file")
        return current, metadata
    except (OSError, ValueError) as error:
        os.close(current)
        if isinstance(error, ValueError):
            raise
        raise ValueError("backup artifact could not be opened safely") from error


def validate_backup_artifact(path: Path) -> dict:
    """Validate one pg_dump artifact before it is used for restore evidence."""
    if contains_symlink_component(path):
        raise ValueError("backup artifact path contains a symlink component")
    try:
        descriptor, metadata = open_regular_file_nofollow(path)
    except ValueError as error:
        if not path.exists() and not path.is_symlink():
            raise ValueError("backup artifact does not exist") from error
        raise
    try:
        if metadata.st_size <= 0:
            raise ValueError("backup artifact is empty")
        digest = hashlib.sha256()
        with os.fdopen(os.dup(descriptor), "rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        os.lseek(descriptor, 0, os.SEEK_SET)
        listing = subprocess.run(
            ["pg_restore", "--list", f"/proc/self/fd/{descriptor}"],
            check=False,
            capture_output=True,
            text=True,
            pass_fds=(descriptor,),
        )
        if listing.returncode != 0 or not listing.stdout.strip():
            raise ValueError("backup artifact failed pg_restore --list validation")
        return {
            "bytes": metadata.st_size,
            "sha256": digest.hexdigest(),
            "toc_entries": sum(
                1
                for line in listing.stdout.splitlines()
                if line.strip() and not line.startswith(";")
            ),
        }
    finally:
        os.close(descriptor)


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


def playlist_seed_pair(sequence: int, songs: int, playlists: int) -> tuple[int, int]:
    """Return a unique playlist/song pair for a one-based seed sequence."""
    zero_based = sequence - 1
    playlist_id = (zero_based % playlists) + 1
    song_id = ((zero_based // playlists) % songs) + 1
    return playlist_id, song_id


def user_song_seed_count(songs: int, users: int) -> int:
    """Bound seeded user-song states to the available unique pair space."""
    return min(max(500, songs // 2), users * songs)


def user_song_seed_pair(sequence: int, songs: int, users: int) -> tuple[int, int]:
    """Return a unique user/song pair for a one-based seed sequence."""
    zero_based = sequence - 1
    user_id = ((zero_based // songs) % users) + 1
    song_id = (zero_based % songs) + 1
    return user_id, song_id


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


def cli_top(target: str, args: list[str], extra_env: dict[str, str] | None = None) -> str:
    """Run a top-level CLI command against the disposable target database."""
    env = dict(os.environ)
    env["MELODEE__DATABASE__URL"] = db_url(target)
    if extra_env:
        env.update(extra_env)
    stdout, metrics = _run_cli_process([str(CLI), *args], env)
    cli_top.last_metrics = metrics
    return stdout


cli.last_metrics = {}
cli_plain.last_metrics = {}
cli_top.last_metrics = {}


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
    user_songs = user_song_seed_count(songs, users)
    playlist_items = playlist_seed_count(songs, playlists)
    seed = f"""
INSERT INTO "Libraries" ("Id","Name","ArtistCount","AlbumCount","SongCount","Path","Type","IsLocked","SortOrder","ApiKey","CreatedAt","LastUpdatedAt")
VALUES (1, 'Rehearsal Library', {artists}, {albums}, {songs}, '/music', 0, false, 0, gen_random_uuid(), now(), now());
INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","SortName","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(),
       CASE WHEN g = 1 THEN '東京事変' WHEN g = 2 THEN 'AC/DC Rehearsal' ELSE 'Rehearsal Artist ' || g END,
       lower(CASE WHEN g = 1 THEN '東京事変' WHEN g = 2 THEN 'AC/DC Rehearsal' ELSE 'rehearsal artist ' || g END),
       CASE WHEN g = 1 THEN '東京事変' WHEN g = 2 THEN 'AC/DC Rehearsal' ELSE 'Rehearsal Artist ' || g END,
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
SELECT ((g - 1) % {playlists}) + 1, (((g - 1) / {playlists}) % {songs}) + 1, ((g - 1) / {playlists}) + 1,
       (SELECT "ApiKey" FROM "Songs" WHERE "Id" = (((g - 1) / {playlists}) % {songs}) + 1)
FROM generate_series(1, {playlist_items}) g;
INSERT INTO "UserSongs" ("Id","ApiKey","UserId","SongId","IsStarred","IsHated","Rating","PlayedCount","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
SELECT g, gen_random_uuid(), (((g - 1) / {songs}) % {users}) + 1, ((g - 1) % {songs}) + 1,
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
        # The legacy importer composes Artist.Directory and Album.Directory;
        # the synthetic Album.Directory already begins with artistN.
        path = root / "library" / f"artist{artist}" / f"artist{artist}" / f"release{release}" / f"track{track}.flac"
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
    validate_library_path_maps(path_maps)
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
        inventory = json.loads(
            cli(TARGET_DB, ["inventory", "--source-db", source_database, "--json"])
        )
        evidence["steps"]["inventory"] = {
            "wall_s": round(time.monotonic() - step, 2),
            "table_count": inventory["schema"]["table_count"],
            "media_file_count": inventory["media"]["file_count"],
            "media_total_bytes": inventory["media"]["total_bytes"],
        }

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
        baseline = validate_baseline_artifact(json.loads(out))
        evidence["baseline"] = baseline
        # `legacy inventory` deliberately contains only inventory facts. The
        # source session boundary is emitted by `legacy baseline`; retain the
        # validated fact alongside the inventory step without duplicating an
        # unsupported inventory field.
        evidence["steps"]["inventory"]["source_read_only"] = baseline["source_read_only"]

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

        # The validator is intentionally exercised against the same bounded
        # canonical tree. Synthetic drills expect the remaining generated
        # files to be reported missing; snapshot drills preserve those counts
        # without ever mutating the operator-owned tree.
        step = time.monotonic()
        validation = json.loads(
            cli_top(
                TARGET_DB,
                [
                    "legacy",
                    "validate",
                    "--run",
                    run_id,
                    "--canonical-root",
                    str(canonical),
                    "--json",
                ],
            )
        )
        evidence["steps"]["media_validate"] = {
            "wall_s": round(time.monotonic() - step, 2),
            "checked": validation["checked"],
            "valid": validation["valid"],
            "missing": validation["missing"],
            "size_mismatch": validation["size_mismatch"],
            "unsafe": validation["unsafe"],
            "repair_rows": len(validation["repair_plan"]),
            "read_only": True,
        }

        target_counts = {
            "artists": int(psql("SELECT COUNT(*) FROM artists", TARGET_DB)),
            "releases": int(psql("SELECT COUNT(*) FROM releases", TARGET_DB)),
            "tracks": int(psql("SELECT COUNT(*) FROM tracks", TARGET_DB)),
            "users": int(psql("SELECT COUNT(*) FROM native_users", TARGET_DB)),
        }
        target_media = {
            "file_count": int(psql("SELECT COUNT(*) FROM media_assets", TARGET_DB)),
            "bytes": int(psql("SELECT COALESCE(SUM(byte_size), 0) FROM media_assets", TARGET_DB)),
        }
        evidence["target_counts"] = target_counts
        evidence["migration_metrics"] = {
            "database_counts": target_counts,
            "media": target_media,
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

        status_out = cli_plain(TARGET_DB, ["search", "status"], search_env)
        search_status = validate_search_status(json.loads(status_out), target_counts)
        evidence["search_status"] = {
            "projection": search_status["projection"],
            "generation": search_status["generation"],
            "generation_error_code": search_status["generation_error_code"],
            "generation_error": search_status["generation_error"],
        }

        search_probe = build_search_probe(snapshot_mode)
        autocomplete_out = cli_plain(
            TARGET_DB,
            [
                "search",
                "autocomplete",
                "artist",
                search_probe["prefix"],
                "--limit",
                "20",
            ],
            search_env,
        )
        autocomplete_values = [line for line in autocomplete_out.splitlines() if line]
        minimum_autocomplete_values = 1
        if len(autocomplete_values) < minimum_autocomplete_values:
            raise RuntimeError("search prefix query returned no suggestions")
        if len(autocomplete_values) > 20:
            raise RuntimeError("search prefix query exceeded its result limit")
        if not any(
            search_probe["prefix_expected"] in value for value in autocomplete_values
        ):
            raise RuntimeError("search prefix query omitted its authoritative representative")
        if snapshot_mode:
            evidence["search_prefix"] = {
                "kind": "artist",
                "prefix_sha256": _fingerprint_search_value(search_probe["prefix"]),
                "limit": 20,
                "value_fingerprints": [
                    _fingerprint_search_value(value) for value in autocomplete_values
                ],
                "minimum_values": minimum_autocomplete_values,
            }
        else:
            evidence["search_prefix"] = {
                "kind": "artist",
                "prefix": search_probe["prefix"],
                "limit": 20,
                "values": autocomplete_values,
                "minimum_values": minimum_autocomplete_values,
            }

        query_cases = search_probe["queries"]
        query_evidence = []
        query_wall_ms = []
        for query_kind, expression, synthetic_minimum, expected_display in query_cases:
            out = cli_plain(
                TARGET_DB,
                ["search", "query", expression, "--limit", "100"],
                search_env,
            )
            hits = [line for line in out.splitlines() if line]
            minimum_hits = synthetic_minimum if not snapshot_mode else 0
            if len(hits) < minimum_hits:
                raise RuntimeError(
                    f"search {query_kind} query returned {len(hits)} hits; expected at least {minimum_hits}"
                )
            if len(hits) > 100:
                raise RuntimeError(f"search {query_kind} query exceeded its result limit")
            if not any(expected_display in hit for hit in hits):
                raise RuntimeError(
                    f"search {query_kind} query omitted its authoritative representative"
                )
            query_record = {
                "kind": query_kind,
                "hit_count": len(hits),
                "minimum_hits": minimum_hits,
                "limit": 100,
                **cli_plain.last_metrics,
            }
            if snapshot_mode:
                query_record.update(
                    {
                        "expression_sha256": _fingerprint_search_value(expression),
                        "expected_display_sha256": _fingerprint_search_value(expected_display),
                    }
                )
            else:
                query_record.update(
                    {
                        "expression": expression,
                        "expected_display": expected_display,
                    }
                )
            query_evidence.append(query_record)
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
        backup_evidence = validate_backup_artifact(dump_path)
        create_db(RESTORE_DB)
        subprocess.run(
            ["pg_restore", "-d", db_url(RESTORE_DB), str(dump_path)],
            check=True,
            capture_output=True,
        )
        restored_counts = {
            "artists": int(psql("SELECT COUNT(*) FROM artists", RESTORE_DB)),
            "releases": int(psql("SELECT COUNT(*) FROM releases", RESTORE_DB)),
            "tracks": int(psql("SELECT COUNT(*) FROM tracks", RESTORE_DB)),
            "users": int(psql("SELECT COUNT(*) FROM native_users", RESTORE_DB)),
        }
        restored_media = {
            "file_count": int(psql("SELECT COUNT(*) FROM media_assets", RESTORE_DB)),
            "bytes": int(psql("SELECT COALESCE(SUM(byte_size), 0) FROM media_assets", RESTORE_DB)),
        }
        restore_matches = restored_counts == target_counts and restored_media == target_media
        evidence["steps"]["backup_restore"] = {
            "wall_s": round(time.monotonic() - step, 2),
            "dump_bytes": dump_path.stat().st_size,
            "restored_tracks": restored_counts["tracks"],
            "restore_matches": restore_matches,
            "backup": backup_evidence,
            "restored_counts": restored_counts,
            "restored_media": restored_media,
        }
        assert restore_matches, {
            "target_counts": target_counts,
            "restored_counts": restored_counts,
            "target_media": target_media,
            "restored_media": restored_media,
        }

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

        if not snapshot_mode:
            # This records the synthetic rollback audit path only. It is not
            # evidence that external traffic moved in either direction.
            cli_top(
                TARGET_DB,
                [
                    "rollback",
                    "record",
                    "--database-url",
                    db_url(TARGET_DB),
                    "--run",
                    run_id,
                    "--legacy-target",
                    "synthetic-legacy",
                    "--by",
                    "phase11-rehearsal",
                    "--note",
                    "synthetic target failure; traffic-return procedure exercised",
                    "--confirm",
                ],
            )

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
            "audit_recorded": not snapshot_mode,
            "audit_scope": "synthetic durable audit only" if not snapshot_mode else "external operator leg not executed",
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
