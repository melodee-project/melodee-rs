"""Reviewing a candidate is distinct from applying that listener's legacy choices."""

import contextlib
import io
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from uuid import uuid4

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))
import reconcile_legacy_user_state as reconcile


class ReviewTest(unittest.TestCase):
    def test_ambiguous_tracks_require_choice_and_remember_it_after_restart(self):
        with (
            tempfile.TemporaryDirectory() as temp,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            path = Path(temp) / "approvals.json"
            db = str(uuid4())
            source = dict(
                kind="track",
                publicId=str(uuid4()),
                artist="Same",
                release="Title",
                title="Recording",
                trackNumber=1,
                durationMs=135000,
            )
            candidates = [
                source | {"publicId": str(uuid4()), "durationMs": duration}
                for duration in (134000, 136000, 65000)
            ]
            index = {reconcile.match_key(source): candidates}
            result = reconcile.read_map(path, db)
            reconcile.reconcile([source], set(), index, result, path)
            self.assertFalse(path.exists(), "a preview must not approve either edition")
            with patch("builtins.input", return_value="2"):
                reconcile.reconcile([source], set(), index, result, path, review=True)
            stored = reconcile.read_map(path, db)
            expected = max(row["publicId"] for row in candidates[:2])
            self.assertEqual(
                stored["approved"],
                [dict(kind="track", legacyId=source["publicId"], targetId=expected)],
            )
            with patch(
                "builtins.input",
                side_effect=AssertionError("must reuse previous approval"),
            ):
                reconcile.reconcile([source], set(), index, stored, path, review=True)
            self.assertEqual(stored, reconcile.read_map(path, db))
            with self.assertRaisesRegex(ValueError, "installation"):
                reconcile.read_map(path, str(uuid4()))

    def test_email_match_skips_review_but_username_collision_offers_both_accounts(self):
        old = dict(
            kind="user",
            publicId=str(uuid4()),
            username="old",
            email="Person@Example.Test",
        )
        current = old | {"publicId": str(uuid4()), "username": "new"}
        index = {("email", "PERSON@EXAMPLE.TEST"): [current]}
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "links.json"
            data = reconcile.read_map(path, str(uuid4()))
            with (
                contextlib.redirect_stdout(io.StringIO()) as output,
                patch(
                    "builtins.input", side_effect=AssertionError("no approval needed")
                ),
            ):
                reconcile.reconcile([old], set(), index, data, path, review=True)
            self.assertIn(
                "accounts automatically matched by email: 1", output.getvalue()
            )
            self.assertFalse(path.exists())
            index[("username", "OLD")] = [
                old | {"publicId": str(uuid4()), "email": "another@example.test"}
            ]
            with (
                contextlib.redirect_stdout(io.StringIO()) as output,
                patch("builtins.input", return_value="") as prompt,
            ):
                reconcile.reconcile([old], set(), index, data, path, review=True)
            prompt.assert_called_once()
            self.assertIn("multiple candidates: 1", output.getvalue())

    def test_old_export_can_review_users_without_catalog_evidence(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            name = "users.ndjson"
            user = {
                "publicId": str(uuid4()),
                "username": "listener",
                "email": "listener@example.test",
            }
            raw = (json.dumps(user) + "\n").encode()
            (root / name).write_bytes(raw)
            (root / "manifest.json").write_text(
                json.dumps(
                    {
                        "schemaVersion": 1,
                        "checksums": {name: hashlib.sha256(raw).hexdigest()},
                        "counts": {name: 1},
                    }
                )
            )
            with contextlib.redirect_stderr(io.StringIO()) as note:
                self.assertEqual(
                    reconcile.sources_from_export(root), [user | {"kind": "user"}]
                )
            self.assertIn("accounts only", note.getvalue())

    def test_exporter_placeholder_does_not_identify_a_person(self):
        self.assertFalse(
            reconcile.usable_account_email("MIGRATED-USER-12@IMPORT.LOCAL")
        )
        self.assertTrue(reconcile.usable_account_email("REAL.PERSON@IMPORT.LOCAL"))

    def test_exact_title_triple_with_missing_position_and_duration_skips_review(self):
        source = dict(
            kind="track",
            publicId=str(uuid4()),
            artist="The Artist",
            release="The Release",
            title="The Track",
        )
        target = source | {
            "publicId": str(uuid4()),
            "trackNumber": 2,
            "durationMs": 120000,
        }
        with (
            tempfile.TemporaryDirectory() as temp,
            contextlib.redirect_stdout(io.StringIO()),
        ):
            path = Path(temp) / "links.json"
            data = reconcile.read_map(path, str(uuid4()))
            index = {reconcile.match_key(target): [target]}
            with patch(
                "builtins.input",
                side_effect=AssertionError("exact track should match automatically"),
            ):
                reconcile.reconcile(
                    [source],
                    set(),
                    index,
                    data,
                    path,
                    review=True,
                    exact_tracks={source["publicId"]: target["publicId"]},
                )
            self.assertFalse(path.exists())

    def test_competing_reviews_do_not_overwrite_existing_decisions(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "approvals.json"
            with (
                reconcile.exclusive_review(path),
                self.assertRaisesRegex(ValueError, "another review"),
            ):
                with reconcile.exclusive_review(path):
                    self.fail("second lock acquired")
            with reconcile.exclusive_review(path):
                self.assertFalse(path.exists())

    def test_check_export_integrity_before_suggesting_a_link(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            name = "catalog-evidence.ndjson"
            (directory / name).write_text(json.dumps({"kind": "track"}) + "\n")
            manifest = {"counts": {name: 1}, "checksums": {name: "wrong"}}
            with self.assertRaisesRegex(ValueError, "manifest"):
                reconcile.checked_rows(directory, manifest, name)

    def test_database_url_keeps_password_out_of_process_arguments(self):
        url, env = reconcile.connection(
            "postgresql://listener:f%40ke@host:5432/db?options=-c%20default_transaction_read_only%3Don"
        )
        self.assertEqual(env["PGPASSWORD"], "f@ke")
        self.assertEqual(
            url,
            "postgresql://listener@host:5432/db?options=-c%20default_transaction_read_only%3Don",
        )
