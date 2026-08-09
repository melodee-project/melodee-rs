# opensubsonic

Store route inventories, OpenAPI/protocol documents, normalizers, and promoted golden fixtures for
this compatibility surface here. Every fixture must identify its legacy baseline and test coverage.

## Contents (Issue 6)

- `contracts/route-manifest.json` — 164 routes (plain + `.view` forms; includes scan routes).
- `contracts/route-inventory.json` — 81 endpoints classified
  (supported/conditional/gone/not-implemented) with prerequisites and scenario coverage.
  Regenerate: `python3 scripts/build_opensubsonic_inventory.py`; drift check `--check`.
- `golden/` — 177 sanitized captures across system, auth, browse, lists, media, annotation,
  playlists, community, negative (410/501), conditional (podcasts/jukebox), conditional-off
  (podcast disabled). Provenance per directory.
- Findings: `design/evidence/phase-0/opensubsonic-capture.md`. Tests:
  `tests/python/test_opensubsonic_capture.py`.
