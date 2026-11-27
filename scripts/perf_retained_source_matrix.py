#!/usr/bin/env python3
"""Measure real retained-source reobservation under loopback HTTP and DB load.

Run only as a direct child of perf_representative.py. Each case owns a cloned
database, generated WAV files, a server and temporary paths. The original
fixture receives only a private ownership marker. Raw logs and media are removed; results are bounded
and contain no source paths, credentials or log lines.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid

from build_perf_server import file_digest
from perf_db_competition import (MAX_LOG_BYTES, MAX_SAMPLES, ENVIRONMENT_SQL, native_login,
    prepare_root, probe_loop, query, summary, unused_loopback_port, within)
from perf_ingress_workload import host_info, percentiles, server_resources, start_server, stop_server, temporary_storage_info
from perf_jellyfin_audio_plan import check_parent
from run_db_competition_matrix import verify_provenance
from summarize_media_operations import summarize as summarize_operations
from summarize_perf_metrics import summarize as summarize_db_metrics
from run_perf_mutation_benchmarks import validate_fixture
import test_database
import perf_owned_clone as owned_clone

ROOT = Path(__file__).resolve().parents[1]
SOURCE_PREFIX = "retained-bench-"
STATUS_SQL = """SELECT json_build_object(
 'database', current_database(),
 'total', count(*),
 'active', count(*) FILTER (WHERE retained.status='active'),
 'superseded', count(*) FILTER (WHERE retained.status='superseded'),
 'leased', count(*) FILTER (WHERE retained.lease_owner IS NOT NULL),
 'retrying', count(*) FILTER (WHERE retained.retry_count > 0 AND retained.last_retry_reason IS DISTINCT FROM 'awaiting_intake'),
 'created', count(*) FILTER (WHERE retained.completed_created IS TRUE),
 'outcomes', count(*) FILTER (WHERE retained.completed_outcome='superseded'),
 'correct_holds', count(*) FILTER (WHERE retained.status='active'
   AND retained.baseline_change_pending=true AND retained.last_retry_reason='awaiting_intake'
   AND retained.lease_owner IS NULL AND retained.superseded_by IS NULL
   AND prior.inbound_active=true AND successor.id IS NULL
   AND NOT EXISTS (SELECT 1 FROM release_work_items candidate WHERE candidate.id<>prior.id
      AND candidate.inbound_active=true AND candidate.source_library_id IS NOT DISTINCT FROM prior.source_library_id
      AND candidate.source_path COLLATE "C"=prior.source_path COLLATE "C")),
 'lock_waiters', (SELECT count(*) FROM pg_stat_activity WHERE datname=current_database()
   AND pid<>pg_backend_pid() AND wait_event_type='Lock'))
 FROM inbound_retained_sources retained
 JOIN release_work_items prior ON prior.id=retained.work_item_id
 LEFT JOIN release_work_items successor ON successor.id=retained.superseded_by
 WHERE retained.source_path LIKE 'retained-bench-%'"""


sql = owned_clone.sql


def run_owned_clone(admin: str, source_url: str, record: dict, manifest: Path, action) -> bool:
    """Persist recovery authority before CREATE; only an independently proven clone is dropped."""
    owned_clone.validate_record(record)
    schema, token, name, source = (record[k] for k in ("schema", "token", "name", "source"))
    sql(source_url, f'''BEGIN; DELETE FROM "{schema}".ownership;
        INSERT INTO "{schema}".ownership(singleton,token) VALUES (true,'{token}'); COMMIT''')
    # Never attempt cleanup of a pre-existing or inaccessible candidate.
    if (owned_clone.server_identity(admin) != record["server"]
            or sql(admin, f"SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname='{name}')") != "f"):
        raise RuntimeError("candidate name is not provably absent")
    owned_clone.write_manifest(manifest, record)
    acknowledged = False
    try:
        sql(admin, f'CREATE DATABASE "{name}" TEMPLATE "{source}"')
        acknowledged = True
        owned_clone.verify_candidate(record, admin)
        return action(name)
    finally:
        # A timeout or signal can arrive after CREATE committed. Even on a
        # missing candidate retain the manifest unless CREATE was acknowledged:
        # the remote backend may still be finishing the uncertain operation.
        result = owned_clone.recover(record, admin)
        if result == "dropped_verified" or (result == "absent" and acknowledged):
            if owned_clone.read_manifest(manifest) != record:
                raise RuntimeError("ownership manifest changed during cleanup")
            manifest.unlink()
            directory = os.open(manifest.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                os.fsync(directory)
            finally:
                os.close(directory)
        elif result != "absent":
            raise RuntimeError("unrecognized clone cleanup result")


def validate_options(rates: list[int], items: int, media_seconds: int, timeout: int,
                     idle_seconds: int, pool_size: int) -> None:
    if not rates or len(rates) > 6 or len(rates) != len(set(rates)) or any(not 1 <= rate <= 256 for rate in rates):
        raise ValueError("rates must contain 1..6 unique values in 1..256 MiB/s")
    if not 1 <= items <= 16 or not 5 <= media_seconds <= 120 or not 20 <= timeout <= 1200:
        raise ValueError("items, media duration or timeout out of bounds")
    if not 5 <= idle_seconds <= 120 or not 2 <= pool_size <= 64:
        raise ValueError("idle duration or pool size out of bounds")


def seed_statement(items: int) -> str:
    if not 1 <= items <= 16:
        raise ValueError("item count out of bounds")
    return f"""WITH inserted AS (
      INSERT INTO release_work_items (id,api_key,source_path,current_path,state,correlation_id,
        discovered_at,updated_at,created_at,lease_owner,lease_acquired_at,lease_expires_at)
      SELECT uuidv7(),uuidv7(),'{SOURCE_PREFIX}'||lpad(n::text,4,'0'),
        '{SOURCE_PREFIX}'||lpad(n::text,4,'0'),'ready',uuidv7(),
        clock_timestamp(),clock_timestamp(),clock_timestamp(),
        'retained-benchmark-hold',clock_timestamp(),clock_timestamp()+interval '1 hour'
      FROM generate_series(1,{items}) AS n RETURNING id,source_path
    ) INSERT INTO inbound_retained_sources
       (work_item_id,source_path,source_generation,content_digest,next_reobserve_at,created_at,updated_at)
      SELECT id,source_path,'retained-benchmark-previous','v3:'||repeat('0',64),
        clock_timestamp()+interval '1 day',clock_timestamp(),clock_timestamp() FROM inserted"""


def generate_media(root: Path, count: int, seconds: int) -> list[dict]:
    result = []
    for index in range(1, count + 1):
        directory = root / f"{SOURCE_PREFIX}{index:04d}"
        directory.mkdir()
        target = directory / "track.wav"
        command = ["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i",
                   f"anoisesrc=color=white:amplitude=0.1:sample_rate=44100:duration={seconds}:seed={index}",
                   "-ac", "2", "-c:a", "pcm_s16le", str(target)]
        subprocess.run(command, check=True, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=120)
        result.append({"path": target, "bytes": target.stat().st_size, "sha256": file_digest(target)})
    return result


def verify_media(files: list[dict]) -> bool:
    return all(not item["path"].is_symlink() and item["path"].is_file()
               and item["path"].stat().st_size == item["bytes"]
               and file_digest(item["path"]) == item["sha256"] for item in files)


def expected_retained_state(final: dict, items: int, rate: int | None, media_unchanged: bool) -> bool:
    return (media_unchanged and final.get("total") == items and final.get("retrying") == 0
            and final.get("active") == items and final.get("superseded") == 0
            and (rate is None or final.get("correct_holds") == items))


def run_case(rate: int | None, items: int, seconds: int, timeout: int, idle_seconds: int,
             pool_size: int, binary: Path, media_parent: Path, env: dict[str, str], output: Path,
             registered: set[str], source_name: str, admin_url: str) -> bool:
    validate_fixture(env)
    url, database = env["MELODEE__DATABASE__URL"], env["MELODEE_PERF_OWNED_DATABASE"]
    if (database not in registered or database == source_name
            or test_database.database_url(admin_url, database) != url):
        raise ValueError("refusing workload outside a registered clone")
    initial = query(url, ENVIRONMENT_SQL)
    if initial.pop("database") != database or initial["existing_work"] or initial["existing_jobs"]:
        raise ValueError("fixture identity mismatch or preexisting work")
    with tempfile.TemporaryDirectory(prefix="melodee-retained-mixed-", dir=media_parent) as temporary:
        root = Path(temporary)
        prepare_root(root)
        media = generate_media(root / "inbound", items, seconds)
        total_bytes = sum(row["bytes"] for row in media)
        sql(url, seed_statement(items))
        state = query(url, STATUS_SQL)
        if state.pop("database") != database or state["total"] != items or state["active"] != items:
            raise RuntimeError("retained seed shape mismatch")
        overrides = {"MELODEE__LOGGING__LEVEL": None,
                     "RUST_LOG": "info,melodee::performance=trace,melodee::media_operations=trace",
                     "MELODEE__SERVER__BIND": f"127.0.0.1:{unused_loopback_port()}",
                     "MELODEE__DATABASE__MAX_CONNECTIONS": str(pool_size),
                     "MELODEE__DATABASE__MIN_CONNECTIONS": "1",
                     "MELODEE__INGEST__RECONCILIATION_INTERVAL_SECONDS": "3600",
                     "MELODEE__INGEST__RETAINED_RECHECK_MIB_PER_SECOND": str(rate or 8),
                     "MELODEE__INGEST__RETAINED_RECHECK_INTERVAL_SECONDS": "3600",
                     "MELODEE__CATALOG__ENABLED": "false", "MELODEE__SEARCH__ENABLED": "false"}
        process, log, port = start_server(binary, root, url, 0, True, overrides, isolate_host_environment=True)
        log_path = Path(log.name)
        stop = threading.Event()
        samples, error = [], None
        active_begin = active_end = None
        started = time.monotonic()
        try:
            token = native_login(port)
            with ThreadPoolExecutor(max_workers=2) as executor:
                admin = executor.submit(probe_loop, port, token, "/api/admin/v1/stats", stop, started)
                browse = executor.submit(probe_loop, port, token, "/api/v1/artists?page=1&pageSize=50", stop, started)
                try:
                    # Exporter snapshots are periodic. Warm up before releasing
                    # the retained queue and allow a later snapshot afterward.
                    time.sleep(11)
                    if process.poll() is not None:
                        raise RuntimeError("server exited during metrics warmup")
                    if rate is not None:
                        sql(url, "UPDATE inbound_retained_sources SET next_reobserve_at=clock_timestamp() WHERE source_path LIKE 'retained-bench-%' AND status='active'")
                    measurement_started = time.monotonic()
                    if rate is None:
                        active_begin = measurement_started - started
                    limit = timeout if rate is not None else idle_seconds
                    while time.monotonic() - measurement_started < limit and len(samples) < MAX_SAMPLES:
                        if process.poll() is not None:
                            error = "server_exited"
                            break
                        if log_path.stat().st_size > MAX_LOG_BYTES:
                            error = "log_limit"
                            break
                        before = time.monotonic()
                        try:
                            row = query(url, STATUS_SQL)
                            elapsed = time.monotonic() - before
                            if row.pop("database") != database:
                                raise RuntimeError("fixture identity changed")
                            row.update({"started_seconds": before - started,
                                        "finished_seconds": time.monotonic() - started,
                                        "query_seconds": elapsed, "error": None})
                            samples.append(row)
                        except (OSError, ValueError, subprocess.SubprocessError):
                            samples.append({"started_seconds": before - started,
                                            "finished_seconds": time.monotonic() - started,
                                            "query_seconds": time.monotonic() - before,
                                            "error": "sampling_failed"})
                            error = "sampling_failed"
                            break
                        if row["retrying"]:
                            error = "retained_retry"
                            break
                        if rate is not None and row["leased"]:
                            # Conservative observed window: exclude the first
                            # and last sampling calls themselves. Sequential
                            # obligations may still leave gaps inside it.
                            active_begin = row["finished_seconds"] if active_begin is None else active_begin
                            active_end = row["started_seconds"]
                        if rate is not None and row["correct_holds"] == items:
                            break
                        time.sleep(0.25)
                    else:
                        if rate is not None:
                            error = "sample_limit" if len(samples) >= MAX_SAMPLES else "deadline_censored"
                        else:
                            active_end = time.monotonic() - started
                    if error is None and process.poll() is None:
                        time.sleep(11)
                finally:
                    stop.set()
                admin_samples, browse_samples = admin.result(timeout=50), browse.result(timeout=50)
            resources = server_resources(process.pid)
        finally:
            stop.set()
            stop_server(process, log)
        if log_path.stat().st_size > MAX_LOG_BYTES:
            raise RuntimeError("server log exceeded bound")
        media_unchanged = verify_media(media)
        with log_path.open(encoding="utf-8", errors="replace") as stream:
            operations = summarize_operations(stream)
        try:
            with log_path.open(encoding="utf-8", errors="replace") as stream:
                db_metrics = summarize_db_metrics(stream)
        except (OSError, ValueError):
            db_metrics = []
        overlap_admin = within(admin_samples, active_begin, active_end)
        overlap_browse = within(browse_samples, active_begin, active_end)
        db_good = [row["query_seconds"] for row in samples if row.get("error") is None]
        db_overlap = [row["query_seconds"] for row in within(samples, active_begin, active_end) if row.get("error") is None]
        final = samples[-1] if samples else {}
        counts = operations["operations"]["acquisition_tree_hash"]
        valid_ops = (rate is None or (operations["complete"] and operations["byte_counts_complete"]
            and counts["operations"] >= items * 2 and counts["succeeded"] >= items * 2
            and counts["read_bytes"] >= total_bytes * 2))
        correct = expected_retained_state(final, items, rate, media_unchanged)
        overlap_sufficient = (active_begin is not None and active_end is not None
                              and active_end > active_begin and bool(overlap_admin) and bool(overlap_browse))
        success = (error is None and correct and valid_ops and summary(admin_samples)["errors"] == 0
                   and summary(browse_samples)["errors"] == 0 and bool(db_good)
                   and overlap_sufficient)
        report = {"case": "idle" if rate is None else "retained", "rate_mib_per_second": rate,
            "timestamp_utc": datetime.now(timezone.utc).isoformat(), "fixture": initial,
            "items": items, "media_seconds": seconds, "media_bytes": total_bytes,
            "server_sha256": file_digest(binary), "pool_max_connections": pool_size,
            "host": host_info(), "storage": temporary_storage_info(media_parent),
            "active_window_seconds": [active_begin, active_end], "timeout_seconds": timeout,
            "window_kind": "idle control interval" if rate is None else "sampled reobservation interval",
            "metrics_warmup_seconds": 11, "metrics_post_observation_seconds": 11 if error is None else 0,
            "failure": error, "deadline_censored": error == "deadline_censored",
            "database_samples": samples, "db_query_all": {"count": len(db_good),
                "errors": len(samples)-len(db_good), "latency_seconds": percentiles(db_good) if db_good else None},
            "db_query_overlap": {"count": len(db_overlap), "latency_seconds": percentiles(db_overlap) if db_overlap else None},
            "admin_all": summary(admin_samples), "browse_all": summary(browse_samples),
            "admin_overlap": summary(overlap_admin), "browse_overlap": summary(overlap_browse),
            "operations": operations, "db_pool_metrics": db_metrics, "server_resources": resources,
            "source_bytes_unchanged": media_unchanged, "holds_correct": correct,
            "expected_minimum_retained_logical_read_bytes": total_bytes * 2 if rate is not None else 0,
            "telemetry_sufficient": valid_ops, "overlap_sufficient": overlap_sufficient, "success": success,
            "limitations": ["seeded durable obligations; source copy, receipt binding and initial ingestion are bypassed",
                "changed sources remain held awaiting copy-only intake; no successor is produced or measured",
                "single host, sequential clones and uncontrolled cache; WAV noise, not representative music",
                "logical instrumented reads are shared by all acquisition hashing and are not physical I/O",
                "DB timing includes psql process startup and status SQL; not pooled SQL-only latency",
                "active-window bounds sampled at 250ms intervals; gaps between obligations may be included and short episodes missed",
                "HTTP uses loopback; only two clients, search/indexing/other ingestion workers not stressed",
                "pool histograms cover exporter snapshots surrounding the workload, not only the overlap window",
                "server RSS/CPU exclude PostgreSQL and subprocesses; trace logging overhead unmeasured"]}
        with output.open("x", encoding="utf-8") as destination:
            json.dump(report, destination, indent=2, sort_keys=True)
            destination.write("\n")
        return success


def run(rates: list[int], items: int, media_seconds: int, timeout: int, idle_seconds: int,
        pool_size: int, server: Path, provenance_path: Path, migration_cli: Path,
        media_parent: Path, output_dir: Path, artists: int, releases: int,
        tracks_per_release: int, env: dict[str, str]) -> int:
    validate_options(rates, items, media_seconds, timeout, idle_seconds, pool_size)
    if min(artists, releases, tracks_per_release) < 1 or max(artists, releases) > 1_000_000 or tracks_per_release > 100:
        raise ValueError("fixture cardinality out of bounds")
    admin, source_name, source_url = check_parent(env, os.getppid())
    if output_dir.exists() or output_dir.is_symlink():
        raise ValueError("output directory already exists")
    if provenance_path.is_symlink() or not provenance_path.is_file() or provenance_path.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid build provenance")
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    server, migration_cli, media_parent = (path.resolve(strict=True) for path in (server, migration_cli, media_parent))
    if not server.is_file() or not migration_cli.is_file() or not media_parent.is_dir():
        raise ValueError("missing binary or media parent")
    storage = temporary_storage_info(media_parent)
    if storage["filesystem"] in (None, "tmpfs", "ramfs", "devtmpfs", "overlay"):
        raise ValueError("media parent must use a known disk-backed filesystem")
    verify_provenance(ROOT, server, provenance)
    migration_hash = file_digest(migration_cli)
    if env.get("MELODEE_PERF_MIGRATION_CLI_SHA256") != migration_hash:
        raise ValueError("migration CLI differs from the parent fixture")
    if sql(source_url, "SELECT current_database()") != source_name:
        raise ValueError("source fixture identity changed")
    shape = json.loads(sql(source_url, """SELECT json_build_object(
        'artists',(SELECT count(*) FROM artists), 'releases',(SELECT count(*) FROM releases),
        'tracks',(SELECT count(*) FROM tracks), 'work',(SELECT count(*) FROM release_work_items),
        'jobs',(SELECT count(*) FROM search_rebuild_jobs),
        'retained',(SELECT count(*) FROM inbound_retained_sources))"""))
    if shape != {"artists": artists, "releases": releases, "tracks": releases * tracks_per_release,
                 "work": 0, "jobs": 0, "retained": 0}:
        raise ValueError("fixture shape or preexisting work does not match")
    paths = ("scripts/perf_retained_source_matrix.py", "scripts/perf_representative.py",
             "scripts/test_database.py", "scripts/perf_db_competition.py", "scripts/perf_ingress_workload.py",
             "scripts/perf_ingestion_workload.py", "scripts/summarize_media_operations.py",
             "scripts/summarize_perf_metrics.py", "scripts/run_db_competition_matrix.py",
             "scripts/perf_jellyfin_audio_plan.py", "scripts/run_perf_mutation_benchmarks.py",
             "scripts/perf_owned_clone.py")
    script_hashes = {name: file_digest(ROOT / name) for name in paths}
    if (env.get("MELODEE_PERF_FIXTURE_SHA256") != script_hashes["scripts/perf_representative.py"]
            or env.get("MELODEE_PERF_DATABASE_HELPER_SHA256") != script_hashes["scripts/test_database.py"]):
        raise ValueError("fixture code changed since parent started")
    output_dir.mkdir(mode=0o700)
    parent_fd = os.open(output_dir.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(parent_fd)
    finally:
        os.close(parent_fd)
    schema = "perf_owner_" + uuid.uuid4().hex
    identity = owned_clone.server_identity(admin)
    endpoint = owned_clone.endpoint(admin)
    if owned_clone.server_identity(source_url) != identity:
        raise ValueError("source fixture server identity differs from administrator server")
    sql(source_url, f'CREATE SCHEMA "{schema}"; CREATE TABLE "{schema}".ownership '
        '(singleton boolean PRIMARY KEY CHECK (singleton), token text NOT NULL)')
    results = []
    for rate in [None, *rates]:
            verify_provenance(ROOT, server, provenance)
            if script_hashes != {name: file_digest(ROOT / name) for name in paths} or file_digest(migration_cli) != migration_hash:
                raise RuntimeError("benchmark or migration binary changed during matrix")
            name = "melodee_template_" + uuid.uuid4().hex
            if not test_database.OWNED.fullmatch(name) or name == source_name:
                raise ValueError("invalid owned clone name")
            name_out = "idle.json" if rate is None else f"rate-{rate}.json"
            record = {"version": 1, "name": name, "source": source_name, "schema": schema,
                      "token": uuid.uuid4().hex + uuid.uuid4().hex, "endpoint": endpoint, "server": identity}
            def action(owned_name):
                url = test_database.database_url(admin, owned_name)
                child_env = env.copy()
                child_env.update({"MELODEE_PERF_OWNED_DATABASE": owned_name, "MELODEE__DATABASE__URL": url})
                return run_case(rate, items, media_seconds, timeout, idle_seconds, pool_size,
                                server, media_parent, child_env, output_dir / name_out,
                                {owned_name}, source_name, admin)
            success = run_owned_clone(admin, source_url, record, output_dir / f"{name}.ownership.json", action)
            results.append({"rate_mib_per_second": rate, "output": name_out, "success": success})
            if not success:
                break
    verify_provenance(ROOT, server, provenance)
    if script_hashes != {name: file_digest(ROOT / name) for name in paths} or file_digest(migration_cli) != migration_hash:
        raise RuntimeError("benchmark or migration binary changed during matrix")
    complete = len(results) == len(rates) + 1 and all(row["success"] for row in results)
    report = {"build_record_sha256": file_digest(provenance_path), "binary_sha256": file_digest(server),
        "migration_cli_sha256": migration_hash, "migration_cli_source_identity": "not independently established",
        "fixture": shape, "script_sha256": script_hashes, "case_order": results, "success": complete,
        "limitations": ["sequential cases with uncontrolled caches, no independent repetitions",
                        "source fingerprints do not rule out transient source edits",
                        "read per-case limitations and overlap/sample counts before comparing latencies"]}
    with (output_dir / "matrix.json").open("x", encoding="utf-8") as destination:
        json.dump(report, destination, indent=2, sort_keys=True)
        destination.write("\n")
    return 0 if complete else 1


def install_signal_handlers() -> None:
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rate", action="append", type=int, help="repeat 1..6 distinct values, default 1, 8, 32")
    parser.add_argument("--items", type=int, default=8)
    parser.add_argument("--media-seconds", type=int, default=30)
    parser.add_argument("--timeout", type=int, default=300)
    parser.add_argument("--idle-seconds", type=int, default=30)
    parser.add_argument("--pool-size", type=int, default=8)
    parser.add_argument("--artists", type=int, default=35000)
    parser.add_argument("--releases", type=int, default=50000)
    parser.add_argument("--tracks-per-release", type=int, default=12)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    parser.add_argument("--migration-cli", type=Path, required=True)
    parser.add_argument("--provenance", type=Path, required=True)
    parser.add_argument("--media-parent", type=Path, default=ROOT)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    install_signal_handlers()
    try:
        return run(args.rate or [1, 8, 32], args.items, args.media_seconds, args.timeout,
                   args.idle_seconds, args.pool_size, args.server, args.provenance,
                   args.migration_cli, args.media_parent, args.output_dir, args.artists,
                   args.releases, args.tracks_per_release, dict(os.environ))
    except (OSError, ValueError, RuntimeError, InterruptedError, subprocess.SubprocessError, KeyError) as error:
        print(f"Retained-source matrix failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
