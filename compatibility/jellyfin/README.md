# jellyfin

Store route inventories, OpenAPI/protocol documents, normalizers, and promoted golden fixtures for
this compatibility surface here. Every fixture must identify its legacy baseline and test coverage.

## Contents (Issue 7)

- `contracts/route-manifest.json` — 58 manifest entries (60 method routes).
- `contracts/route-inventory.json` — 60 routes with routing rules, auth model, and scenario
  coverage (100%). Regenerate: `python3 scripts/build_jellyfin_inventory.py`; `--check`.
- `golden/` — sanitized captures across discovery, auth, browse, playback, userstate,
  playlists, images, extra. Provenance per directory.
- Findings: historical artifact removed. Tests:
  `tests/python/test_jellyfin_capture.py`.
