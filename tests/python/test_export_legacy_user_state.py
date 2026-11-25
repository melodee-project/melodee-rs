"""Unit tests for the portable C# user-state exporter's pure helpers.

The extraction itself needs a legacy PostgreSQL snapshot; these tests cover the
parts that decide whether an export is safe: timestamp normalization, boolean
and integer parsing, deterministic stream writing with checksums, and output
directory safety.
"""

from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "export_legacy_user_state", ROOT / "scripts" / "export_legacy_user_state.py"
)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
SPEC.loader.exec_module(MODULE)


class CatalogEvidenceTests(unittest.TestCase):
    def test_tracks_referenced_only_by_history_playlists_or_bookmarks_are_included_once(
        self,
    ) -> None:
        ids = [f"00000000-0000-4000-8000-{n:012d}" for n in range(1, 5)]
        streams = {name: [] for name in MODULE.STREAMS}
        streams["entity-states.ndjson"].append(
            MODULE.Row(
                "entity-states.ndjson", {"entity": "track", "entityPublicId": ids[0]}
            )
        )
        for stream, key in zip(
            ("track-plays.ndjson", "playlist-items.ndjson", "bookmarks.ndjson"),
            ids[1:],
            strict=True,
        ):
            streams[stream] = [
                MODULE.Row(stream, {"trackPublicId": ref}) for ref in (key, ids[0])
            ]

        def query(_psql, _url, sql):
            for key in ids:
                self.assertEqual(sql.count(key), 1)
            return [
                [json.dumps({"kind": "track", "publicId": key, "title": "One\nTwo"})]
                for key in reversed(ids)
            ]

        with patch.object(MODULE, "run_query", side_effect=query) as lookup:
            rows = MODULE.export_catalog_evidence("psql", "fixture", streams)
        lookup.assert_called_once()
        self.assertEqual([row.payload["publicId"] for row in rows], ids)
        self.assertTrue(all(row.stream == "catalog-evidence.ndjson" for row in rows))


    def test_missing_metadata_still_lists_the_original_public_id(self):
        key = "00000000-0000-4000-8000-000000000123"
        streams = {name: [] for name in MODULE.STREAMS}
        streams["bookmarks.ndjson"] = [MODULE.Row("bookmarks.ndjson", {"trackPublicId": key})]
        with patch.object(MODULE, "run_query", return_value=[]):
            rows = MODULE.export_catalog_evidence("psql", "fixture", streams)
        self.assertEqual([row.payload for row in rows], [{"kind": "track", "publicId": key}])


class TimestampTests(unittest.TestCase):
    def test_normalizes_offsets_to_utc(self) -> None:
        self.assertEqual(
            MODULE.timestamp("2024-05-01 12:00:00+02", "playedAt"),
            "2024-05-01T10:00:00Z",
        )

    def test_missing_value_is_none(self) -> None:
        self.assertIsNone(MODULE.timestamp(None, "playedAt"))
        self.assertIsNone(MODULE.timestamp("  ", "playedAt"))

    def test_required_timestamp_rejects_missing_value(self) -> None:
        with self.assertRaises(MODULE.ExportError):
            MODULE.required_timestamp(None, "createdAt")

    def test_naive_timestamp_is_rejected(self) -> None:
        with self.assertRaises(MODULE.ExportError):
            MODULE.timestamp("2024-05-01T12:00:00", "createdAt")


class ClampedTimestampTests(unittest.TestCase):
    def test_null_update_falls_back_to_creation_time(self) -> None:
        self.assertEqual(
            MODULE.clamped_timestamps("2024-05-01T12:00:00Z", None, "Users"),
            ("2024-05-01T12:00:00Z", "2024-05-01T12:00:00Z"),
        )

    def test_earlier_update_is_clamped_to_creation_time(self) -> None:
        self.assertEqual(
            MODULE.clamped_timestamps(
                "2024-05-01T12:00:00Z", "2024-04-01T12:00:00Z", "Users"
            ),
            ("2024-05-01T12:00:00Z", "2024-05-01T12:00:00Z"),
        )

    def test_null_creation_uses_update_time(self) -> None:
        self.assertEqual(
            MODULE.clamped_timestamps(None, "2024-05-01T12:00:00Z", "Users"),
            ("2024-05-01T12:00:00Z", "2024-05-01T12:00:00Z"),
        )

    def test_both_null_is_an_actionable_error(self) -> None:
        with self.assertRaises(MODULE.ExportError):
            MODULE.clamped_timestamps(None, None, "UserSongs")

    def test_public_id_requires_non_blank_value(self) -> None:
        self.assertIsNone(MODULE.public_id(None))
        self.assertIsNone(MODULE.public_id("  "))
        self.assertEqual(MODULE.public_id(" 018f0000-0000-7000-8000-000000000001 "), "018f0000-0000-7000-8000-000000000001")


