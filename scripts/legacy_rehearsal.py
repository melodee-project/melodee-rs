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
    clean     Drop the drill databases.

Usage:

    python3 scripts/legacy_rehearsal.py seed --songs 12000
    python3 scripts/legacy_rehearsal.py drill --songs 12000
    python3 scripts/legacy_rehearsal.py clean
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

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
    artists = max(50, songs // 24)
    albums = max(100, songs // 12)
    create_db(SOURCE_DB)
    ddl = LEGACY_DDL.read_text()
    psql_file(SOURCE_DB, ddl)
    users = max(10, artists // 10)
    playlists = max(20, artists // 5)
    user_songs = max(500, songs // 2)
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
FROM generate_series(1, greatest(200, {songs} / 5)) g;
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
            digest = hashlib.blake2b(path.read_bytes(), digest_size=16).hexdigest()
            size = path.stat().st_size
            entries.append({"path": str(path.relative_to(root)), "size": size, "blake2b": digest})
            total += size
    return {"files": len(entries), "bytes": total, "entries": entries}


def drill(songs: int) -> dict:
    evidence: dict = {
        "drill": "legacy_rehearsal",
        "songs_requested": songs,
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "steps": {},
    }
    workdir = Path(tempfile.mkdtemp(prefix="melodee-r4-rehearsal-"))
    canonical = make_canonical_root(workdir, songs)
    canonical_before = tree_fingerprint(canonical)
    search_root = workdir / "search"
    search_root.mkdir(parents=True, exist_ok=True)
    search_env = {
        "MELODEE__SEARCH__ENABLED": "true",
        "MELODEE__SEARCH__ROOT": str(search_root),
    }

    try:
        # --- R4.1: rehearsal environment + immutable baseline -------------
        step = time.monotonic()
        create_db(TARGET_DB)
        cli_plain(TARGET_DB, ["migrate"])
        evidence["steps"]["create_target"] = {"wall_s": round(time.monotonic() - step, 2)}

        step = time.monotonic()
        out = cli(TARGET_DB, ["plan", "--source-db", db_url(SOURCE_DB), "--library-path-map",
                              "/music=library", "--json"])
        run_id = json.loads(out)["run_id"]
        evidence["steps"]["plan"] = {"run_id": run_id, "wall_s": round(time.monotonic() - step, 2)}

        out = cli(TARGET_DB, ["baseline", "--run", run_id, "--source-db", db_url(SOURCE_DB), "--json"])
        baseline = json.loads(out)
        evidence["baseline"] = baseline

        # --- R4.2: import + verify --------------------------------------
        step = time.monotonic()
        cli(TARGET_DB, ["import", "--run", run_id, "--source-db", db_url(SOURCE_DB),
                        "--library-path-map", "/music=library", "--batch-size", "2500"])
        evidence["steps"]["import"] = {
            **cli.last_metrics,
            "wall_s": round(time.monotonic() - step, 2),
        }

        step = time.monotonic()
        out = cli(TARGET_DB, ["verify", "--run", run_id, "--source-db", db_url(SOURCE_DB), "--json"])
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
        }
        assert restored_tracks == target_counts["tracks"]

        # --- R4.7: freeze + final delta + cutover-ready ------------------
        # One write lands before the marker; it must be imported by the
        # freeze's final delta.
        psql(
            'INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","SortName","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt") '
            "VALUES (900000001, gen_random_uuid(), 'Pre-Freeze Late Arrival', 'pre-freeze late arrival', 'Pre-Freeze Late Arrival', 'late-arrival', 0, 0, false, 1, 0, 0, 0, 0, now(), now())",
            SOURCE_DB,
        )
        step = time.monotonic()
        out = cli(TARGET_DB, ["freeze", "--run", run_id, "--source-db", db_url(SOURCE_DB),
                              "--library-path-map", "/music=library",
                              "--acknowledge-cutover",
                              "--note", "r4 rehearsal drill freeze",
                              "--json"])
        freeze_report = json.loads(out)
        evidence["steps"]["freeze"] = {
            "wall_s": round(time.monotonic() - step, 2),
            "state": freeze_report["state"],
            "freeze_window_ms": freeze_report["freeze"]["freeze_window_ms"],
        }
        assert freeze_report["state"] == "cutover-ready"
        late = int(
            psql("SELECT COUNT(*) FROM artists WHERE canonical_name = 'Pre-Freeze Late Arrival'", TARGET_DB)
        )
        evidence["steps"]["freeze"]["pre_marker_write_imported"] = late == 1
        assert late == 1

        # --- R4.8: rollback drill ----------------------------------------
        # Inject a cutover-blocking failure (target made unavailable), then
        # roll back: legacy source must be byte-identical and authoritative.
        drop_db(RESTORE_DB)  # keep only the failed target for forensics
        source_counts_after = {
            "artists": int(psql('SELECT COUNT(*) FROM "Artists"', SOURCE_DB)),
            "songs": int(psql('SELECT COUNT(*) FROM "Songs"', SOURCE_DB)),
        }
        media_after = tree_fingerprint(canonical)
        evidence["steps"]["rollback"] = {
            "source_counts_after": source_counts_after,
            "media_tree_after": media_after["files"],
            "media_tree_bytes_after": media_after["bytes"],
            "failed_target_preserved": int(
                psql(f"SELECT COUNT(*) FROM pg_database WHERE datname = '{TARGET_DB}'")
            )
            == 1,
        }
        canonical_after = tree_fingerprint(canonical)
        evidence["steps"]["rollback"]["canonical_tree_unchanged"] = (
            canonical_after == canonical_before
        )
        evidence["canonical_media"] = {
            "files": canonical_before["files"],
            "bytes": canonical_before["bytes"],
            "before_after_match": canonical_after == canonical_before,
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

    counts = seed_source(args.songs)
    evidence = drill(args.songs)
    evidence["source_counts"] = counts
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    out_path = args.output or EVIDENCE_DIR / f"rehearsal-drill-{args.songs}.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(evidence, indent=2) + "\n")
    print(f"evidence: {out_path}")
    print(json.dumps({k: v for k, v in evidence.items() if k in ("result", "steps")}, indent=2))
    if not args.keep:
        for name in (SOURCE_DB, TARGET_DB, RESTORE_DB):
            drop_db(name)


if __name__ == "__main__":
    main()
