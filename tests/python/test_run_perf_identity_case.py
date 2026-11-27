"""Identity measurement only executes under an owned fixture and bounded parser."""
import os
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import build_perf_test
import run_perf_identity_case as case
import run_perf_identity_benchmark


def example():
    lines = ["fixture_artists=10 fixture_releases=10 fixture_tracks=120 statement_timeout_seconds=20",
             "identity_fixture_relations observations=130 current_decisions=130 track_credits=120 artist_external_ids=4 release_external_ids=3"]
    for name in case.QUERIES:
        for scope in ("scoped", "global"):
            lines.append(f"identity_query={name}_{scope} status=completed elapsed_ms=1.25 rows=1")
            lines.append(f'identity_plan={name} scope={scope} [{{"Plan":{{"Actual Rows":1}}}}]')
        lines.append(f"identity_oracle={name} outcome=equivalent returned_rows=1")
    lines.append("identity_drain_ran=true elapsed_ms=2.0 initial_seeds=8 affected_artist_ids=6 affected_release_ids=2 remaining_seeds=7 merged_strong_artists=1 closure_artist_merges=2 closure_release_merges=1")
    lines.extend(["identity_closure_turn=0 seeds_remaining=7 original_seeds_remaining=0 elapsed_ms=0.1",
                  "identity_closure_turn=1 seeds_remaining=0 original_seeds_remaining=0 elapsed_ms=1.0",
                  "identity_closure_outcome=quiescent turns=1 seeds_remaining=0 elapsed_ms=1.1"])
    return "\n".join(lines)


