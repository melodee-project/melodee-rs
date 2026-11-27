#!/usr/bin/env python3
"""Compare two read-only admin-stats production routes on one owned fixture.

The baseline may differ from the candidate only in admin_stats.rs. Execute
ABBA sequentially with fingerprinted release-test binaries; keep synthetic
aggregate data and samples, never raw output, credentials, or paths.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import math
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

ROOT = Path(__file__).resolve().parents[1]
CHANGED_FILE = "apps/melodee-server/src/admin_stats.rs"
MAX_LOG = 4 * 1024 * 1024
SAMPLE = re.compile(r"^(test measure_read_only_admin_stats_route \.\.\. )?admin_stats_ab_iteration=([0-8]) elapsed_ms=([0-9.]+) response_bytes=([0-9]+)$")
FIXTURE = re.compile(r"^admin_stats_ab_fixture artists=([0-9]+) releases=([0-9]+) tracks=([0-9]+)$")


def parse_output(data: str) -> dict:
    samples, response, fixture = [], None, None
    for line in data.splitlines():
        if match := SAMPLE.fullmatch(line):
            if int(match[2]) != len(samples) or len(samples) >= 9 or (match[1] and samples):
                raise ValueError("invalid admin-stats sample sequence")
            duration = float(match[3])
            if not math.isfinite(duration) or int(match[4]) < 1:
                raise ValueError("invalid admin-stats sample")
            samples.append({"elapsed_ms": duration, "response_bytes": int(match[4])})
        elif line.startswith("admin_stats_ab_response="):
            if response is not None:
                raise ValueError("duplicate response output")
            response = line.split("=", 1)[1]
            parsed = json.loads(response)
            if (not isinstance(parsed, dict) or "generatedAt" in parsed
                    or not isinstance(parsed.get("library"), dict)):
                raise ValueError("invalid normalized response output")
        elif match := FIXTURE.fullmatch(line):
            if fixture is not None:
                raise ValueError("duplicate fixture output")
            fixture = {"artists": int(match[1]), "releases": int(match[2]), "tracks": int(match[3])}
    if len(samples) != 9 or response is None or fixture is None:
        raise ValueError("incomplete admin-stats output")
    if any(parsed["library"].get(key) != value for key, value in fixture.items()):
        raise ValueError("library totals differ from fixture")
    return {"samples": samples, "response": response, "fixture": fixture}


def load_proof(path: Path, target: str) -> dict:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid proof")
    proof = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(proof, dict) or proof.get("test_target") != target or proof.get("build_profile") != "release":
        raise ValueError("wrong test build proof")
    return proof


def validate_binary(path: Path, proof: dict) -> Path:
    if path.is_symlink() or not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError("invalid test executable")
    path = path.resolve(strict=True)
    if (path.parent.name != "deps" or path.parent.parent.name != "release"
            or path.parent.parent.parent.name != "target" or not path.name.startswith("perf_admin_stats_ab-")
            or proof.get("test_executable_name") != path.name
            or proof.get("test_executable_sha256") != file_digest(path)):
        raise ValueError("test executable differs from proof")
    return path


def source_pair(old: dict, current: dict, baseline_source: bytes) -> tuple[str, str]:
    if (set(old) != set(current)
            or [name for name in sorted(current) if old[name] != current[name]] != [CHANGED_FILE]
            or old[CHANGED_FILE] != hashlib.sha256(baseline_source).hexdigest()):
        raise ValueError("baseline must differ only in admin stats source")
    def digest(values: dict) -> str:
        encoded = json.dumps(values, sort_keys=True, separators=(",", ":")).encode()
        return hashlib.sha256(encoded).hexdigest()
    return digest(old), digest(current)


def cross_process_response(response: str) -> str:
    """Preserve ordering while masking only independently varying DB size."""
    value = json.loads(response)
    if not isinstance(value, dict) or not isinstance(value.get("database"), dict):
        raise ValueError("invalid normalized response output")
    size = value["database"].get("totalBytes")
    if type(size) is not int or size < 0:
        raise ValueError("invalid database size observation")
    # Assignment preserves the existing key's position and integer type.
    value["database"]["totalBytes"] = 0
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def summarize_samples(runs: list[dict]) -> dict:
    result = {}
    for variant in ("baseline", "candidate"):
        values = sorted(sample["elapsed_ms"] for run in runs if run["variant"] == variant
                        for sample in run["samples"][1:])
        if len(values) != 16:
            raise ValueError("incomplete post-warmup samples")
        result[variant] = {"count": len(values), **{
            f"p{int(percentile * 100)}_ms": values[math.ceil(percentile * len(values)) - 1]
            for percentile in (0.50, 0.95, 0.99)}}
    return result


def parse_quiescence(raw: str | None) -> dict | None:
    quiescence_raw = raw
    quiescence = None
    if quiescence_raw is not None:
        if len(quiescence_raw) > 512:
            raise ValueError("invalid fixture quiescence record")
        quiescence = json.loads(quiescence_raw)
        if (not isinstance(quiescence, dict)
                or set(quiescence) != {"mode", "stable_observations", "observations", "duration_seconds"}
                or quiescence["mode"] != "vacuum_and_size_stability"
                or type(quiescence["stable_observations"]) is not int
                or quiescence["stable_observations"] != 3
                or type(quiescence["observations"]) is not int or quiescence["observations"] < 3
                or type(quiescence["duration_seconds"]) not in (int, float)
                or not math.isfinite(quiescence["duration_seconds"])
                or not 0 <= quiescence["duration_seconds"] <= 600):
            raise ValueError("invalid fixture quiescence record")
    return quiescence


def test_command(binary: Path) -> list[str]:
    return [str(binary), "measure_read_only_admin_stats_route", "--exact",
            "--ignored", "--nocapture", "--test-threads=1"]


def changed_response_fields(data: str) -> list[str]:
    """Return fixed field labels from a Rust assertion, never its values."""
    sides = {}
    for line in data.splitlines():
        match = re.fullmatch(r"\s*(left|right):\s*(\".*\")\s*", line)
        if not match:
            continue
        try:
            decoded = json.loads(json.loads(match[2]))
        except (ValueError, TypeError):
            continue
        if isinstance(decoded, dict):
            sides[match[1]] = decoded
    if set(sides) != {"left", "right"}:
        return []
    return response_field_labels(sides["left"], sides["right"])


def response_field_labels(left: dict, right: dict) -> list[str]:
    """Compare only fixed section labels and never include response values."""
    fields = []
    for name in ("library", "processing", "listening"):
        if json.dumps(left.get(name), separators=(",", ":")) != json.dumps(right.get(name), separators=(",", ":")):
            fields.append(name)
    if isinstance(left.get("database"), dict) and isinstance(right.get("database"), dict):
        for name in ("totalBytes", "assetFileBytes", "assetFileCount", "tables", "workItems", "playEvents",
                     "pendingProjection", "poolCapacity", "poolInUse"):
            if left["database"].get(name) != right["database"].get(name):
                fields.append("database." + name)
    elif left.get("database") != right.get("database"):
        fields.append("database")
    return fields


def test_failure_reason(data: str) -> str:
    # Test output is untrusted; expose only fixed labels, never assertion values.
    if "normalized stats result changed between samples" in data:
        return "admin-stats test failed: normalized response changed within process"
    return "admin-stats test failed: other"


def invoke(binary: Path, root: Path, env: dict[str, str], timeout: int) -> dict:
    with tempfile.TemporaryFile(mode="w+b", dir=root) as log:
        child = subprocess.Popen(test_command(binary),
            cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT,
            start_new_session=True)
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
            return {"status": censored}
        log.seek(0)
        data = log.read(MAX_LOG + 1).decode("utf-8", errors="replace")
        if child.returncode:
            fields = changed_response_fields(data)
            if fields:
                print("admin-stats changed fields: " + ",".join(fields), file=sys.stderr)
            raise RuntimeError(test_failure_reason(data))
        return {"status": "complete", **parse_output(data)}


def run(env: dict[str, str], baseline: Path, candidate: Path, baseline_proof: Path,
        candidate_proof: Path, migration_proof: Path, output: Path, timeout: int) -> None:
    if not 1 <= timeout <= 900:
        raise ValueError("timeout out of bounds")
    check_parent(env, os.getppid())
    if output.exists() or output.is_symlink():
        raise ValueError("output already exists")
    old, new = load_proof(baseline_proof, "admin-stats"), load_proof(candidate_proof, "admin-stats")
    old_proof_sha, new_proof_sha = file_digest(baseline_proof), file_digest(candidate_proof)
    baseline, candidate = validate_binary(baseline, old), validate_binary(candidate, new)
    current = source_fingerprints(ROOT)
    head = command_output(["git", "rev-parse", "HEAD"], ROOT)
    old_sources, new_sources = old.get("source_sha256"), new.get("source_sha256")
    baseline_source = subprocess.run(["git", "show", f"{head}:{CHANGED_FILE}"], cwd=ROOT,
                                     capture_output=True, check=True, timeout=15).stdout
    if (not isinstance(old_sources, dict) or new_sources != current or old.get("commit") != head
            or new.get("commit") != head or type(old.get("worktree_dirty")) is not bool
            or type(new.get("worktree_dirty")) is not bool):
        raise ValueError("invalid baseline or candidate source identity")
    old_source_digest, new_source_digest = source_pair(old_sources, current, baseline_source)
    for proof in (old, new):
        if (proof.get("provenance_producer_sha256") != file_digest(ROOT / "scripts/build_perf_test.py")
                or proof.get("fingerprint_helper_sha256") != file_digest(ROOT / "scripts/build_perf_server.py")):
            raise ValueError("build provenance helper differs")
    if migration_proof.is_symlink() or not migration_proof.is_file() or migration_proof.stat().st_size > 4 * 1024 * 1024:
        raise ValueError("invalid migration proof")
    migration_sha = file_digest(migration_proof)
    migration = json.loads(migration_proof.read_text(encoding="utf-8"))
    if (not isinstance(migration, dict) or migration.get("target") != "migration-cli"
            or migration.get("build_profile") != "release"
            or migration.get("source_sha256") != current or migration.get("commit") != head
            or migration.get("binary_sha256") != env.get("MELODEE_PERF_MIGRATION_CLI_SHA256")
            or env.get("MELODEE_PERF_MIGRATION_PROVENANCE_SHA256") != migration_sha
            or migration.get("provenance_producer_sha256") != file_digest(ROOT / "scripts/build_perf_cli.py")
            or migration.get("fingerprint_helper_sha256") != file_digest(ROOT / "scripts/build_perf_server.py")):
        raise ValueError("fixture migration proof differs")
    quiescence = parse_quiescence(env.get("MELODEE_PERF_FIXTURE_QUIESCENCE"))
    require_owned_session_support()
    scripts = ("scripts/run_perf_admin_stats_ab.py", "scripts/perf_representative.py", "scripts/test_database.py",
               "scripts/build_perf_cli.py", "scripts/build_perf_test.py", "scripts/build_perf_server.py",
               "scripts/perf_media_io_trace.py", "scripts/perf_jellyfin_audio_plan.py")
    script_hashes = {name: file_digest(ROOT / name) for name in scripts}
    if (script_hashes["scripts/perf_representative.py"] != env.get("MELODEE_PERF_FIXTURE_SHA256")
            or script_hashes["scripts/test_database.py"] != env.get("MELODEE_PERF_DATABASE_HELPER_SHA256")):
        raise ValueError("fixture scripts changed")
    with tempfile.TemporaryDirectory(prefix="melodee-admin-stats-ab-") as directory:
        root = Path(directory)
        (root / ".env").touch()
        child_env = env.copy()
        for key in ("TMPDIR", "TMP", "TEMP"):
            child_env[key] = str(root)
        runs, expected_response, expected_cross_response, expected_fixture = [], None, None, None
        for label, binary in (("baseline", baseline), ("candidate", candidate), ("candidate", candidate), ("baseline", baseline)):
            row = invoke(binary, root, child_env, timeout)
            row["variant"] = label
            if row["status"] != "complete":
                runs.append(row)
                break
            cross_response = cross_process_response(row["response"])
            if expected_response is None:
                expected_response, expected_cross_response, expected_fixture = row["response"], cross_response, row["fixture"]
            elif cross_response != expected_cross_response or row["fixture"] != expected_fixture:
                fields = [field for field in response_field_labels(json.loads(expected_response), json.loads(row["response"]))
                          if field != "database.totalBytes"]
                print("admin-stats changed fields: " + (",".join(fields) if fields else "order_or_unclassified"),
                      file=sys.stderr)
                raise RuntimeError("baseline/candidate normalized response results differ")
            del row["response"]
            runs.append(row)
        if (source_fingerprints(ROOT) != current or command_output(["git", "rev-parse", "HEAD"], ROOT) != head
                or script_hashes != {name: file_digest(ROOT / name) for name in scripts}
                or file_digest(migration_proof) != migration_sha
                or file_digest(baseline_proof) != old_proof_sha or file_digest(candidate_proof) != new_proof_sha
                or file_digest(baseline) != old["test_executable_sha256"]
                or file_digest(candidate) != new["test_executable_sha256"]):
            raise RuntimeError("benchmark source or proof changed")
        complete = len(runs) == 4 and all(row["status"] == "complete" for row in runs)
        response_json = expected_response if expected_response is not None else ""
        record = {"status": "complete" if complete else "censored", "runs": runs,
                  "post_warmup_percentiles": summarize_samples(runs) if complete else None,
                  "fixture": expected_fixture, "fixture_quiescence": quiescence, "pool_connections": 8,
                  "normalized_response_sha256": hashlib.sha256(response_json.encode()).hexdigest(),
                  "cross_process_response_sha256": hashlib.sha256((expected_cross_response or "").encode()).hexdigest(),
                  "volatile_cross_process_fields": ["database.totalBytes"],
                  "baseline_binary_sha256": old["test_executable_sha256"], "candidate_binary_sha256": new["test_executable_sha256"],
                  "baseline_build_sha256": old_proof_sha, "candidate_build_sha256": new_proof_sha,
                  "migration_build_sha256": migration_sha, "source_changed_file": CHANGED_FILE,
                  "baseline_file_sha256": old_sources[CHANGED_FILE], "candidate_file_sha256": current[CHANGED_FILE],
                  "script_sha256": script_hashes, "base_commit": head,
                  "baseline_worktree_dirty": old["worktree_dirty"], "candidate_worktree_dirty": new["worktree_dirty"],
                  "baseline_source_tree_sha256": old_source_digest, "candidate_source_tree_sha256": new_source_digest,
                  "source_context": "fingerprinted snapshots based on commit; neither is assumed to be clean HEAD",
                  "limitations": ["in-process authenticated router; no TCP or concurrent ingestion",
                    "eight fixture pool connections prewarmed and observed stable before each process",
                    "ABBA on one fixture; cache not controlled, first sample per process is warmup",
                    "nearest-rank percentiles use only 16 samples per variant; p95 and p99 are coarse",
                    "within each process the normalized response compares exactly except validated generatedAt",
                    "across processes database.totalBytes is validated as nonnegative integer and replaced at the same key position before ordered comparison",
                    "source and binary hashes do not attest dependency binaries or exclude transient edits"]}
        with output.open("x", encoding="utf-8") as destination:
            json.dump(record, destination, indent=2, sort_keys=True)
            destination.write("\n")
        if not complete:
            raise RuntimeError("admin-stats comparison censored")


def safe_failure_reason(error: Exception) -> str | None:
    allowed = {"invalid admin-stats sample sequence", "invalid admin-stats sample", "duplicate response output",
               "invalid normalized response output", "duplicate fixture output", "incomplete admin-stats output",
               "library totals differ from fixture", "fixture parent or database topology mismatch",
               "fixture or ownership helper changed", "invalid baseline or candidate source identity",
               "baseline must differ only in admin stats source", "build provenance helper differs",
               "invalid migration proof", "fixture migration proof differs", "fixture scripts changed",
               "invalid test executable", "test executable differs from proof", "invalid proof", "wrong test build proof",
               "invalid fixture quiescence record", "invalid database size observation"}
    reason = str(error)
    runtime_allowed = {"admin-stats test failed: normalized response changed within process",
                       "admin-stats test failed: other", "baseline/candidate normalized response results differ",
                       "benchmark source or proof changed", "admin-stats comparison censored"}
    if isinstance(error, ValueError) and reason in allowed:
        return reason
    if isinstance(error, RuntimeError) and reason in runtime_allowed:
        return reason
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-binary", type=Path, required=True)
    parser.add_argument("--candidate-binary", type=Path, required=True)
    parser.add_argument("--baseline-proof", type=Path, required=True)
    parser.add_argument("--candidate-proof", type=Path, required=True)
    parser.add_argument("--migration-cli-provenance", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=300)
    args = parser.parse_args()
    def interrupt(signum, _frame):
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        raise InterruptedError(f"signal {signum}")
    signal.signal(signal.SIGINT, interrupt)
    signal.signal(signal.SIGTERM, interrupt)
    try:
        run(dict(os.environ), args.baseline_binary, args.candidate_binary, args.baseline_proof,
            args.candidate_proof, args.migration_cli_provenance, args.output, args.timeout_seconds)
    except (OSError, UnicodeError, ValueError, RuntimeError, InterruptedError) as error:
        reason = safe_failure_reason(error)
        detail = f": {reason}" if reason else ""
        print(f"Admin stats comparison failed: {type(error).__name__}{detail}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
