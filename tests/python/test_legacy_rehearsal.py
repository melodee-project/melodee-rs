"""Safety checks for the isolated Phase 11 rehearsal harness."""
from __future__ import annotations

import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "scripts" / "legacy_rehearsal.py"
SPEC = importlib.util.spec_from_file_location("legacy_rehearsal", MODULE_PATH)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RehearsalInputTests(unittest.TestCase):
    def test_seed_playlist_pairs_are_unique_for_small_shapes(self) -> None:
        for songs in (1, 2, 12, 24, 101):
            count = MODULE.playlist_seed_count(songs, 20)
            self.assertLessEqual(count, songs * 20)
            self.assertEqual(count, min(max(200, songs // 5), songs * 20))
            pairs = {
                MODULE.playlist_seed_pair(sequence, songs, 20)
                for sequence in range(1, count + 1)
            }
            self.assertEqual(len(pairs), count)

            user_count = MODULE.user_song_seed_count(songs, 10)
            user_pairs = {
                MODULE.user_song_seed_pair(sequence, songs, 10)
                for sequence in range(1, user_count + 1)
            }
            self.assertEqual(len(user_pairs), user_count)

    def test_malformed_database_url_fails_without_echoing_credentials(self) -> None:
        with self.assertRaisesRegex(ValueError, "database URL") as context:
            MODULE.validate_database_url(
                "postgres://operator:super-secret@example.invalid"
            )
        self.assertNotIn("super-secret", str(context.exception))

    def test_library_path_maps_fail_closed_before_target_creation(self) -> None:
        invalid = [
            "relative=library",
            "/music=../library",
            "/music=/absolute",
            "/music=library\\nested",
            "/music/../other=library",
            "/music=library\x00suffix",
        ]
        with tempfile.TemporaryDirectory() as directory:
            canonical = Path(directory) / "canonical"
            canonical.mkdir()
            with patch.object(MODULE, "create_db") as create_db:
                for value in invalid:
                    with self.subTest(value=value), self.assertRaisesRegex(
                        ValueError, "invalid --library-path-map"
                    ):
                        MODULE.drill(
                            1,
                            source_url="postgres://operator@example.invalid/legacy",
                            library_path_maps=[value],
                            canonical_root=canonical,
                            source_backup=canonical / "backup.dump",
                        )
            create_db.assert_not_called()

    def test_baseline_artifact_schema_is_redacted_and_fail_closed(self) -> None:
        baseline = MODULE.validate_baseline_artifact(
            {
                "artifact": MODULE.BASELINE_ARTIFACT,
                "schema_version": MODULE.BASELINE_SCHEMA_VERSION,
                "captured_at": "2026-08-25T00:00:00Z",
                "table_count": MODULE.FROZEN_SOURCE_TABLE_COUNT,
                "schema_fingerprint": "schema-fixture",
                "source_identity": "pg-source-sha256:fixture:schema:schema-fixture",
                "snapshot_marker": f"legacy-marker-v2:{'0' * 64}",
                "source_read_only": {
                    "transaction_read_only": True,
                    "default_transaction_read_only": True,
                    "in_recovery": False,
                },
                "domains": [
                    {"domain": "artists", "rows": 0, "min_pk": None, "max_pk": None}
                ],
                "anomalies": [],
                "media": {"file_count": 0, "total_bytes": 0},
                "configuration_fingerprint": {
                    "algorithm": "sha256-v1",
                    "settings_keys": ["smtp.password"],
                    "settings": [{"key": "smtp.password", "redacted": True}],
                    "library_root_count": 0,
                },
            }
        )
        self.assertEqual(baseline["artifact"], "melodee.legacy-baseline")

        malformed = json.loads(json.dumps(baseline))
        malformed["source_read_only"]["transaction_read_only"] = False
        with self.assertRaisesRegex(ValueError, "not transaction read-only"):
            MODULE.validate_baseline_artifact(malformed)

        malformed = json.loads(json.dumps(baseline))
        malformed["configuration_fingerprint"]["settings"] = [
            {"key": "smtp.password", "value": "secret"}
        ]
        malformed["configuration_fingerprint"]["settings_keys"] = ["smtp.password"]
        with self.assertRaisesRegex(ValueError, "not redacted or hashed"):
            MODULE.validate_baseline_artifact(malformed)

    def test_search_status_requires_activated_authoritative_generation(self) -> None:
        status = {
            "projection": {"latest_rebuild_status": "activated"},
            "generation": {
                "artists": 2,
                "releases": 3,
                "tracks": 4,
                "total": 9,
                "schema_version": 1,
            },
            "generation_error_code": None,
            "generation_error": None,
        }
        self.assertIs(
            MODULE.validate_search_status(
                status,
                {"artists": 2, "releases": 3, "tracks": 4},
            ),
            status,
        )

        malformed = json.loads(json.dumps(status))
        malformed["projection"]["latest_rebuild_status"] = "failed"
        with self.assertRaisesRegex(ValueError, "activated rebuild"):
            MODULE.validate_search_status(
                malformed,
                {"artists": 2, "releases": 3, "tracks": 4},
            )

        malformed = json.loads(json.dumps(status))
        malformed["generation"]["tracks"] = 5
        with self.assertRaisesRegex(ValueError, "tracks count"):
            MODULE.validate_search_status(
                malformed,
                {"artists": 2, "releases": 3, "tracks": 4},
            )

    def test_snapshot_search_probe_comes_from_authority_and_requires_all_classes(self) -> None:
        with patch.object(
            MODULE,
            "psql",
            return_value='["Alpha Band", "Alpha Crew", "東京事変", "AC/DC"]',
        ) as query:
            probe = MODULE.build_search_probe(True, "migrated-target")
        query.assert_called_once()
        self.assertEqual(probe["prefix"], "Alp")
        self.assertEqual(probe["prefix_expected"], "Alpha Band")
        self.assertEqual(
            {case[0] for case in probe["queries"]},
            {"exact", "normalized", "unicode", "punctuation", "broad", "low_selectivity"},
        )
        self.assertIn("Alpha Band", probe["queries"][0][1])
        self.assertNotIn("Alpha Band", MODULE._fingerprint_search_value("Alpha Band"))

    def test_snapshot_search_probe_fails_when_required_representatives_are_absent(self) -> None:
        with patch.object(MODULE, "psql", return_value='["Only Artist"]'):
            with self.assertRaisesRegex(RuntimeError, "Unicode Artist"):
                MODULE.build_search_probe(True)

    def test_synthetic_shape_is_bounded_before_database_creation(self) -> None:
        with self.assertRaisesRegex(ValueError, "between 1 and 10000000"):
            MODULE.validate_drill_config(0)
        with self.assertRaisesRegex(ValueError, "between 1 and 10000000"):
            MODULE.validate_drill_config(10_000_001)

    def test_missing_or_symlink_backup_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "does not exist"):
                MODULE.validate_backup_artifact(root / "missing.dump")

            source = root / "source.dump"
            source.write_bytes(b"not a postgres backup")
            with self.assertRaisesRegex(ValueError, "pg_restore"):
                MODULE.validate_backup_artifact(source)

            link = root / "link.dump"
            link.symlink_to(source)
            with self.assertRaisesRegex(ValueError, "symlink"):
                MODULE.validate_backup_artifact(link)

    @unittest.skipUnless(os.name == "posix", "requires POSIX no-follow support")
    def test_backup_validation_rejects_special_files_without_opening_them(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fifo = Path(directory) / "backup.fifo"
            os.mkfifo(fifo)
            with self.assertRaisesRegex(ValueError, "regular"):
                MODULE.validate_backup_artifact(fifo)

    @unittest.skipUnless(os.name == "posix", "requires POSIX descriptor passing")
    def test_backup_validation_keeps_pg_restore_on_the_held_descriptor(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            backup = Path(directory) / "backup.dump"
            backup.write_bytes(b"stable backup bytes")
            with patch.object(
                MODULE.subprocess,
                "run",
                return_value=SimpleNamespace(
                    returncode=0,
                    stdout="; comment\nTABLE DATA public.example\n",
                ),
            ) as run:
                evidence = MODULE.validate_backup_artifact(backup)
            command = run.call_args.args[0]
            kwargs = run.call_args.kwargs
            self.assertEqual(command[:2], ["pg_restore", "--list"])
            self.assertTrue(command[2].startswith("/proc/self/fd/"))
            self.assertEqual(len(kwargs["pass_fds"]), 1)
            self.assertEqual(evidence["bytes"], len(b"stable backup bytes"))
            self.assertEqual(evidence["toc_entries"], 1)

    def test_existing_snapshot_requires_explicit_mapping_and_media_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "library-path-map"):
                MODULE.validate_snapshot_config(
                    "postgres://operator@example.invalid/legacy",
                    [],
                    root,
                )
            with self.assertRaisesRegex(ValueError, "canonical-root"):
                MODULE.validate_snapshot_config(
                    "postgres://operator@example.invalid/legacy",
                    ["/srv/music=library"],
                    None,
                )

    def test_existing_snapshot_requires_a_validated_source_backup(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "canonical"
            canonical.mkdir()
            with self.assertRaisesRegex(ValueError, "source-backup"):
                MODULE.validate_snapshot_config(
                    "postgres://operator@example.invalid/legacy",
                    ["/srv/music=library"],
                    canonical,
                )

    def test_backup_parent_symlink_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "target"
            target.mkdir()
            (target / "backup.dump").write_bytes(b"not a postgres backup")
            link = root / "link"
            link.symlink_to(target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlink component"):
                MODULE.validate_backup_artifact(link / "backup.dump")

    def test_direct_drill_entrypoint_validates_backup_before_target_creation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            canonical = root / "canonical"
            canonical.mkdir()
            with patch.object(MODULE, "create_db") as create_db:
                with self.assertRaisesRegex(ValueError, "does not exist"):
                    MODULE.drill(
                        1,
                        source_url="postgres://operator@example.invalid/legacy",
                        library_path_maps=["/srv/music=library"],
                        canonical_root=canonical,
                        source_backup=root / "missing.dump",
                    )
            create_db.assert_not_called()

    def test_existing_snapshot_rejects_missing_or_symlink_media_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            missing = root / "missing"
            with self.assertRaisesRegex(ValueError, "existing"):
                MODULE.validate_snapshot_config(
                    "postgres://operator@example.invalid/legacy",
                    ["/srv/music=library"],
                    missing,
                )

            target = root / "target"
            target.mkdir()
            link = root / "link"
            link.symlink_to(target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlink"):
                MODULE.validate_snapshot_config(
                    "postgres://operator@example.invalid/legacy",
                    ["/srv/music=library"],
                    link,
                )

            parent_link = root / "parent-link"
            parent_link.symlink_to(target, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "symlink component"):
                MODULE.validate_snapshot_config(
                    "postgres://operator@example.invalid/legacy",
                    ["/srv/music=library"],
                    parent_link / "nested",
                )

    def test_snapshot_query_failure_redacts_connection_details(self) -> None:
        with patch.object(
            MODULE.subprocess,
            "run",
            return_value=SimpleNamespace(
                returncode=2,
                stdout="",
                stderr="FATAL: password=super-secret",
            ),
        ):
            with self.assertRaisesRegex(RuntimeError, "exit code 2") as context:
                MODULE.source_psql(
                    "postgres://operator:super-secret@example.invalid/legacy",
                    'SELECT COUNT(*) FROM "Artists"',
                )
            self.assertNotIn("super-secret", str(context.exception))
            command = MODULE.subprocess.run.call_args.args[0]
            kwargs = MODULE.subprocess.run.call_args.kwargs
            self.assertEqual(command[:2], ["psql", "-X"])
            self.assertEqual(
                kwargs["env"]["PGOPTIONS"],
                "-c default_transaction_read_only=on",
            )

    def test_metadata_fingerprint_does_not_claim_content_hashing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            media = root / "library" / "track.flac"
            media.parent.mkdir()
            media.write_bytes(b"first")
            before = MODULE.tree_metadata_fingerprint(root)
            content_before = MODULE.tree_fingerprint(root)
            stat = media.stat()
            media.write_bytes(b"other")
            os.utime(media, ns=(stat.st_atime_ns, stat.st_mtime_ns))
            after = MODULE.tree_metadata_fingerprint(root)
            self.assertFalse(before["content_hash_collected"])
            self.assertEqual(before["metadata_fingerprint"], after["metadata_fingerprint"])
            self.assertNotEqual(content_before["entries"], MODULE.tree_fingerprint(root)["entries"])

    def test_disk_budget_fails_closed_before_a_target_is_created(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            usage = SimpleNamespace(total=1_000, used=900, free=100)
            with patch.object(MODULE.shutil, "disk_usage", return_value=usage):
                with self.assertRaisesRegex(ValueError, "between 0 and 1"):
                    MODULE.validate_disk_budget(root, min_headroom_ratio=1.1)
                with self.assertRaisesRegex(RuntimeError, "free-byte budget"):
                    MODULE.validate_disk_budget(root, min_free_bytes=101)
                with self.assertRaisesRegex(RuntimeError, "headroom budget"):
                    MODULE.validate_disk_budget(root, min_headroom_ratio=0.101)
                evidence = MODULE.validate_disk_budget(
                    root,
                    min_free_bytes=100,
                    min_headroom_ratio=0.1,
                )
            self.assertEqual(evidence["minimum_free_bytes"], 100)
            self.assertEqual(evidence["minimum_headroom_ratio"], 0.1)

    def test_negative_disk_budget_is_rejected_without_filesystem_access(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(MODULE.shutil, "disk_usage") as disk_usage:
                with self.assertRaisesRegex(ValueError, "non-negative"):
                    MODULE.validate_disk_budget(Path(directory), min_free_bytes=-1)
            disk_usage.assert_not_called()

    def test_drill_disk_preflight_runs_before_target_creation(self) -> None:
        low_headroom = {
            "path_fingerprint": "test",
            "total_bytes": 1_000,
            "used_bytes": 950,
            "free_bytes": 50,
            "headroom_ratio": 0.05,
        }
        with patch.object(MODULE, "filesystem_metrics", return_value=low_headroom):
            with patch.object(MODULE, "create_db") as create_db:
                with self.assertRaisesRegex(RuntimeError, "free-byte budget"):
                    MODULE.drill(1, min_free_bytes=51)
        create_db.assert_not_called()


if __name__ == "__main__":
    unittest.main()
