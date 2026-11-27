#!/usr/bin/env python3
"""Run isolated DB competition cases sequentially using a verified server build.

Run as a child of perf_representative.py. Each case clones that invocation's
verified, idle fixture and removes only its own UUID-named database; the source
fixture is never modified. The new output directory and any completed results
are retained on failure; nothing in an existing directory is changed.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import uuid

from build_perf_server import file_digest, source_fingerprints
from perf_db_competition import MODES, run as run_workload, validate_options
from run_perf_mutation_benchmarks import validate_fixture
import test_database

ROOT = Path(__file__).resolve().parents[1]
WORKLOAD_FILES = ("scripts/perf_representative.py", "scripts/test_database.py",
                  "scripts/perf_db_competition.py", "scripts/perf_ingestion_workload.py",
                  "scripts/perf_ingress_workload.py", "scripts/run_perf_mutation_benchmarks.py",
                  "scripts/summarize_perf_metrics.py", "scripts/run_db_competition_matrix.py")


def validate_matrix(modes: list[str], pools: list[int], artists: int, releases: int,
                    tracks_per_release: int, albums: int, seconds: int) -> None:
    if not modes or len(modes) != len(set(modes)) or any(mode not in MODES for mode in modes):
        raise ValueError("modes must be nonempty, distinct and supported")
    if not pools or len(pools) != len(set(pools)) or len(pools) > 8:
        raise ValueError("pool sizes must be nonempty, distinct and at most eight")
    if min(artists, releases, tracks_per_release) < 1 or max(artists, releases) > 1_000_000 or tracks_per_release > 100:
        raise ValueError("fixture dimensions out of bounds")
    for pool in pools:
        validate_options(modes[0], albums, seconds, pool)


def verify_provenance(repo: Path, binary: Path, provenance: dict) -> None:
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True,
                            text=True, check=True, timeout=15).stdout.strip()
    if (provenance.get("build_profile") != "release" or provenance.get("commit") != commit
            or provenance.get("binary_sha256") != file_digest(binary)
            or provenance.get("source_sha256") != source_fingerprints(repo)):
        raise ValueError("server binary or source tree differs from the build record")


def run(modes: list[str], pools: list[int], artists: int, releases: int,
        tracks_per_release: int, albums: int, seconds: int, server: Path,
        provenance_path: Path, output_dir: Path, migration_cli: Path | None,
        media_parent: Path, allow_missing_metrics: bool, source_repo: Path = ROOT,
        env: dict[str, str] | None = None) -> int:
    validate_matrix(modes, pools, artists, releases, tracks_per_release, albums, seconds)
    env = dict(os.environ if env is None else env)
    validate_fixture(env)
    source = env["MELODEE_PERF_OWNED_DATABASE"]
    source_url = env["MELODEE__DATABASE__URL"]
    admin = env.get("MELODEE_TEST_DATABASE_URL", "")
    if not admin or test_database.database_url(admin, source) != source_url:
        raise ValueError("fixture and administrator database topology differ")
    if output_dir.exists() or output_dir.is_symlink():
        raise ValueError("output directory already exists")
    output_dir = output_dir.absolute()
    server = server.resolve(strict=True)
    media_parent = media_parent.resolve(strict=True)
    source_repo = source_repo.resolve(strict=True)
    if not server.is_file() or not media_parent.is_dir():
        raise ValueError("server binary and media parent must exist")
    if provenance_path.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("build record exceeds size bound")
    provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
    if not isinstance(provenance, dict):
        raise ValueError("invalid build record")
    verify_provenance(source_repo, server, provenance)
    workload_hashes = {name: file_digest(ROOT / name) for name in WORKLOAD_FILES}
    if (env.get("MELODEE_PERF_FIXTURE_SHA256") != workload_hashes["scripts/perf_representative.py"]
            or env.get("MELODEE_PERF_DATABASE_HELPER_SHA256") != workload_hashes["scripts/test_database.py"]):
        raise ValueError("fixture or database helper changed since the parent invocation began")
    if migration_cli is None:
        target = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target")))
        if not target.is_absolute():
            target = ROOT / target
        if os.environ.get("CARGO_BUILD_TARGET"):
            target /= os.environ["CARGO_BUILD_TARGET"]
        migration_cli = target / "debug" / ("melodee-cli.exe" if os.name == "nt" else "melodee-cli")
    migration_cli = migration_cli.resolve(strict=True)
    if not migration_cli.is_file():
        raise ValueError("migration CLI must be a file")
    migration_hash = file_digest(migration_cli)
    if env.get("MELODEE_PERF_MIGRATION_CLI_SHA256") != migration_hash:
        raise ValueError("migration CLI differs from the fixture invocation")
    if test_database.sql(source_url, "SELECT current_database()") != source:
        raise ValueError("fixture database identity mismatch")
    shape = json.loads(test_database.sql(source_url, """SELECT json_build_object(
        'artists',(SELECT count(*) FROM artists), 'releases',(SELECT count(*) FROM releases),
        'tracks',(SELECT count(*) FROM tracks), 'work',(SELECT count(*) FROM release_work_items),
        'jobs',(SELECT count(*) FROM search_rebuild_jobs))"""))
    if shape != {"artists": artists, "releases": releases,
                 "tracks": releases * tracks_per_release, "work": 0, "jobs": 0}:
        raise ValueError("fixture dimensions differ or source has existing work")
    output_dir.mkdir()
    cases = []
    created: set[str] = set()
    try:
        for pool in pools:
            for mode in modes:
                verify_provenance(source_repo, server, provenance)
                if workload_hashes != {name: file_digest(ROOT / name) for name in WORKLOAD_FILES}:
                    raise RuntimeError("workload source changed during matrix")
                if file_digest(migration_cli) != migration_hash:
                    raise RuntimeError("migration CLI changed during matrix")
                name = "melodee_template_" + uuid.uuid4().hex
                if not test_database.OWNED.fullmatch(name) or name == source:
                    raise ValueError("invalid clone database name")
                clone_started = time.monotonic()
                try:
                    test_database.sql(admin, f'CREATE DATABASE "{name}" TEMPLATE "{source}"')
                except BaseException:
                    # CREATE may have committed before the client lost its
                    # acknowledgement. Never drop an unregistered name: an
                    # operator must inspect the uncertain outcome first.
                    print(json.dumps({"uncertain_clone_create": name}), file=sys.stderr, flush=True)
                    raise
                created.add(name)
                clone_url = test_database.database_url(admin, name)
                if test_database.sql(clone_url, "SELECT current_database()") != name:
                    raise RuntimeError("clone database identity mismatch")
                clone_seconds = time.monotonic() - clone_started
                output = output_dir / f"pool-{pool}-{mode}.json"
                child_env = env.copy()
                child_env.update({"MELODEE_PERF_OWNED_DATABASE": name, "MELODEE__DATABASE__URL": clone_url})
                # Running the workload in this coordinator keeps the server
                # process handle in the same process that owns clone cleanup.
                exit_code = run_workload(mode, albums, seconds, pool, server, media_parent,
                                         child_env, allow_missing_metrics, output)
                test_database.drop_owned(admin, name, created)
                metrics_complete = False
                if output.is_file() and not output.is_symlink() and output.stat().st_size <= 16 * 1024 * 1024:
                    data = json.loads(output.read_text(encoding="utf-8"))
                    missing = data.get("required_metrics_missing") if isinstance(data, dict) else None
                    metrics_complete = isinstance(missing, list) and not missing
                cases.append({"mode": mode, "pool_size": pool, "exit_code": exit_code,
                              "result": output.name if output.is_file() else None,
                              "clone_seconds": round(clone_seconds, 6),
                              "metrics_complete": metrics_complete})
                if exit_code:
                    break
            if cases[-1]["exit_code"]:
                break
    finally:
        failures = []
        for name in list(created):
            try:
                test_database.drop_owned(admin, name, created)
            except (RuntimeError, ValueError, OSError) as error:
                failures.append(f"{name}: {error}")
        if failures:
            raise RuntimeError("owned clone cleanup failed: " + "; ".join(failures))
    verify_provenance(source_repo, server, provenance)
    if workload_hashes != {name: file_digest(ROOT / name) for name in WORKLOAD_FILES}:
        raise RuntimeError("workload source changed during matrix")
    if file_digest(migration_cli) != migration_hash:
        raise RuntimeError("migration CLI changed during matrix")
    report = {"build_record_sha256": file_digest(provenance_path),
              "binary_sha256": provenance["binary_sha256"], "commit": provenance["commit"],
              "workload_sha256": workload_hashes,
              "migration_cli_sha256": migration_hash,
              "migration_cli_source_identity": "not independently established",
              "fixture": {"artists": artists, "releases": releases,
                          "tracks": releases * tracks_per_release,
                          "seed_timeout_seconds": env.get("MELODEE_PERF_SEED_TIMEOUT_SECONDS")},
              "cases": cases, "cases_succeeded": len(cases) == len(modes) * len(pools)
              and all(case["exit_code"] == 0 for case in cases),
              "metrics_complete": len(cases) == len(modes) * len(pools)
              and all(case["metrics_complete"] for case in cases),
              "limitations": ["each case uses a clone of the same fixture and uncontrolled operating-system cache",
                              "source fingerprints do not prove reproducible compilation or prevent transient edits",
                              "refer to each case for overlap, sampling and metric-coverage limitations"]}
    with (output_dir / "matrix.json").open("x", encoding="utf-8") as destination:
        json.dump(report, destination, indent=2, sort_keys=True)
        destination.write("\n")
    print(json.dumps({"output_directory": str(output_dir), "cases_succeeded": report["cases_succeeded"],
                      "metrics_complete": report["metrics_complete"]}))
    return 0 if report["cases_succeeded"] else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=MODES, action="append", help="repeat; defaults to all four modes")
    parser.add_argument("--pool-size", type=int, action="append", help="repeat; defaults to 8 and 32")
    parser.add_argument("--artists", type=int, default=35000)
    parser.add_argument("--releases", type=int, default=50000)
    parser.add_argument("--tracks-per-release", type=int, default=12)
    parser.add_argument("--albums", type=int, default=200)
    parser.add_argument("--seconds", type=int, default=900)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/melodee-server")
    parser.add_argument("--source-repo", type=Path, default=ROOT,
                        help="worktree whose source matches the server build record")
    parser.add_argument("--provenance", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True, help="must not already exist")
    parser.add_argument("--migration-cli", type=Path)
    parser.add_argument("--media-parent", type=Path, default=ROOT)
    parser.add_argument("--allow-missing-metrics", action="store_true")
    args = parser.parse_args()
    previous_term = signal.getsignal(signal.SIGTERM)
    previous_int = signal.getsignal(signal.SIGINT)
    def interrupt(signum, _frame):
        # Do not let repeated signals interrupt owned-server/clone cleanup.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if signum == signal.SIGINT:
            raise KeyboardInterrupt
        raise InterruptedError("termination requested")
    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    try:
        return run(args.mode or list(MODES), args.pool_size or [8, 32], args.artists,
                   args.releases, args.tracks_per_release, args.albums, args.seconds,
                   args.server, args.provenance, args.output_dir, args.migration_cli,
                   args.media_parent, args.allow_missing_metrics, args.source_repo)
    except RuntimeError as error:
        print(f"Competition matrix failed: {error}", file=sys.stderr)
        return 1
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Competition matrix failed: {type(error).__name__}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("Competition matrix interrupted; owned cleanup attempted", file=sys.stderr)
        return 130
    finally:
        signal.signal(signal.SIGTERM, previous_term)
        signal.signal(signal.SIGINT, previous_int)


if __name__ == "__main__":
    raise SystemExit(main())
