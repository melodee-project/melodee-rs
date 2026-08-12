# Phase 7 Compatibility Acceptance Map

This map separates the frozen legacy contract from Rust acceptance evidence. A Phase 0 golden proves
what the legacy server did; it does not prove that the Phase 7 adapter implements that behavior.
Route-level status is generated in `design/agents/PHASE-7-ROUTE-MATRIX.json` from the legacy inventory
and `compatibility/melodee-api-v1/contracts/phase7-route-evidence.json`.

## Evidence states

| State | Meaning | May close a route? |
|---|---|---:|
| Legacy captured | A sanitized oracle scenario records the frozen response. | No |
| Rust test | A route-level Rust test proves handler behavior. | No, not alone |
| Differentially tested | Legacy scenario and Rust route test are linked in the evidence registry. | Yes, for exact parity |
| Approved normalization | Legacy defect and intended Rust result are both tested under an accepted ADR. | Yes |
| Preserved absence | Legacy negative runtime behavior and Rust negative routing behavior are both tested. | Yes |
| Pending | Required capture, implementation, or test evidence is missing. | No |

## Acceptance contracts

| ID | Requirements | Acceptance contract | Legacy evidence | Rust evidence required | Current state |
|---|---|---|---|---|---|
| P7-A01 | PRD-API-M1-001, PRD-API-M1-006 | All 202 manifest method/path rows have one Phase 7 status; OpenAPI is not treated as sole authority. | `route-manifest.json`, `route-inventory.json`, `native-client-usage.json` | Generated matrix drift test | Satisfied — 202 complete, 0 pending/deferred |
| P7-A02 | PRD-API-M1-002, PRD-API-M1-005 | Method/path, parameter names, body shape, status, relevant headers, exact JSON types/casing, null/omission, pagination, ordering, and URLs match or carry an approved normalization. | Native golden metadata and bodies | Route integration test plus differential comparison | Satisfied — 150 exact and 17 ADR-approved routes |
| P7-A03 | PRD-API-M1-003 | Compatibility DTOs retain `Album`/`Song`; core modules retain `Release`/`Track`. | OpenAPI schemas and native goldens | DTO serialization tests and vocabulary boundary checks | Satisfied — compatibility DTO/core boundary compiled and tested |
| P7-A04 | PRD-API-M1-004, PRD-PLAYLIST-003 | Migrated User/Artist/Release/Track/Playlist/Share public IDs round-trip unchanged and collision-safe. | Deterministic public IDs in fixtures/goldens | Migration, constraint, and API round-trip tests under ADR-0127 | Satisfied for Phase 7 schema/API; production cutover remains Phase 11 |
| P7-A05 | PRD-AUTH-001..007 | Password migration, login, JWT, cookie/social/reset where exposed, refresh rotation/replay/family revoke, logout/revoke, roles, library ACLs, rate limiting, and secret-safe diagnostics obey ADR-0124/0126. | `golden/native-auth` and auth scenarios | Unit, PostgreSQL concurrency/fault, HTTP differential, and log-redaction tests | Satisfied for native routes; future general config export/import remains outside this surface |
| P7-A06 | PRD-API-M1-002..006 | Artist/Album/Song browse/detail/search/image DTOs preserve public behavior and use bounded projection-backed queries. | `golden/native-catalog` | HTTP differential tests, SQL query-count/bounds tests, Tantivy integration | Satisfied — catalog/ACL/image tests plus 560k harness |
| P7-A07 | PRD-PLAYLIST-001..003 | Playlist CRUD, item order, smart rules, visibility, image behavior, and IDs survive migration; both ADR-0114 add-item shapes work atomically. | Playlist golden flows including both broken legacy bodies | Migration, transaction/fault, DTO, and approved-normalization tests | Satisfied for native-v1; other adapters remain Phase 8 |
| P7-A08 | PRD-USER-001..005 | Preferences, favorite/rating/hate/pin state, now-playing, scrobble, queue contract, and statistics preserve captured semantics except ADR-0128 fixes. | Catalog star/rating/scrobble/queue flows | PostgreSQL state/integrity tests and HTTP differentials | Satisfied — current Queue absence is also captured and tested |
| P7-A09 | PRD-STREAM-001..003 | Exact HMAC URL contract, HEAD/full/range behavior, content headers, cancellation/concurrency, and approved credential-error normalizations follow ADR-0125/0128. | `golden/native-stream` | Token vectors, range matrix, path safety, disconnect, concurrent playback, and HTTP differential tests | Satisfied for native direct play and real-client gate; cross-adapter transcoding remains later work |
| P7-A10 | PRD-REQUEST-001..002, PRD-SHARE-001..003, PRD-POD-001..003, PRD-DISCOVERY-001..003 | Every compiled native requests/shares/podcasts/analytics/chart/recommendation route is implemented in Phase 7; Phase 9 owns only non-native domain breadth. | Inventory/client-use evidence; missing scenarios must be captured | Route, persistence, authorization, pagination, and differential tests under ADR-0123 | Satisfied — every compiled native route has linked evidence |
| P7-A11 | PRD-PARTY-001..004, PRD-USER-004 | All 32 Party and three Queue legacy-build exclusions preserve the observed negative routing behavior unless superseded. | Promoted `native-excluded` captures for all 35 method/path rows | Exact per-route Rust negative test | Satisfied — 35 promoted negatives and exact Rust method/path test |
| P7-A12 | PRD-STREAM-003 | Android phone/Auto and MeloAmp perform their designated login, browse, search, stream, seek, refresh/restore, scrobble, playlist, and process-restart workflows. | Pinned client source inventory | Reproducible real-client smoke report with versions/results | Satisfied with documented emulator/no-DHU limitation |
| P7-A13 | PRD-API-M1-005, PRD-AUTH-005..006 | API lists are bounded, stream startup avoids catalog scans, auth/rate controls are bounded, errors/logs are secret-safe, and correlation is present. | Phase 0 benchmark/capture conventions | Route latency/query-count/load/security harnesses | Satisfied — current-catalog, ACL, rate, stream and secret-safety tests |
| P7-A14 | All Phase 7 requirements | Public docs, configuration, migration/rollback, traceability, changelog, full repository gate, docs build, and clean diff are complete. | Definition of Done | Recorded final commands and outputs | Satisfied — final independent review and all repository/docs gates pass |

## Route evidence workflow

1. Capture any missing legacy scenario before changing behavior.
2. Add the lowest useful Rust unit/integration/fault test and an HTTP compatibility test.
3. For exact parity, compare a selected scenario without requiring an entire category capture:

   ```bash
   python3 scripts/compare_captures.py \
     compatibility/melodee-api-v1/golden/native-catalog \
     /path/to/rust-capture \
     --scenario catalog/artists-list
   ```

4. For an approved normalization, keep the legacy result as evidence and assert the exact new result
   in an ADR-linked Rust test; never widen the baseline normalization masks.
5. Add the method/path, requirement IDs, legacy scenario names, Rust test names, and authority to
   `phase7-route-evidence.json`.
6. Regenerate and check the matrix:

   ```bash
   python3 scripts/build_phase7_route_matrix.py
   python3 scripts/build_phase7_route_matrix.py --check
   ```

The matrix validator rejects unknown routes, duplicate rows, evidence without route-specific legacy
scenarios, missing Rust tests, invalid requirement IDs, excluded routes claimed as implemented, and
normalization/deferral authority that is not an accepted ADR.
