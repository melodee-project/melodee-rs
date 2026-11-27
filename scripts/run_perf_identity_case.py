#!/usr/bin/env python3
"""Capture one bounded production identity-query case on an owned fixture.

Invoke as the direct child of perf_representative.py. Only synthetic plans and
fixed benchmark fields are retained; raw child output is deleted. Run different
fixture cardinalities sequentially with the same fingerprinted test binary.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time

from build_perf_server import command_output, file_digest, source_fingerprints
from perf_jellyfin_audio_plan import check_parent
from perf_media_io_trace import child_exited, require_owned_session_support, terminate_owned
from run_perf_identity_benchmark import resolve_binary

ROOT = Path(__file__).resolve().parents[1]
MAX_LOG = 32 * 1024 * 1024
WORKLOAD_SCRIPTS = ("scripts/perf_representative.py", "scripts/test_database.py",
                    "scripts/build_perf_server.py", "scripts/build_perf_test.py", "scripts/build_perf_cli.py",
                    "scripts/run_perf_identity_benchmark.py", "scripts/perf_identity_fixture.py",
                    "scripts/run_perf_mutation_benchmarks.py", "scripts/perf_jellyfin_audio_plan.py",
                    "scripts/perf_media_io_trace.py", "scripts/run_perf_identity_case.py")
QUERIES = ("strong_artist", "same_release", "repeated_credit", "name_proposal", "strong_release")
QUERY = re.compile(r"^identity_query=([a-z_]+)_(scoped|global) status=(completed|censored_statement_timeout) elapsed_ms=([0-9.]+)(?: rows=([0-9]+))?$")
PLAN = re.compile(r"^identity_plan=([a-z_]+) scope=(scoped|global) (.*)$")
FIXTURE = re.compile(r"^fixture_artists=([0-9]+) fixture_releases=([0-9]+) fixture_tracks=([0-9]+) statement_timeout_seconds=20$")
RELATIONS = re.compile(r"^identity_fixture_relations observations=([0-9]+) current_decisions=([0-9]+) track_credits=([0-9]+) artist_external_ids=([0-9]+) release_external_ids=([0-9]+)$")
DRAIN = re.compile(r"^identity_drain_ran=(true|false) elapsed_ms=([0-9.]+) initial_seeds=([0-9]+) affected_artist_ids=([0-9]+) affected_release_ids=([0-9]+) remaining_seeds=([0-9]+) merged_strong_artists=([0-9]+) closure_artist_merges=([0-9]+) closure_release_merges=([0-9]+)$")
ORACLE = re.compile(r"^identity_oracle=([a-z_]+) outcome=(equivalent|censored)(?: returned_rows=([0-9]+))?$")
CLOSURE_TURN = re.compile(r"^identity_closure_turn=([0-9]+) seeds_remaining=([0-9]+) original_seeds_remaining=([0-9]+) elapsed_ms=([0-9.]+)$")
CLOSURE_OUTCOME = re.compile(r"^identity_closure_outcome=(quiescent|turn_limit_censored|deadline_censored|drain_unavailable) turns=([0-9]+) seeds_remaining=([0-9]+) elapsed_ms=([0-9.]+)$")


def parse_output(output: str) -> dict:
    queries, plans, oracle, fixture, relations, drain = {}, {}, {}, None, None, None
    turns, closure = [], None
    for line in output.splitlines():
        # With an exact libtest filter, --nocapture may put the harness's
        # fixed test-name prefix on the first application output line.
        prefix = "test measure_scoped_identity_discovery_and_production_drain ... "
        if line.startswith(prefix):
            line = line[len(prefix):]
        if match := QUERY.fullmatch(line):
            key = f"{match[1]}_{match[2]}"
            if match[1] not in QUERIES or key in queries:
                raise ValueError("unexpected identity query event")
            queries[key] = {"status": match[3], "elapsed_ms": float(match[4]),
                            "rows": int(match[5]) if match[5] is not None else None}
        elif match := PLAN.fullmatch(line):
            key = f"{match[1]}_{match[2]}"
            if match[1] not in QUERIES or key in plans:
                raise ValueError("unexpected identity plan event")
            if match[3] == "status=censored_statement_timeout":
                plans[key] = {"status": "censored_statement_timeout"}
            else:
                value = json.loads(match[3])
                if not isinstance(value, list) or len(value) != 1 or not isinstance(value[0], dict):
                    raise ValueError("invalid EXPLAIN output")
                plans[key] = {"status": "complete", "explain": value}
        elif match := FIXTURE.fullmatch(line):
            if fixture is not None:
                raise ValueError("duplicate fixture event")
            fixture = {"artists": int(match[1]), "releases": int(match[2]), "tracks": int(match[3])}
        elif match := RELATIONS.fullmatch(line):
            if relations is not None:
                raise ValueError("duplicate relationship event")
            relations = dict(zip(("observations", "current_decisions", "track_credits", "artist_external_ids", "release_external_ids"),
                                 (int(value) for value in match.groups()), strict=True))
        elif match := DRAIN.fullmatch(line):
            if drain is not None:
                raise ValueError("duplicate drain event")
            drain = {"ran": match[1] == "true", "elapsed_ms": float(match[2]),
                     **dict(zip(("initial_seeds", "affected_artist_ids", "affected_release_ids", "remaining_seeds",
                                 "merged_strong_artists", "closure_artist_merges", "closure_release_merges"),
                                (int(value) for value in match.groups()[2:]), strict=True))}
        elif match := ORACLE.fullmatch(line):
            if match[1] not in QUERIES or match[1] in oracle:
                raise ValueError("unexpected identity oracle event")
            oracle[match[1]] = {"outcome": match[2], "returned_rows": int(match[3]) if match[3] else None}
        elif match := CLOSURE_TURN.fullmatch(line):
            if int(match[1]) != len(turns) or len(turns) > 64:
                raise ValueError("invalid closure turn sequence")
            turns.append({"turn": int(match[1]), "seeds_remaining": int(match[2]),
                          "original_seeds_remaining": int(match[3]), "elapsed_ms": float(match[4])})
        elif match := CLOSURE_OUTCOME.fullmatch(line):
            if closure is not None:
                raise ValueError("duplicate closure outcome")
            closure = {"outcome": match[1], "turns": int(match[2]),
                       "seeds_remaining": int(match[3]), "elapsed_ms": float(match[4])}
    expected = {f"{query}_{scope}" for query in QUERIES for scope in ("scoped", "global")}
    if (fixture is None or relations is None or drain is None or closure is None or not turns
            or set(queries) != expected or set(oracle) != set(QUERIES)):
        raise ValueError("incomplete identity measurement")
    for key, value in queries.items():
        if value["status"] == "completed" and key not in plans:
            raise ValueError("completed query lacks plan")
        if key.endswith("_scoped") and (value["status"] != "completed" or not value["rows"]):
            raise ValueError("scoped workload did not produce its intended candidate")
    for name in QUERIES:
        global_query = queries[f"{name}_global"]
        scoped_query = queries[f"{name}_scoped"]
        comparison = oracle[name]
        global_plan = plans.get(f"{name}_global")
        if global_query["status"] == "completed":
            if (global_query["rows"] is None or comparison["outcome"] != "equivalent"
                    or comparison["returned_rows"] != global_query["rows"]
                    or global_query["rows"] != scoped_query["rows"]):
                raise ValueError("global query and oracle disagree")
        elif (global_query["rows"] is not None or comparison["outcome"] != "censored"
              or comparison["returned_rows"] is not None or global_plan is not None):
            raise ValueError("censored global query has inconsistent evidence")
    if not drain["ran"]:
        raise ValueError("production drain did not run")
    if (turns[0]["seeds_remaining"] != drain["remaining_seeds"]
            or closure["turns"] != turns[-1]["turn"]
            or closure["seeds_remaining"] != turns[-1]["seeds_remaining"]
            or any(turn["original_seeds_remaining"] > min(drain["initial_seeds"], turn["seeds_remaining"]) for turn in turns)
            or any(right["original_seeds_remaining"] > left["original_seeds_remaining"]
                   or right["elapsed_ms"] < left["elapsed_ms"] for left, right in zip(turns, turns[1:]))
            or (closure["outcome"] == "quiescent" and (closure["seeds_remaining"] != 0
                or turns[-1]["original_seeds_remaining"] != 0))
            or (closure["outcome"] != "quiescent" and closure["seeds_remaining"] == 0)):
        raise ValueError("inconsistent closure measurement")
    return {"fixture": fixture, "relations": relations, "queries": queries, "plans": plans,
            "oracle": oracle, "drain": drain, "closure": {**closure, "samples": turns}}


def run(env: dict[str, str], binary: Path, provenance: Path, output: Path, timeout: int,
        migration_provenance: Path | None = None) -> None:
    if not 1 <= timeout <= 1800:
        raise ValueError("timeout out of bounds")
    check_parent(env, os.getppid())
    binary = resolve_binary(binary)
    if output.exists() or output.is_symlink() or provenance.is_symlink() or not provenance.is_file() or provenance.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid output or provenance")
    build = json.loads(provenance.read_text(encoding="utf-8"))
    head = command_output(["git", "rev-parse", "HEAD"], ROOT)
    before = source_fingerprints(ROOT)
    script_hashes = {name: file_digest(ROOT / name) for name in WORKLOAD_SCRIPTS}
    if (env.get("MELODEE_PERF_FIXTURE_SHA256") != script_hashes["scripts/perf_representative.py"]
            or env.get("MELODEE_PERF_DATABASE_HELPER_SHA256") != script_hashes["scripts/test_database.py"]):
        raise ValueError("fixture source changed since parent startup")
    if (build.get("test_target") != "identity" or build.get("build_profile") != "release"
            or build.get("test_executable_sha256") != file_digest(binary)
            or build.get("provenance_producer_sha256") != script_hashes["scripts/build_perf_test.py"]
            or build.get("fingerprint_helper_sha256") != script_hashes["scripts/build_perf_server.py"]
            or build.get("source_sha256") != before or build.get("commit") != head):
        raise ValueError("identity binary does not match current build provenance")
    migration_proof_sha = None
    if migration_provenance is not None:
        if (migration_provenance.is_symlink() or not migration_provenance.is_file()
                or migration_provenance.stat().st_size > 4 * 1024 * 1024):
            raise ValueError("invalid migration CLI provenance")
        migration_proof_sha = file_digest(migration_provenance)
        migration = json.loads(migration_provenance.read_text(encoding="utf-8"))
        if (not isinstance(migration, dict)
                or env.get("MELODEE_PERF_MIGRATION_PROVENANCE_SHA256") != migration_proof_sha
                or migration.get("target") != "migration-cli" or migration.get("build_profile") != "release"
                or migration.get("binary_sha256") != env.get("MELODEE_PERF_MIGRATION_CLI_SHA256")
                or migration.get("commit") != head or migration.get("source_sha256") != before
                or migration.get("provenance_producer_sha256") != script_hashes["scripts/build_perf_cli.py"]
                or migration.get("fingerprint_helper_sha256") != script_hashes["scripts/build_perf_server.py"]):
            raise ValueError("fixture migration proof does not match this build")
    require_owned_session_support()
    with tempfile.TemporaryDirectory(prefix="melodee-identity-case-") as directory:
        root = Path(directory)
        (root / ".env").touch()
        child_env = env.copy()
        for key in ("TMPDIR", "TMP", "TEMP"):
            child_env[key] = str(root)
        with tempfile.TemporaryFile(mode="w+b", dir=root) as log:
            child = subprocess.Popen([sys.executable, str(ROOT / "scripts/run_perf_identity_benchmark.py"),
                "--test-binary", str(binary), "--augment-identities"], cwd=root, env=child_env,
                stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                leader = os.pidfd_open(child.pid)
            except BaseException:
                terminate_owned(child)
                raise
            started = time.monotonic()
            censored = None
            try:
                while not child_exited(leader):
                    if time.monotonic() - started >= timeout:
                        censored = "timeout_censored"
                        break
                    if os.fstat(log.fileno()).st_size > MAX_LOG:
                        censored = "output_limit_censored"
                        break
                    time.sleep(0.1)
            finally:
                try:
                    terminate_owned(child)
                finally:
                    os.close(leader)
            if os.fstat(log.fileno()).st_size > MAX_LOG:
                censored = "output_limit_censored"
            if censored:
                result = {"status": censored, "timeout_seconds": timeout}
            elif child.returncode:
                raise RuntimeError("identity benchmark failed; raw output removed")
            else:
                log.seek(0)
                result = parse_output(log.read(MAX_LOG + 1).decode("utf-8"))
                result["status"] = "complete" if result["closure"]["outcome"] == "quiescent" else "closure_incomplete"
            if (source_fingerprints(ROOT) != before or command_output(["git", "rev-parse", "HEAD"], ROOT) != head
                    or script_hashes != {name: file_digest(ROOT / name) for name in WORKLOAD_SCRIPTS}
                    or (migration_provenance is not None and file_digest(migration_provenance) != migration_proof_sha)):
                raise RuntimeError("source changed during identity measurement")
            result.update({"binary_sha256": file_digest(binary), "build_provenance_sha256": file_digest(provenance),
                           "build_commit": head, "migration_cli_sha256": env.get("MELODEE_PERF_MIGRATION_CLI_SHA256"),
                           "migration_cli_source_identity": ("matched fixture-validated build proof"
                               if migration_proof_sha is not None else "not independently established"),
                           "migration_cli_provenance_sha256": migration_proof_sha,
                           "fixture_sha256": env.get("MELODEE_PERF_FIXTURE_SHA256"),
                           "collector_sha256": file_digest(Path(__file__)),
                           "script_sha256": script_hashes,
                           "limitations": ["single sample per query, global runs after scoped under uncontrolled warm cache",
                               "synthetic logical closure is structurally identical but UUIDs differ across fixture sizes",
                               "SQL buffer statistics do not measure end-to-end import or physical disk bytes",
                               "no representative negative decisions, redirects, concurrent imports or media processing",
                               "build fingerprints do not rule out transient edits or attest dependency binaries"]})
            with output.open("x", encoding="utf-8") as destination:
                json.dump(result, destination, indent=2, sort_keys=True)
                destination.write("\n")
            if censored or result["status"] == "closure_incomplete":
                raise RuntimeError("identity measurement censored")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", required=True, type=Path)
    parser.add_argument("--build-provenance", required=True, type=Path)
    parser.add_argument("--migration-cli-provenance", type=Path,
                        help="require the fixture-validated migration CLI build record")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout-seconds", type=int, default=600)
    args = parser.parse_args()
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        run(dict(os.environ), args.test_binary, args.build_provenance, args.output, args.timeout_seconds,
            args.migration_cli_provenance)
    except (OSError, UnicodeError, ValueError, RuntimeError, InterruptedError) as error:
        print(f"Identity measurement failed: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
