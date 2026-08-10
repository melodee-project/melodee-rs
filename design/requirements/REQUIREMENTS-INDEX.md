# Requirements Index

Generated from `PRD.md` by `scripts/extract_requirements.py`. Do not edit manually.

Total requirements: **232**

| Requirement | Title | Planned phase | Status | Implementation evidence | Test evidence |
|---|---|---:|---|---|---|
| `PRD-PRINCIPLE-001` | Behavioral compatibility over implementation similarity | all | planned | — | — |
| `PRD-PRINCIPLE-002` | Existing external contracts are first-class product features | all | planned | — | — |
| `PRD-PRINCIPLE-003` | The native Melodee API is a core product surface | all | planned | — | — |
| `PRD-PRINCIPLE-004` | OpenSubsonic and native Melodee APIs serve different purposes | all | planned | — | — |
| `PRD-PRINCIPLE-005` | Melodee owns identity | all | planned | — | — |
| `PRD-PRINCIPLE-006` | Uncertainty must not become destructive behavior | all | planned | — | — |
| `PRD-PRINCIPLE-007` | FOSS-only core stack | all | planned | — | — |
| `PRD-PRINCIPLE-008` | Architecture decisions are durable records | all | planned | — | — |
| `PRD-SCALE-001` | PRD-SCALE-001 | 0,6 | planned | — | — |
| `PRD-SCALE-002` | PRD-SCALE-002 | 0,6 | planned | — | — |
| `PRD-SCALE-003` | PRD-SCALE-003 | 0,6 | planned | — | — |
| `PRD-ARCH-001` | Modular monolith first | 1 | planned | — | — |
| `PRD-ARCH-002` | Compile-time module boundaries | 1 | planned | — | — |
| `PRD-ARCH-003` | PostgreSQL is the authoritative state store | 1 | planned | — | — |
| `PRD-ARCH-004` | Tantivy is a projection | 1 | planned | — | — |
| `PRD-ARCH-005` | Filesystem is canonical for media bytes, not identity | 1 | planned | — | — |
| `PRD-ARCH-006` | Avoid unnecessary infrastructure | 1 | planned | — | — |
| `PRD-INGEST-001` | Per-Release retry | 2 | implemented | crates/melodee-domain/src/ingestion.rs (RetryPolicy, FailureClass); crates/melodee-persistence/src/work_items.rs (record_failure, claim-time failed->ready requeue) | crates/melodee-persistence/tests/claiming.rs (future_next_attempt_blocks_claim_until_eligible, exhausted_retries_are_not_claimed); crates/melodee-persistence/tests/recovery.rs (permanent/review failure classes); crates/melodee-domain unit tests (retry backoff) |
| `PRD-INGEST-002` | Lease-based concurrency | 2 | implemented | crates/melodee-persistence/src/work_items.rs (claim with FOR UPDATE SKIP LOCKED, leases, recovery, renewal); crates/melodee-persistence/migrations/0001_release_work_items.sql | crates/melodee-persistence/tests/claiming.rs (exclusive claim, 1000x32 parallel queue, poll race, lease blocking/expiry); crates/melodee-persistence/tests/recovery.rs (crash simulations) |
| `PRD-INGEST-003` | Watchers are accelerators only | 2 | tested | crates/melodee-ingestion/src/reconcile.rs (periodic reconciliation drives source of truth); crates/melodee-ingestion/src/scheduler.rs (optional periodic reconciliation loop); apps/melodee-server/src/main.rs (periodic reconciler startup and shutdown) | crates/melodee-ingestion/tests/reconciliation.rs (lost_watcher_notification_is_recovered_by_scan); crates/melodee-ingestion/tests/reconciliation.rs (two_hundred_releases_discover_and_settle_without_duplicates); crates/melodee-ingestion/tests/reconciliation.rs (concurrent_reconcilers_do_not_create_duplicates) |
| `PRD-INGEST-004` | Settling detection | 2 | tested | crates/melodee-ingestion/src/snapshot.rs (deterministic tree signature); crates/melodee-ingestion/src/reconcile.rs (discovered->settling->ready transitions and ready marker shortcut); crates/melodee-ingestion/src/source.rs (candidate discovery and boundary filtering) | crates/melodee-ingestion/tests/reconciliation.rs (settling_requires_quiet_period_without_marker); crates/melodee-ingestion/tests/reconciliation.rs (changed_snapshot_reverts_ready_to_settling); crates/melodee-ingestion/tests/reconciliation.rs (ready_marker_shortcuts_settling); crates/melodee-ingestion/tests/reconciliation.rs (two_hundred_releases_discover_and_settle_without_duplicates) |
| `PRD-INGEST-005` | Source safety | 2 | tested | crates/melodee-ingestion/src/config.rs (inbound roots canonicalized, write-paths never touched); crates/melodee-ingestion/src/reconcile.rs (no source moves/renames/deletions during discovery); crates/melodee-ingestion/src/source.rs (safe symlink and path traversal filtering) | crates/melodee-ingestion/tests/reconciliation.rs (missing_candidate_is_not_deleted); crates/melodee-ingestion/tests/reconciliation.rs (snapshot_error_for_disappeared_directory_is_survivable); crates/melodee-ingestion/tests/reconciliation.rs (symlink_candidates_are_safe); crates/melodee-ingestion/tests/reconciliation.rs (directory_discovery_ignores_non_directories_and_invalid_candidates) |
| `PRD-INGEST-006` | Safe staging/publishing | 2 | planned | — | — |
| `PRD-INGEST-007` | Versioned manifest | 2 | tested | crates/melodee-domain/src/manifest.rs (schema-versioned ReleaseManifest and semantic hash input); crates/melodee-persistence/migrations/0004_release_work_item_manifests.sql; crates/melodee-persistence/migrations/0005_normalization_provenance.sql; crates/melodee-persistence/src/work_items.rs (transactional manifest publication, claim_normalizing, publish_normalization); crates/melodee-media/src/lib.rs (deterministic media observations); crates/melodee-media/src/normalize.rs (normalization observations embedded in manifest) | crates/melodee-media/src/lib.rs (manifest_hash_excludes_volatile_provenance); crates/melodee-persistence/tests/manifests.rs; crates/melodee-persistence/tests/normalization.rs; crates/melodee-ingestion/tests/inspection.rs |
| `PRD-INGEST-008` | Structural validity is independent of enrichment | 2 | tested | crates/melodee-media/src/lib.rs (structured findings and tag-independent ffprobe validation); crates/melodee-media/src/normalize.rs (NormalizationReport.findings aggregate findings across audio, artwork, and sidecars) | crates/melodee-media/src/lib.rs (valid_tagless_wav_is_inspected_as_audio); crates/melodee-ingestion/tests/inspection.rs |
| `PRD-INGEST-009` | Bounded resource pools | 2 | tested | crates/melodee-ingestion/src/config.rs (explicit inspection concurrency and ffprobe timeout); crates/melodee-ingestion/src/config.rs (normalization concurrency, ffmpeg/fpcalc timeouts, work-area base); crates/melodee-ingestion/src/inspect.rs (bounded claim batch and spawn_blocking boundary); crates/melodee-ingestion/src/normalize.rs (bounded normalization concurrency with lease heartbeat); crates/melodee-media/src/lib.rs (streaming hashes and bounded tool output/artwork limits) | crates/melodee-ingestion/tests/inspection.rs; crates/melodee-persistence/tests/normalization.rs |
| `PRD-INGEST-010` | Operator visibility | 2 | tested | crates/melodee-persistence/migrations/0002_release_work_item_events.sql; apps/melodee-cli/src/main.rs (work show prints state, attempts, last error, event history); apps/melodee-server/src/admin.rs (Admin diagnostics API: state counts, bounded listing, work-item detail, event history, active leases); apps/melodee-server/src/main.rs (admin diagnostics routes wired into the server router) | crates/melodee-persistence/tests/history.rs; apps/melodee-server/tests/admin_diagnostics_api.rs (13 integration tests against real PostgreSQL) |
| `PRD-MEDIA-001` | PRD-MEDIA-001 | 3 | captured | compatibility/media/baseline.json + cases/ (31 synthetic cases: formats, metadata, conversion, artwork, validation, classification, failures); scripts/generate_media_corpus.py; scripts/capture_media_baseline.py; design/evidence/phase-0/media-baseline.md | tests/python/test_media_baseline.py |
| `PRD-MEDIA-002` | Canonical format policy | 3 | tested | crates/melodee-media/src/normalize.rs (canonical MP3 encoding via FFmpeg libmp3lame VBR q4, 384k max, 48kHz); crates/melodee-media/src/normalize.rs (is_canonical_mp3 retention: MP3 at 48kHz, <=384k, channels preserved); crates/melodee-persistence/migrations/0005_normalization_provenance.sql (provenance columns); crates/melodee-persistence/src/work_items.rs (publish_normalization with manifest update + Validating transition); crates/melodee-ingestion/src/normalize.rs (normalization worker with lease heartbeat) | crates/melodee-persistence/tests/normalization.rs (claim_normalizing, publish_normalization, stale lease rejection); crates/melodee-ingestion/src/normalize.rs (classify_normalization_error unit tests); crates/melodee-media/src/normalize.rs (is_canonical_mp3, output_name_replaces_extension); compatibility/media/baseline.json environment.seededSettings (legacy conversion profile preserved as evidence) |
| `PRD-MEDIA-003` | Use mature media tools | 3 | captured | design/evidence/phase-0/media-baseline.md section 12 (eight classified legacy deficiencies incl. F1 metadata-container gate, F2 destructive same-size dedupe, no resumability, no crash-safety) | tests/python/test_media_baseline.py (test_metadata_container_gate, test_same_size_deletion_bug) |
| `PRD-MEDIA-004` | Metadata reading | 3 | tested | crates/melodee-media/src/lib.rs (Lofty native observation plus ffprobe technical fallback) | crates/melodee-media/src/lib.rs; crates/melodee-ingestion/tests/inspection.rs |
| `PRD-MEDIA-005` | Artwork | 3 | tested | crates/melodee-media/src/normalize.rs (extract and normalize embedded and external artwork via FFmpeg to JPEG, bounded by max_artwork_bytes, max dimension 1600px); crates/melodee-media/src/normalize.rs (BLAKE3 byte hash of normalized output, work-area path containment via normalize_path) | crates/melodee-media/src/normalize.rs (ensure_within_accepts_nested_paths, ensure_within_rejects_escape) |
| `PRD-MEDIA-006` | Sidecar files | 3 | tested | crates/melodee-media/src/normalize.rs (parse_sidecar_content: CUE, M3U, SFV, NFO, LRC structured parsing); crates/melodee-media/src/normalize.rs (ParsedSidecar domain type with findings); crates/melodee-domain/src/manifest.rs (ParsedSidecar domain type) | crates/melodee-media/src/normalize.rs (parse_m3u_extracts_entries, parse_sfv_extracts_crc32_entries, parse_lrc_extracts_timed_lines, parse_nfo_text_vs_xml, parse_cue_extracts_tracks) |
| `PRD-FP-001` | Compute once, reuse | 3,5 | tested | crates/melodee-media/src/lib.rs (streaming BLAKE3 byte hashes in ReleaseManifest); crates/melodee-media/src/normalize.rs (run_fpcalc: acoustic fingerprint via fpcalc -json, source audio file fingerprinting) | crates/melodee-media/src/lib.rs (byte_hash_distinguishes_same_size_content); crates/melodee-ingestion/tests/inspection.rs; crates/melodee-persistence/tests/normalization.rs |
| `PRD-FP-002` | Distinguish byte identity from audio identity | 3,5 | tested | crates/melodee-media/src/normalize.rs (FingerprintObservation: source-relative path, Chromaprint fingerprint, duration_ms); crates/melodee-domain/src/manifest.rs (FingerprintObservation domain type); crates/melodee-domain/src/manifest.rs (byte hash is an observation, not identity resolution) | crates/melodee-media/src/lib.rs (byte_hash_distinguishes_same_size_content) |
| `PRD-FP-003` | Acoustic fingerprinting | 3,5 | tested | crates/melodee-media/src/normalize.rs (run_fpcalc: fpcalc -json -length for Chromaprint fingerprint + duration); crates/melodee-media/src/normalize.rs (NormalizationOptions.fpcalc_path/fpcalc_timeout); crates/melodee-ingestion/src/config.rs (ENV_FPCALC_PATH, ENV_FPCALC_TIMEOUT_SECONDS, normalization_concurrency) | crates/melodee-persistence/tests/normalization.rs |
| `PRD-FP-004` | Underlying audio identity abstraction | 3,5 | tested | crates/melodee-media/src/normalize.rs (FingerprintObservation with source_file + fingerprint + duration_ms); crates/melodee-media/src/normalize.rs (ManifestFingerprintObservations: aggregate fingerprints in NormalizationObservations); crates/melodee-domain/src/manifest.rs (FingerprintObservation domain type) | crates/melodee-persistence/tests/normalization.rs |
| `PRD-FP-005` | Physical byte deduplication is optional for parity | 3,5 | implemented | crates/melodee-media/src/normalize.rs (NormalizedOutput with byte_hash; retention of already-canonical sources without re-encoding) | crates/melodee-media/src/normalize.rs (is_canonical_mp3_retains_canonical_source) |
| `PRD-ARTIST-001` | Names are not unique | 4 | planned | — | — |
| `PRD-ARTIST-002` | Stable immutable identity | 4 | planned | — | — |
| `PRD-ARTIST-003` | Artist types remain identity-oriented | 4 | planned | — | — |
| `PRD-ARTIST-004` | Aliases mean same identity | 4 | planned | — | — |
| `PRD-ARTIST-005` | External identities are normalized rows | 4 | planned | — | — |
| `PRD-ARTIST-006` | Relationships do not imply equality | 4 | planned | — | — |
| `PRD-ARTIST-007` | Date-bounded group membership | 4 | planned | — | — |
| `PRD-ARTIST-008` | User decisions outrank providers | 4 | planned | — | — |
| `PRD-ARTIST-009` | Negative identity assertions | 4 | planned | — | — |
| `PRD-ARTIST-010` | Safe merge/split | 4 | planned | — | — |
| `PRD-ARTIST-011` | Bias against false merges | 4 | planned | — | — |
| `PRD-ARTIST-012` | Do not port the current external-authority identity model | 4 | planned | — | — |
| `PRD-RESOLVE-001` | PRD-RESOLVE-001 | 4 | planned | — | — |
| `PRD-RESOLVE-002` | PRD-RESOLVE-002 | 4 | planned | — | — |
| `PRD-RESOLVE-003` | PRD-RESOLVE-003 | 4 | planned | — | — |
| `PRD-RESOLVE-004` | PRD-RESOLVE-004 | 4 | planned | — | — |
| `PRD-RESOLVE-005` | PRD-RESOLVE-005 | 4 | planned | — | — |
| `PRD-CREDIT-001` | ReleaseArtist | 4 | planned | — | — |
| `PRD-CREDIT-002` | TrackArtist | 4 | planned | — | — |
| `PRD-CREDIT-003` | Contribution | 4 | planned | — | — |
| `PRD-CREDIT-004` | Preserve DisplayArtist | 4 | planned | — | — |
| `PRD-CREDIT-005` | Parsing does not create identity | 4 | planned | — | — |
| `PRD-CREDIT-006` | “Everything this Artist is on” is first-class | 4 | planned | — | — |
| `PRD-CREDIT-007` | Direct vs inferred participation | 4 | planned | — | — |
| `PRD-CREDIT-008` | Materialized relationship projection | 4 | planned | — | — |
| `PRD-CLASS-001` | Multi-valued traits | 5 | planned | — | — |
| `PRD-CLASS-002` | Evidence and confidence | 5 | planned | — | — |
| `PRD-CLASS-003` | Evidence hierarchy | 5 | planned | — | — |
| `PRD-POLICY-001` | Dispositions | 5 | planned | — | — |
| `PRD-POLICY-002` | Semantics | 5 | planned | — | — |
| `PRD-POLICY-003` | Greatest Hits requirement | 5 | planned | — | — |
| `PRD-POLICY-004` | Track novelty categories | 5 | planned | — | — |
| `PRD-POLICY-005` | Novelty semantics for redundant collections | 5 | planned | — | — |
| `PRD-POLICY-006` | Example default rules | 5 | planned | — | — |
| `PRD-POLICY-007` | Durable decision record before delete | 5 | planned | — | — |
| `PRD-POLICY-008` | Policy test mode | 5 | planned | — | — |
| `PRD-STORAGE-001` | PRD-STORAGE-001 | 2,3,11 | planned | — | — |
| `PRD-STORAGE-002` | PRD-STORAGE-002 | 2,3,11 | planned | — | — |
| `PRD-STORAGE-003` | PRD-STORAGE-003 | 2,3,11 | planned | — | — |
| `PRD-STORAGE-004` | PRD-STORAGE-004 | 2,3,11 | planned | — | — |
| `PRD-CATALOG-001` | PRD-CATALOG-001 | 4 | planned | — | — |
| `PRD-CATALOG-002` | PRD-CATALOG-002 | 4 | planned | — | — |
| `PRD-CATALOG-003` | PRD-CATALOG-003 | 4 | planned | — | — |
| `PRD-CATALOG-004` | PRD-CATALOG-004 | 4 | planned | — | — |
| `PRD-CATALOG-005` | PRD-CATALOG-005 | 4 | planned | — | — |
| `PRD-SEARCH-001` | Search engine | 6 | planned | — | — |
| `PRD-SEARCH-002` | Searchable entities | 6 | planned | — | — |
| `PRD-SEARCH-003` | Participant IDs in Track index | 6 | planned | — | — |
| `PRD-SEARCH-004` | MQL isolation | 6 | planned | — | — |
| `PRD-SEARCH-005` | MQL parity | 6 | planned | — | — |
| `PRD-SEARCH-006` | Performance target | 6 | planned | — | — |
| `PRD-SEARCH-007` | Rebuild command | 6 | planned | — | — |
| `PRD-API-M1-001` | Compatibility contract | 7 | in-progress | compatibility/melodee-api-v1/contracts/ (OpenAPI snapshot, route manifest, client usage, unified inventory); compatibility/melodee-api-v1/golden/ (native-auth, native-catalog, native-stream); design/evidence/phase-0/native-api-v1-capture.md; design/evidence/phase-0/native-client-route-inventory.md | tests/python/test_native_v1_capture.py |
| `PRD-API-M1-002` | No forced synchronized client release | 7 | planned | — | — |
| `PRD-API-M1-003` | Preserve existing vocabulary | 7 | planned | — | — |
| `PRD-API-M1-004` | Preserve public IDs | 7 | in-progress | compatibility/fixtures/catalog-v1.json (deterministic fixture UUID namespace); compatibility/melodee-api-v1/golden/ (stable public IDs across reseeds) | tests/python/test_native_v1_capture.py (GoldenFixtureTests) |
| `PRD-API-M1-005` | Preserve response details | 7 | in-progress | compatibility/scenarios/native-*.json (method/path, params, casing, null-vs-omitted, status codes, envelopes, pagination, ordering, URLs, headers captured for the critical path) | tests/python/test_native_v1_capture.py; scripts/compare_captures.py |
| `PRD-API-M1-006` | Current native surface | 7 | in-progress | compatibility/melodee-api-v1/contracts/route-inventory.json (all 202 routes classified; 35 excluded-from-build) | tests/python/test_native_v1_capture.py (RouteInventoryTests) |
| `PRD-API-M1-007` | Future API evolution | 7 | planned | — | — |
| `PRD-STREAM-001` | PRD-STREAM-001 | 7,8 | captured | compatibility/melodee-api-v1/golden/native-stream/ (URL shape, HMAC semantics, 200/206/400/401/302 matrix); design/evidence/phase-0/native-api-v1-capture.md section 4 | tests/python/test_native_v1_capture.py (test_stream_range_semantics_evidence) |
| `PRD-STREAM-002` | PRD-STREAM-002 | 7,8 | in-progress | compatibility/melodee-api-v1/golden/native-stream/ (direct play, ranges, content type/length) | tests/python/test_native_v1_capture.py |
| `PRD-STREAM-003` | Android/Android Auto release gate | 7,8 | in-progress | design/evidence/phase-0/native-client-route-inventory.md (Android/Android Auto route inventory and gate prerequisites) | — |
| `PRD-API-OS-001` | PRD-API-OS-001 | 8 | captured | compatibility/opensubsonic/contracts/route-inventory.json (164 routes; plain and .view forms) | tests/python/test_opensubsonic_capture.py |
| `PRD-API-OS-002` | PRD-API-OS-002 | 8 | captured | compatibility/opensubsonic/golden/system/ (JSON/XML/JSONP envelope evidence incl. quirks) | tests/python/test_opensubsonic_capture.py (SystemAndFormatTests) |
| `PRD-API-OS-003` | PRD-API-OS-003 | 8 | captured | compatibility/opensubsonic/golden/auth/ (token/salt, plain+enc password, JWT, apiKey, form POST, bypass); design/evidence/phase-0/opensubsonic-capture.md section 3 | tests/python/test_opensubsonic_capture.py (AuthEvidenceTests) |
| `PRD-API-OS-004` | Current supported groups | 8 | captured | compatibility/opensubsonic/golden/ (browse, lists, media, annotation, playlists, community); compatibility/opensubsonic/contracts/route-inventory.json (81 endpoints classified) | tests/python/test_opensubsonic_capture.py |
| `PRD-API-OS-005` | Preserve intentional unsupported behavior | 8 | captured | compatibility/opensubsonic/golden/negative/ (all 410/501 routes automated) | tests/python/test_opensubsonic_capture.py (NegativeAndConditionalTests) |
| `PRD-API-OS-006` | Advertised extensions | 8 | captured | compatibility/opensubsonic/golden/system/os-getopensubsonicextensions-* (advertised set incl. two non-functional entries) | tests/python/test_opensubsonic_capture.py (test_extensions_advertised) |
| `PRD-API-OS-007` | Client verification | 8 | in-progress | design/evidence/phase-0/opensubsonic-capture.md (server-side evidence for login/browse/search/stream+seek/playlists/download/star) | tests/python/test_opensubsonic_capture.py |
| `PRD-API-JF-001` | PRD-API-JF-001 | 8 | captured | compatibility/jellyfin/contracts/route-inventory.json (60 routes, music-focused subset); compatibility/jellyfin/golden/ | tests/python/test_jellyfin_capture.py |
| `PRD-API-JF-002` | PRD-API-JF-002 | 8 | captured | compatibility/jellyfin/golden/discovery/ (middleware allowlist, marker triggers, root probe, fallthrough); design/evidence/phase-0/jellyfin-capture.md section 1 | tests/python/test_jellyfin_capture.py (RoutingEvidenceTests) |
| `PRD-API-JF-003` | PRD-API-JF-003 | 8 | captured | compatibility/jellyfin/golden/auth/ (issuance, all 5 token transports, failures, logout revocation) | tests/python/test_jellyfin_capture.py (AuthEvidenceTests) |
| `PRD-API-JF-004` | PRD-API-JF-004 | 8 | captured | compatibility/jellyfin/golden/{browse,playback,userstate,playlists,images,extra}/; compatibility/jellyfin/contracts/route-inventory.json (100% scenario coverage) | tests/python/test_jellyfin_capture.py |
| `PRD-API-JF-005` | PRD-API-JF-005 | 8 | captured | compatibility/jellyfin/golden/discovery/ (unsupported video/TV/books scope: plain 404, no simulation) | tests/python/test_jellyfin_capture.py (test_unsupported_scope_is_404) |
| `PRD-API-JF-006` | PRD-API-JF-006 | 8 | in-progress | design/evidence/phase-0/jellyfin-capture.md section 9 (documented music-client smoke path covered server-side) | — |
| `PRD-API-ADMIN-001` | PRD-API-ADMIN-001 | 10 | planned | — | — |
| `PRD-API-ADMIN-002` | PRD-API-ADMIN-002 | 10 | planned | — | — |
| `PRD-API-ADMIN-003` | PRD-API-ADMIN-003 | 10 | planned | — | — |
| `PRD-AUTH-001` | PRD-AUTH-001 | 7,8,9 | planned | — | — |
| `PRD-AUTH-002` | PRD-AUTH-002 | 7,8,9 | planned | — | — |
| `PRD-AUTH-003` | PRD-AUTH-003 | 7,8,9 | planned | — | — |
| `PRD-AUTH-004` | PRD-AUTH-004 | 7,8,9 | planned | — | — |
| `PRD-AUTH-005` | PRD-AUTH-005 | 7,8,9 | planned | — | — |
| `PRD-AUTH-006` | PRD-AUTH-006 | 7,8,9 | planned | — | — |
| `PRD-AUTH-007` | PRD-AUTH-007 | 7,8,9 | planned | — | — |
| `PRD-USER-001` | PRD-USER-001 | 9 | planned | — | — |
| `PRD-USER-002` | PRD-USER-002 | 9 | planned | — | — |
| `PRD-USER-003` | PRD-USER-003 | 9 | planned | — | — |
| `PRD-USER-004` | PRD-USER-004 | 9 | planned | — | — |
| `PRD-USER-005` | PRD-USER-005 | 9 | planned | — | — |
| `PRD-PLAYLIST-001` | PRD-PLAYLIST-001 | 9 | planned | — | — |
| `PRD-PLAYLIST-002` | PRD-PLAYLIST-002 | 9 | planned | — | — |
| `PRD-PLAYLIST-003` | PRD-PLAYLIST-003 | 9 | planned | — | — |
| `PRD-REQUEST-001` | PRD-REQUEST-001 | 9 | planned | — | — |
| `PRD-REQUEST-002` | PRD-REQUEST-002 | 9 | planned | — | — |
| `PRD-SHARE-001` | PRD-SHARE-001 | 9 | planned | — | — |
| `PRD-SHARE-002` | PRD-SHARE-002 | 9 | planned | — | — |
| `PRD-SHARE-003` | PRD-SHARE-003 | 9 | planned | — | — |
| `PRD-POD-001` | PRD-POD-001 | 9 | planned | — | — |
| `PRD-POD-002` | PRD-POD-002 | 9 | planned | — | — |
| `PRD-POD-003` | PRD-POD-003 | 9 | planned | — | — |
| `PRD-DISCOVERY-001` | PRD-DISCOVERY-001 | 9 | planned | — | — |
| `PRD-DISCOVERY-002` | PRD-DISCOVERY-002 | 9 | planned | — | — |
| `PRD-DISCOVERY-003` | PRD-DISCOVERY-003 | 9 | planned | — | — |
| `PRD-PARTY-001` | PRD-PARTY-001 | 9 | planned | — | — |
| `PRD-PARTY-002` | PRD-PARTY-002 | 9 | planned | — | — |
| `PRD-PARTY-003` | PRD-PARTY-003 | 9 | planned | — | — |
| `PRD-PARTY-004` | PRD-PARTY-004 | 9 | planned | — | — |
| `PRD-DEVICE-001` | PRD-DEVICE-001 | 9 | planned | — | — |
| `PRD-DEVICE-002` | PRD-DEVICE-002 | 9 | planned | — | — |
| `PRD-DEVICE-003` | PRD-DEVICE-003 | 9 | planned | — | — |
| `PRD-SCRIPT-001` | PRD-SCRIPT-001 | 9 | planned | — | — |
| `PRD-SCRIPT-002` | PRD-SCRIPT-002 | 9 | planned | — | — |
| `PRD-SCRIPT-003` | PRD-SCRIPT-003 | 9 | planned | — | — |
| `PRD-SCRIPT-004` | PRD-SCRIPT-004 | 9 | planned | — | — |
| `PRD-SCRIPT-005` | PRD-SCRIPT-005 | 9 | planned | — | — |
| `PRD-EXT-001` | PRD-EXT-001 | 4,9 | planned | — | — |
| `PRD-EXT-002` | PRD-EXT-002 | 4,9 | planned | — | — |
| `PRD-EXT-003` | PRD-EXT-003 | 4,9 | planned | — | — |
| `PRD-EXT-004` | PRD-EXT-004 | 4,9 | planned | — | — |
| `PRD-UI-001` | Modern, slick, simple, intuitive | 10 | planned | — | — |
| `PRD-UI-002` | Preferred stack | 10 | planned | — | — |
| `PRD-UI-003` | TanStack Table is optional | 10 | planned | — | — |
| `PRD-UI-004` | Admin navigation | 10 | planned | — | — |
| `PRD-UI-005` | Remove `/Data` vs `/Media` from the user mental model | 10 | planned | — | — |
| `PRD-UI-006` | Primary/detail workflows | 10 | planned | — | — |
| `PRD-UI-007` | Preview playback only where useful | 10 | planned | — | — |
| `PRD-UI-008` | Destructive action UX | 10 | planned | — | — |
| `PRD-UI-009` | Command palette | 10 | planned | — | — |
| `PRD-UI-010` | Server-side data operations | 10 | planned | — | — |
| `PRD-UI-REF-001` | PRD-UI-REF-001 | 10 | planned | — | — |
| `PRD-UI-REF-002` | PRD-UI-REF-002 | 10 | planned | — | — |
| `PRD-UI-REF-003` | PRD-UI-REF-003 | 10 | planned | — | — |
| `PRD-THEME-001` | PRD-THEME-001 | 10 | planned | — | — |
| `PRD-THEME-002` | PRD-THEME-002 | 10 | planned | — | — |
| `PRD-THEME-003` | PRD-THEME-003 | 10 | planned | — | — |
| `PRD-THEME-004` | PRD-THEME-004 | 10 | planned | — | — |
| `PRD-I18N-001` | PRD-I18N-001 | 10 | planned | — | — |
| `PRD-I18N-002` | PRD-I18N-002 | 10 | planned | — | — |
| `PRD-I18N-003` | PRD-I18N-003 | 10 | planned | — | — |
| `PRD-I18N-004` | PRD-I18N-004 | 10 | planned | — | — |
| `PRD-I18N-005` | PRD-I18N-005 | 10 | planned | — | — |
| `PRD-I18N-006` | PRD-I18N-006 | 10 | planned | — | — |
| `PRD-WEB-001` | PRD-WEB-001 | 9,10 | planned | — | — |
| `PRD-WEB-002` | PRD-WEB-002 | 9,10 | planned | — | — |
| `PRD-SETUP-001` | PRD-SETUP-001 | 9 | planned | — | — |
| `PRD-SETUP-002` | PRD-SETUP-002 | 9 | planned | — | — |
| `PRD-CONFIG-001` | PRD-CONFIG-001 | 1,9 | planned | — | — |
| `PRD-CONFIG-002` | PRD-CONFIG-002 | 1,9 | planned | — | — |
| `PRD-CONFIG-003` | PRD-CONFIG-003 | 1,9 | planned | — | — |
| `PRD-WORK-001` | PRD-WORK-001 | 1,2,9 | in-progress | crates/melodee-persistence/ (durable work queue foundation: items, leases, retries, history) | crates/melodee-persistence/tests/ |
| `PRD-WORK-002` | PRD-WORK-002 | 1,2,9 | planned | — | — |
| `PRD-WORK-003` | PRD-WORK-003 | 1,2,9 | tested | apps/melodee-server/src/main.rs (ingest scheduler startup); crates/melodee-ingestion/src/scheduler.rs (graceful periodic reconcile task); crates/melodee-ingestion/src/reconcile.rs (reconcile_all_sources and per-source loop) | crates/melodee-ingestion/tests/reconciliation.rs (scheduler_runs_periodic_reconciliation_with_restart_signal) |
| `PRD-WORK-004` | PRD-WORK-004 | 1,2,9 | tested | apps/melodee-cli/src/main.rs (work list/show/seed-test/claim-test sharing WorkItemStore); apps/melodee-cli/src/main.rs (ingest reconcile/status command paths); crates/melodee-ingestion/src/reconcile.rs (one-shot reconciliation against durable state) | apps/melodee-cli (manual command verification in docs and local ops); apps/melodee-ingestion/tests/reconciliation.rs (lost_watcher_notification_is_recovered_by_scan) |
| `PRD-WORK-005` | PRD-WORK-005 | 1,2,9 | in-progress | crates/melodee-persistence/src/work_items.rs (operator transitions: quarantine/reprocess/retry; release_lease; append-only events) | crates/melodee-persistence/tests/transitions.rs (operator_review_decisions_and_reprocess) |
| `PRD-OBS-001` | PRD-OBS-001 | 1,2 | in-progress | crates/melodee-persistence/src/work_items.rs (tracing events with work_item_id/worker_id/correlation_id/lease fields) | — |
| `PRD-OBS-002` | PRD-OBS-002 | 1,2 | planned | — | — |
| `PRD-OBS-003` | PRD-OBS-003 | 1,2 | planned | — | — |
| `PRD-OBS-004` | PRD-OBS-004 | 1,2 | planned | — | — |
| `PRD-OBS-005` | PRD-OBS-005 | 1,2 | planned | — | — |
| `PRD-PERF-001` | PRD-PERF-001 | 0,all | in-progress | compatibility/benchmarks/ (datasets, results with environment metadata, SUMMARY.md); scripts/generate_benchmark_dataset.py; scripts/run_benchmarks.py; design/evidence/phase-0/scale-baseline.md | tests/python/test_benchmark_baseline.py |
| `PRD-PERF-002` | PRD-PERF-002 | 0,all | captured | scripts/generate_benchmark_dataset.py (three deterministic SQL tiers); scripts/generate_media_corpus.py (deterministic ffmpeg media corpus) | tests/python/test_benchmark_baseline.py; tests/python/test_media_baseline.py |
| `PRD-PERF-003` | PRD-PERF-003 | 0,all | in-progress | compatibility/benchmarks/results/ (wall-clock cold/warm, response sizes, server memory at Tier C) | tests/python/test_benchmark_baseline.py |
| `PRD-PERF-004` | PRD-PERF-004 | 0,all | in-progress | design/evidence/phase-0/scale-baseline.md section 4 (per-Release pipeline cost shape ~3s; non-transactional restart behavior evidence) | — |
| `PRD-PERF-005` | PRD-PERF-005 | 0,all | in-progress | design/evidence/phase-0/scale-baseline.md sections 3, 5 (bottlenecks + proposed budgets labeled requirement-vs-proposal) | tests/python/test_benchmark_baseline.py |
| `PRD-PERF-006` | PRD-PERF-006 | 0,all | planned | — | — |
| `PRD-MIG-001` | Preserve media | 11 | planned | — | — |
| `PRD-MIG-002` | Preserve public identity | 11 | planned | — | — |
| `PRD-MIG-003` | Preserve user state | 11 | planned | — | — |
| `PRD-MIG-004` | Preserve catalog relationships | 11 | planned | — | — |
| `PRD-MIG-005` | No mandatory upfront fingerprint of every byte | 11 | planned | — | — |
| `PRD-MIG-006` | Search is rebuildable | 11 | planned | — | — |
| `PRD-MIG-007` | Rollback | 11 | planned | — | — |
| `PRD-TEST-001` | C# server as oracle | 0,all | in-progress | compatibility/scenarios/*.json (22 scenario files, declarative flows); scripts/capture_baseline.py + scripts/compare_captures.py (deterministic A/B comparison); compatibility/*/golden/ (424 promoted captures with provenance) | tests/python/test_native_v1_capture.py; tests/python/test_opensubsonic_capture.py; tests/python/test_jellyfin_capture.py; tests/python/test_media_baseline.py; tests/python/test_benchmark_baseline.py |
| `PRD-TEST-002` | Differential API harness | 0,all | in-progress | compatibility/melodee-api-v1/golden/ + opensubsonic/golden/ + jellyfin/golden/ (normalized golden request/response fixtures incl. negative paths) | tests/python/test_native_v1_capture.py (GoldenFixtureTests); tests/python/test_opensubsonic_capture.py; tests/python/test_jellyfin_capture.py |
| `PRD-TEST-003` | Golden contracts | 0,all | planned | — | — |
| `PRD-TEST-004` | Real-client compatibility | 0,all | planned | — | — |
| `PRD-TEST-005` | Media corpus | 0,all | in-progress | compatibility/media/ (31-case synthetic corpus incl. malformed tags, corrupt/truncated audio, invalid artwork, duplicate track numbers) | tests/python/test_media_baseline.py |
| `PRD-TEST-006` | Fault injection | 0,all | in-progress | design/evidence/phase-0/media-baseline.md section 11 (interruption/non-atomicity evidence); compatibility/melodee-api-v1/golden/native-auth/ (refresh replay/rotation fault evidence) | — |
| `PRD-TEST-007` | Invariant | 0,all | in-progress | compatibility/benchmarks/ (3 tiers, environment metadata, machine-readable results); design/evidence/phase-0/scale-baseline.md (budgets labeled requirement-vs-proposal) | tests/python/test_benchmark_baseline.py |
| `PRD-TEST-008` | UI testing | 0,all | planned | — | — |
| `PRD-SEC-001` | PRD-SEC-001 | all | planned | — | — |
| `PRD-SEC-002` | PRD-SEC-002 | all | planned | — | — |
| `PRD-SEC-003` | PRD-SEC-003 | all | planned | — | — |
| `PRD-SEC-004` | PRD-SEC-004 | all | planned | — | — |
| `PRD-SEC-005` | PRD-SEC-005 | all | planned | — | — |
| `PRD-SEC-006` | PRD-SEC-006 | all | planned | — | — |
| `PRD-SEC-007` | PRD-SEC-007 | all | planned | — | — |
| `PRD-DEPLOY-001` | PRD-DEPLOY-001 | 1,11 | planned | — | — |
| `PRD-DEPLOY-002` | PRD-DEPLOY-002 | 1,11 | planned | — | — |
| `PRD-DEPLOY-003` | PRD-DEPLOY-003 | 1,11 | planned | — | — |
| `PRD-DEPLOY-004` | PRD-DEPLOY-004 | 1,11 | planned | — | — |
| `PRD-DEPLOY-005` | PRD-DEPLOY-005 | 1,11 | planned | — | — |
| `PRD-DEPLOY-006` | PRD-DEPLOY-006 | 1,11 | planned | — | — |