class IdentityCaseTests(unittest.TestCase):
    def test_help_is_safe_and_identity_build_target_is_narrow(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/run_perf_identity_case.py"), "--help"],
                                capture_output=True, env={}, check=False)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(build_perf_test.TARGETS["identity"], ("melodee-catalog", "perf_identity_reconciliation", False))
        self.assertIn("scripts/build_perf_server.py", case.WORKLOAD_SCRIPTS)
        self.assertIn("scripts/build_perf_test.py", case.WORKLOAD_SCRIPTS)
        self.assertEqual(run_perf_identity_benchmark.benchmark_command(Path("fixture-test")),
            ["fixture-test", "measure_scoped_identity_discovery_and_production_drain", "--exact",
             "--ignored", "--nocapture", "--test-threads=1"])

    def test_parser_requires_all_nonempty_scoped_candidates_and_plans(self):
        parsed = case.parse_output(example())
        self.assertEqual(parsed["fixture"]["tracks"], 120)
        self.assertEqual(parsed["drain"]["initial_seeds"], 8)
        self.assertEqual(len(parsed["plans"]), 10)
        prefixed = "test measure_scoped_identity_discovery_and_production_drain ... " + example()
        self.assertEqual(case.parse_output(prefixed), parsed)
        with self.assertRaises(ValueError):
            case.parse_output(example().replace("same_release_scoped status=completed elapsed_ms=1.25 rows=1",
                                                "same_release_scoped status=completed elapsed_ms=1.25 rows=0"))
        with self.assertRaises(ValueError):
            case.parse_output(example().replace("identity_oracle=strong_artist outcome=equivalent returned_rows=1", ""))

    def test_duplicate_oracle_and_unowned_target_fail_closed(self):
        with self.assertRaises(ValueError):
            case.parse_output(example() + "\nidentity_oracle=strong_artist outcome=censored")
        with tempfile.TemporaryDirectory() as directory, \
             mock.patch.object(case.subprocess, "Popen") as child:
            root = Path(directory)
            with self.assertRaises((KeyError, ValueError)):
                case.run({}, root / "fake", root / "proof", root / "result", 10)
            child.assert_not_called()

    def test_censored_global_is_explicit_and_does_not_need_a_plan(self):
        raw = example().replace("identity_query=strong_artist_global status=completed elapsed_ms=1.25 rows=1",
                                "identity_query=strong_artist_global status=censored_statement_timeout elapsed_ms=20000.0")
        raw = raw.replace('identity_plan=strong_artist scope=global [{"Plan":{"Actual Rows":1}}]\n', "")
        raw = raw.replace("identity_oracle=strong_artist outcome=equivalent returned_rows=1",
                          "identity_oracle=strong_artist outcome=censored")
        parsed = case.parse_output(raw)
        self.assertEqual(parsed["oracle"]["strong_artist"]["outcome"], "censored")
        self.assertNotIn("strong_artist_global", parsed["plans"])

    def test_oracle_disposition_must_match_global_query_and_rows(self):
        valid = example()
        variants = [
            valid.replace("identity_oracle=strong_artist outcome=equivalent returned_rows=1",
                          "identity_oracle=strong_artist outcome=censored"),
            valid.replace("identity_oracle=strong_artist outcome=equivalent returned_rows=1",
                          "identity_oracle=strong_artist outcome=equivalent returned_rows=2"),
            valid.replace("strong_artist_global status=completed elapsed_ms=1.25 rows=1",
                          "strong_artist_global status=completed elapsed_ms=1.25"),
            valid.replace("strong_artist_global status=completed elapsed_ms=1.25 rows=1",
                          "strong_artist_global status=censored_statement_timeout elapsed_ms=20000.0"),
            valid.replace("strong_artist_global status=completed elapsed_ms=1.25 rows=1",
                          "strong_artist_global status=completed elapsed_ms=1.25 rows=2")
                 .replace("identity_oracle=strong_artist outcome=equivalent returned_rows=1",
                          "identity_oracle=strong_artist outcome=equivalent returned_rows=2"),
        ]
        for raw in variants:
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                case.parse_output(raw)

    def test_closure_outcome_must_match_observed_queue(self):
        raw = example()
        parsed = case.parse_output(raw)
        self.assertEqual(parsed["closure"]["outcome"], "quiescent")
        for invalid in (
            raw.replace("identity_closure_outcome=quiescent turns=1 seeds_remaining=0",
                        "identity_closure_outcome=quiescent turns=1 seeds_remaining=1"),
            raw.replace("identity_closure_turn=0 seeds_remaining=7", "identity_closure_turn=0 seeds_remaining=8"),
            raw.replace("identity_closure_turn=1 seeds_remaining=0", "identity_closure_turn=2 seeds_remaining=0"),
            raw.replace("identity_closure_turn=1 seeds_remaining=0 original_seeds_remaining=0",
                        "identity_closure_turn=1 seeds_remaining=0 original_seeds_remaining=1"),
        ):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                case.parse_output(invalid)

    def test_wrong_build_fingerprint_fails_before_spawn(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary, proof = root / "binary", root / "proof.json"
            binary.write_bytes(b"fixture")
            proof.write_text('{"test_target":"identity","build_profile":"release",'
                             '"test_executable_sha256":"incorrect","commit":"fixture",'
                             '"source_sha256":{}}')
            env = {"MELODEE_PERF_FIXTURE_SHA256": case.file_digest(ROOT / "scripts/perf_representative.py"),
                   "MELODEE_PERF_DATABASE_HELPER_SHA256": case.file_digest(ROOT / "scripts/test_database.py")}
            with mock.patch.object(case, "check_parent"), \
                 mock.patch.object(case, "resolve_binary", return_value=binary), \
                 mock.patch.object(case, "source_fingerprints", return_value={}), \
                 mock.patch.object(case, "command_output", return_value="fixture"), \
                 mock.patch.object(case.subprocess, "Popen") as child:
                with self.assertRaises(ValueError):
                    case.run(env, binary, proof, root / "result", 10)
                child.assert_not_called()

    def test_changed_fingerprint_helper_rejects_before_spawn(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary, proof = root / "binary", root / "proof.json"
            binary.write_bytes(b"fixture")
            proof.write_text(json.dumps({"test_target": "identity", "build_profile": "release",
                "test_executable_sha256": case.file_digest(binary), "commit": "fixture", "source_sha256": {},
                "provenance_producer_sha256": case.file_digest(ROOT / "scripts/build_perf_test.py"),
                "fingerprint_helper_sha256": "changed-helper"}))
            env = {"MELODEE_PERF_FIXTURE_SHA256": case.file_digest(ROOT / "scripts/perf_representative.py"),
                   "MELODEE_PERF_DATABASE_HELPER_SHA256": case.file_digest(ROOT / "scripts/test_database.py")}
            with mock.patch.object(case, "check_parent"), \
                 mock.patch.object(case, "resolve_binary", return_value=binary), \
                 mock.patch.object(case, "source_fingerprints", return_value={}), \
                 mock.patch.object(case, "command_output", return_value="fixture"), \
                 mock.patch.object(case.subprocess, "Popen") as child:
                with self.assertRaises(ValueError):
                    case.run(env, binary, proof, root / "result", 10)
                child.assert_not_called()

    def test_identity_case_rejects_unvalidated_migration_proof(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary, proof, migration = root / "binary", root / "proof.json", root / "migration.json"
            binary.write_bytes(b"fixture")
            proof.write_text(json.dumps({"test_target": "identity", "build_profile": "release",
                "test_executable_sha256": case.file_digest(binary), "commit": "fixture", "source_sha256": {},
                "provenance_producer_sha256": case.file_digest(ROOT / "scripts/build_perf_test.py"),
                "fingerprint_helper_sha256": case.file_digest(ROOT / "scripts/build_perf_server.py")}))
            migration.write_text('{}')
            env = {"MELODEE_PERF_FIXTURE_SHA256": case.file_digest(ROOT / "scripts/perf_representative.py"),
                   "MELODEE_PERF_DATABASE_HELPER_SHA256": case.file_digest(ROOT / "scripts/test_database.py"),
                   "MELODEE_PERF_MIGRATION_PROVENANCE_SHA256": "incorrect"}
            with mock.patch.object(case, "check_parent"), \
                 mock.patch.object(case, "resolve_binary", return_value=binary), \
                 mock.patch.object(case, "source_fingerprints", return_value={}), \
                 mock.patch.object(case, "command_output", return_value="fixture"), \
                 mock.patch.object(case.subprocess, "Popen") as child:
                with self.assertRaises(ValueError):
                    case.run(env, binary, proof, root / "result", 10, migration)
                child.assert_not_called()


if __name__ == "__main__":
    unittest.main()
