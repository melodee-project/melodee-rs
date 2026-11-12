"""Guard the known-answer corpus UI artefacts against breaking the Admin UI gate.

The Playwright known-answer suite consumed a generated TypeScript module that is
deliberately untracked, which made `tsc` (and therefore the Admin UI workflow)
depend on a file CI never has. The suite now reads JSON at runtime, and these
tests pin the two properties that keep that safe:

1. the emitter writes JSON (not a module that must exist at compile time); and
2. the spec has no static import of a generated artefact.
"""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "verify_known_corpus", ROOT / "scripts" / "verify_known_corpus.py"
)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
SPEC.loader.exec_module(MODULE)

SPEC_SOURCE = ROOT / "admin-ui" / "e2e" / "live" / "known-corpus-views.spec.ts"
SESSION_SOURCE = ROOT / "admin-ui" / "e2e" / "live" / "session.ts"


class KnownCorpusUiArtefactTests(unittest.TestCase):
    def test_emitter_writes_json_the_suite_can_read_without_compiling_it(self) -> None:
        expected = {
            "runId": "unit",
            "releases": [
                {
                    "slug": "fixture",
                    "purpose": "test",
                    "album": "Fixture Album",
                    "folder": "Fixture Artist - Fixture Album (2020) [unit]",
                    "displayArtist": "Fixture Artist",
                    "trackCount": 1,
                    "artwork": {"present": True},
                    "tracks": [{"title": "One", "artist": "Fixture Artist"}],
                    "expect": {"terminalStates": ["indexed"]},
                }
            ],
        }
        original = MODULE.release_row
        MODULE.release_row = lambda url, album, artist: {  # noqa: ARG005
            "public_api_id": "01a1d15b-0000-7000-8000-000000000000"
        }
        try:
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "known-corpus.generated.json"
                verifier = MODULE.Verifier(url="postgres://unused", api=None)
                MODULE.emit_ui_expectations(path, expected, verifier, "postgres://unused")
                payload = json.loads(path.read_text())
        finally:
            MODULE.release_row = original

        self.assertEqual(payload["runId"], "unit")
        self.assertEqual(len(payload["releases"]), 1)
        release = payload["releases"][0]
        self.assertEqual(release["album"], "Fixture Album")
        self.assertEqual(release["releaseId"], "01a1d15b-0000-7000-8000-000000000000")
        self.assertTrue(release["hasArtwork"])
        self.assertEqual(release["tracks"][0]["title"], "One")

    def test_spec_reads_expectations_at_runtime_instead_of_importing_them(self) -> None:
        source = SPEC_SOURCE.read_text()
        self.assertNotIn("from './known-corpus.generated'", source)
        self.assertNotIn('from "./known-corpus.generated"', source)
        self.assertIn("known-corpus.generated.json", source)
        # A missing artefact must skip the suite, never fail the gate.
        self.assertIn("readFileSync", source)
        self.assertIn("catch", source)

    def test_live_session_state_is_checked_against_the_configured_host(self) -> None:
        source = SESSION_SOURCE.read_text()
        self.assertIn("expectedHost", source)
        self.assertIn("forThisHost", source)


if __name__ == "__main__":
    unittest.main()