class EntityStateQueryTests(unittest.TestCase):
    def test_play_stat_columns_are_qualified_to_the_state_table(self) -> None:
        query = MODULE.entity_state_query("UserSongs", "SongId", "track", True)
        self.assertIn('s."PlayedCount"', query)
        self.assertIn('s."LastPlayedAt"', query)
        self.assertNotIn('"PlayedCount", "LastPlayedAt"', query)
        self.assertIn('JOIN "Songs" a ON a."Id" = s."SongId"', query)

    def test_tables_without_play_stats_use_typed_null_columns(self) -> None:
        query = MODULE.entity_state_query("UserArtists", "ArtistId", "artist", False)
        self.assertIn("NULL::integer, NULL::timestamptz", query)
        self.assertIn('JOIN "Artists" a ON a."Id" = s."ArtistId"', query)

    def test_every_selected_column_is_qualified(self) -> None:
        query = MODULE.entity_state_query("UserAlbums", "AlbumId", "release", True)
        select_list = query.split(" FROM ", 1)[0]
        columns = [part.strip() for part in select_list.replace("SELECT ", "").split(",")]
        for column in columns:
            self.assertRegex(
                column,
                r"^(u|s|a)\.",
                f"column {column!r} is not qualified with a table alias",
            )


class ValueParsingTests(unittest.TestCase):
    def test_booleans_follow_postgres_text_output(self) -> None:
        self.assertTrue(MODULE.boolean("t"))
        self.assertTrue(MODULE.boolean("true"))
        self.assertFalse(MODULE.boolean("f"))
        self.assertFalse(MODULE.boolean(None))

    def test_integers_default_for_empty_values(self) -> None:
        self.assertEqual(MODULE.integer(None), 0)
        self.assertEqual(MODULE.integer(""), 0)
        self.assertEqual(MODULE.integer("7"), 7)


class StreamTests(unittest.TestCase):
    def test_stream_writes_sorted_deterministic_lines_and_checksum(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "users.ndjson"
            rows = [MODULE.Row("users.ndjson", {"b": 1, "a": 2})]
            count, digest = MODULE.write_stream(path, rows)
            self.assertEqual(count, 1)
            self.assertEqual(
                path.read_text(encoding="utf-8"),
                json.dumps({"a": 2, "b": 1}, sort_keys=True, separators=(",", ":")) + "\n",
            )
            second = Path(directory) / "second.ndjson"
            _, second_digest = MODULE.write_stream(second, rows)
            self.assertEqual(digest, second_digest)

    def test_prepare_output_refuses_non_empty_directory_without_force(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            (target / "keep.txt").write_text("data", encoding="utf-8")
            with self.assertRaises(MODULE.ExportError):
                MODULE.prepare_output(target, force=False)
            (target / "users.ndjson").write_text("stale", encoding="utf-8")
            MODULE.prepare_output(target, force=True)
            self.assertFalse((target / "users.ndjson").exists())
            self.assertTrue((target / "keep.txt").exists())


class ManifestShapeTests(unittest.TestCase):
    def test_reference_only_tables_all_have_a_reason(self) -> None:
        for table, reason in MODULE.REFERENCE_ONLY_TABLES.items():
            with self.subTest(table=table):
                self.assertTrue(table)
                self.assertGreaterEqual(len(reason.strip()), 10)

    def test_stream_names_are_unique(self) -> None:
        self.assertEqual(len(MODULE.STREAMS), len(set(MODULE.STREAMS)))


if __name__ == "__main__":
    unittest.main()
