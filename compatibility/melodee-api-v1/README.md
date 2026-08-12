# melodee-api-v1

Store route inventories, OpenAPI/protocol documents, normalizers, and promoted golden fixtures for
this compatibility surface here. Every fixture must identify its legacy baseline and test coverage.

## Contents

- `contracts/route-manifest.json` — static route inventory from the frozen legacy checkout
  (`4028b58d`), including `excluded-from-build` routes (`<Compile Remove>` in the legacy csproj).
- `contracts/openapi-v1.json` — captured `/openapi/v1.json` (known-incomplete; one input, not the
  authority).
- `contracts/native-client-usage.json` — routes actually used by melodee-player and MeloAmp at
  pinned client commits.
- `contracts/route-inventory.json` — unified per-route inventory (manifest × OpenAPI × client
  usage × scenario coverage × parity status). Regenerate:
  `python3 scripts/build_native_inventory.py`; drift check with `--check`.
- `contracts/phase7-route-evidence.json` — Rust implementation/differential evidence registry.
  This is intentionally separate from the legacy inventory: a captured legacy route is not an
  implemented Rust route. Regenerate the Phase 7 matrix with
  `python3 scripts/build_phase7_route_matrix.py`; drift check with `--check`.
- `golden/` — promoted, sanitized runtime evidence for the native v1 critical path:
  - `golden/native-auth/` — authentication lifecycle (login, rotation, replay, logout,
    negative paths). Token values masked; structure validated via `structureChecks`.
  - `golden/native-catalog/` — browse/detail/search/playlists/queue/star/rating/scrobble/images
    against the deterministic fixture catalog.
  - `golden/native-stream/` — stream URL shape, HMAC semantics, HEAD/GET/range matrix, negative
    auth and range behavior.
  - Each directory carries `provenance.json` (baseline commit, fixture catalog hash, recreation
    procedure). Coverage and secret-scan tests: `tests/python/test_native_v1_capture.py`.

Golden fixtures preserve legacy `Album`/`Song` vocabulary exactly as served (PRD-API-M1-003).
