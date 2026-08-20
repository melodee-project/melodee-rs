"""Safety checks for the isolated Phase 11 rehearsal harness."""
from __future__ import annotations

import importlib.util
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

    def test_malformed_database_url_fails_without_echoing_credentials(self) -> None:
        with self.assertRaisesRegex(ValueError, "database URL") as context:
            MODULE.validate_database_url(
                "postgres://operator:super-secret@example.invalid"
            )
        self.assertNotIn("super-secret", str(context.exception))

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
            with self.assertRaisesRegex(ValueError, "regular"):
                MODULE.validate_backup_artifact(link)

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
            with self.assertRaisesRegex(ValueError, "non-symlink"):
                MODULE.validate_snapshot_config(
                    "postgres://operator@example.invalid/legacy",
                    ["/srv/music=library"],
                    link,
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
