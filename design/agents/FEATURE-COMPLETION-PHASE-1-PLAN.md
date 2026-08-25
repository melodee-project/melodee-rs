# Feature-Completion Phase 1 Plan — Quick Wins + the P0 Bug (complete phase)

Status: In execution (the "Phase 1 — Quick wins + the P0 bug" milestone of
`design/ROAD-TO-FEATURE-COMPLETION.md` §5). User-assigned 2026-08-23; execution continues
through implementation, tests, compatibility evidence, documentation, traceability, full
validation, and the phase report at
`design/evidence/feature-completion/phase-1/PHASE-1-REPORT.md`.

> **Authority note.** D1–D5 in the Track workstream section below are *implementation
> hypotheses*, not accepted decisions. They must remain consistent with `PRD.md`
> (PRD-FP-001..005), `SPEC.md`, and the accepted ADRs listed below. They are not a source of
> authority.
>
> **ADR scope correction (vs. the earlier draft of this plan):** ADR-0160 governs
> **cross-Release Artist/Release identity promotion** (Feature-Completion Phase 2); it does
> **not** govern AudioIdentity match proposals. TRACK-001/TRACK-002 are primarily governed by
> **PRD-FP-001..005 and ADR-0117/ADR-0118** (AudioIdentity and the catalog identity schema).

## Scope (complete Phase 1 — all workstreams)

| Workstream | Roadmap row | Contents |
|---|---|---|
| **A — RELEASE-002 (P0) metadata editing proof** | `RELEASE-002` | Inspect the work already landed at `cd0256a`/`a937b81`; verify Release/Artist/Track UI field names vs. server patch DTOs; Playwright coverage (edit → save → reload → persisted values); validation failures visible and non-mutating; revision/search-refresh + audit proof; retain last-write-wins; parity/roadmap wording distinguishes catalog metadata editing from the Phase 4 ADR-0157 tag-transformation/rewrite capability (which is **not** implemented here) |
| **B — TRACK-001/002 external-ID retention + AudioIdentity wiring** | `TRACK-001`, `TRACK-002` | Retain embedded MusicBrainz recording/release/release-group/Artist/release-Artist/work IDs through media inspection into the durable manifest; malformed IDs preserved as observations; format-appropriate unit/media fixtures + end-to-end manifest proof; durable AudioIdentity evidence from import-time facts; bounded, conflict-safe, idempotent **Proposed** match decisions (never automatic `Confirmed`); no fabricated no-match assertions; atomicity/retry/pair-dedup/cross-Release proof; existing ID-free fixtures and semantic hashes unchanged; document why no migration is required (or stop on a schema gap) |
| **C1 — Nine UNPROVEN rows, service behaviors** | `PLAYBACK-001`, `TRANSCODE-001`, `PODCAST-001`, `DISCOVERY-001`, `DISCOVERY-002`, `SOCIAL-001` | Differential captures vs. the frozen C# oracle, golden tests, integration tests, and end-to-end lifecycle journeys covering success, authorization, validation, ordering, failure, cancellation/retry, stale-data, cleanup, and recovery where applicable; fix in-scope defects found; preserve separate native-v1 / OpenSubsonic / Jellyfin wire contracts |
| **C2 — OPS-001 + MIGRATE-001/002 evidence** | `OPS-001`, `MIGRATE-001`, `MIGRATE-002` | OPS-001: locally reproducible security headers, correlation, and native/OpenSubsonic/Jellyfin rate-limit behavior; distinguish accepted party/jukebox exclusions (ADR-0133) from unproven middleware; no external/production-scale rate-limit claims. MIGRATE-001/002: consolidate + validate existing synthetic/rehearsal evidence (idempotent resume, public-ID preservation, backup/restore validation, verification, search rebuild, rollback tooling); add missing local regression evidence; **do not** touch the real library or run rehearsal/cutover/rollback — record those operator-authorized legs precisely as external residuals |
| **D — Evidence, documentation, closeout** | — | Update `design/ROAD-TO-FEATURE-COMPLETION.md`, `design/ROAD-TO-FEATURE-PARITY.md`, `design/requirements/FEATURE-PARITY-MATRIX.md`, requirement status/evidence files, `/docs` for changed user-visible behavior, and release notes where applicable; create `design/evidence/feature-completion/phase-1/PHASE-1-REPORT.md` with a truthful terminal disposition for every workstream and all nine unproven rows |

**Reconciled roadmap language (entry corrections):**

- ADR-0157 (metadata transformation & safe rewrite) is an accepted **Phase 4** design
  (ADR-0157 §scope, Phase 10 deferral of admin metadata-edit forms). Phase 1 must prove the
  *existing* catalog metadata save path only, and must not claim the complete legacy
  `MediaEditService` tag-rewriting capability is proven.
- External / operator-authorized R4 legs (real Google OIDC, per-surface production
  rate-limit, 5 TB-library rehearsal/cutover/rollback) are **not** ordinary Phase 1 local
  work; they are recorded as external residuals.
- Real Google OIDC, physical-client acceptance, production integrations, the real
  ~5 TB library, production cutover, and destructive rollback remain outside this phase.

## Design decisions

### D1 — Carrier for MusicBrainz IDs: `ObservedMetadata.raw` (no domain struct change)

`ObservedMetadata` in `crates/melodee-domain/src/manifest.rs` (lines 86–99) is the semantic
manifest payload; adding six `Option<String>` fields would change `semantic_value()` output
for every manifest (the semantic hash is `serde_json::to_value(self)` with provenance
strips only — see `ReleaseManifest::semantic_value`, manifest.rs:751). Existing fixtures,
replay baselines, and `manifest_versions` tests depend on the current JSON shape.

Instead, retain IDs in the already-present `raw: serde_json::Value` field with a fixed key
contract, mirroring the existing `musicbrainz_release_type` / `flag_compilation` precedent
(`crates/melodee-media/src/lib.rs:815–816`). `raw` is an observed fact about the file, so it
belongs in the semantic payload; new manifests will hash differently than old ones, which is
correct (different observed facts), and existing fixtures keep their hashes because they
carry no ID keys.

Key contract (fixed, documented on the extraction site):

```json
{
  "musicbrainz_recording_id": "…",   // TXXX MUSICBRAINZ_TRACKID (preferred) / MUSICBRAINZ_RELEASETRACKID
  "musicbrainz_release_id": "…",     // TXXX MUSICBRAINZ_ALBUMID
  "musicbrainz_release_group_id": "…",
  "musicbrainz_artist_id": "…",      // TXXX MUSICBRAINZ_ARTISTID
  "musicbrainz_release_artist_id": "…", // TXXX MUSICBRAINZ_ALBUMARTISTID
  "musicbrainz_work_id": "…"
}
```

Values are raw trimmed strings, exactly as C# stored them (no GUID validation — malformed
IDs are still observations; a resolver may weigh them, the pipeline does not reject).

### D2 — Extraction in `melodee-media` via canonical `ItemKey`s

Lofty 0.25 already canonicalizes every C# `MetaTagIdentifier` MusicBrainz key into typed
`ItemKey` variants (`lofty/src/tag/item.rs:153–159`, 251–254: `MUSICBRAINZ_TRACKID` →
`ItemKey::MusicBrainzRecordingId`, `MUSICBRAINZ_ALBUMID` → `ItemKey::MusicBrainzReleaseId`,
`MUSICBRAINZ_ARTISTID` → `ItemKey::MusicBrainzArtistId`, `MUSICBRAINZ_ALBUMARTISTID` →
`ItemKey::MusicBrainzReleaseArtistId`, plus `MusicBrainzReleaseGroupId`,
`MusicBrainzTrackId`, `MusicBrainzWorkId`). In `audio_tag_details` (lib.rs:788–826):

- `get_string` for each key (lofty resolves ID3 TXXX / Vorbis comment / MP4 / APE uniformly).
- `musicbrainz_recording_id`: prefer `MusicBrainzRecordingId` (`MUSICBRAINZ_TRACKID`), fall
  back to `MusicBrainzTrackId` (`MUSICBRAINZ_RELEASETRACKID`) — exactly the legacy dual-key
  behavior.

### D3 — Proposal pipeline lives in the catalog importer, not a new worker

`PreparedImport` already persists `audio_identities` with the normalized byte hash and
duration (`crates/melodee-persistence/src/catalog.rs`, `PreparedImport::build`). The importer
already writes identity evidence today, so `audio_identity_evidence` rows for the Track's
own provisional identity are a natural extension of the same transaction:

For each ingested Track, insert `AudioIdentityEvidence` rows (kinds per ADR-0118):

| kind                  | source                          | polarity | confidence |
|-----------------------|---------------------------------|----------|------------|
| `ByteHash`            | normalized `byte_hash`          | Supports | 0.95       |
| `AcousticFingerprint` | `FingerprintObservation`        | Supports | 0.90       |
| `Duration`            | normalized duration_ms          | Supports | 0.70       |
| `ExternalIdentifier`  | `raw.musicbrainz_*` IDs         | Supports | 0.85 each  |
| `CatalogContext`      | disc / track ordinal            | Supports | 0.50       |

Confidences are constants defined in the importer and cited by resolver version
`audio-resolver-import-v1` — proposals are *evidence records plus Proposed decisions*,
never `Confirmed` (the admin/automated-resolution flow owns confirmation per ADR-0160).

### D4 — Match proposals: pairwise support within one import and against the catalog

Within the importing transaction, after all Track identities and evidence are persisted:

1. Group candidate identity pairs by shared supporting evidence value (same MB ID, same
   fingerprint, same normalized byte hash, same duration within the resolver tolerance).
2. A pair with ≥2 independent supporting dimensions creates an
   `AudioIdentityMatchDecision { outcome: Proposed, authority: Automated {
   resolver_version: "audio-resolver-import-v1" }, locked: false, reason }` through the
   existing `AudioIdentityMatchDecision::new` domain constructor (so domain invariant
   checks apply) and is persisted via
   `store.record_audio_identity_match_decision`.
3. Idempotency: re-imports of the same work item short-circuit at
   `state == Cataloged` (catalog.rs:614–618) — proposals are created at most once per
   durable manifest. For cross-item pairs, compare the pair key
   `(min(source,candidate), max(source,candidate))` against existing decisions in the same
   transaction before insert; a matching existing decision skips the insert.
4. **No `no_catalog_match` assertions.** `audio_identity_catalog_assertions` is a negative
   fact requiring a deliberate resolution; the import path must not fabricate them.

Cross-library matching (a new item's MB ID equaling an already-cataloged Track's MB ID) is
supported because step 1 also scans `audio_identity_evidence` rows of existing identities —
bounded by `WHERE kind = 'external_identifier' AND value = ANY($ids)` (the
`evidence_kind_kind_value_idx` index from migration 0010 covers this).

### D5 — Explicit deferrals (recorded in the decision log)

- **No external MusicBrainz lookup** (that is W3 `EXTERNAL-001`, requires an ADR for the
  HTTP client). Retained IDs are *input* to future matching, not fetched.
- **Acoustic cross-identity scoring** beyond exact fingerprint equality is deferred to the
  resolver service (W4); import creates evidence, not fuzzy decisions.

## Implementation waves

### Wave 1 — TRACK-001: ID retention (domain-free media change)

1. `crates/melodee-media/src/lib.rs` — `audio_tag_details`: extract the six IDs per D2 and
   insert into `raw`.
2. Media unit tests (lib.rs test module): ID3 TXXX fixture, Vorbis comment fixture, MP4
   fixture — each asserting raw-string retention, dual-key fallback for the recording ID,
   and that existing `ObservedMetadata` fields are untouched.
3. No schema migration (IDs live in manifest JSONB), no domain-struct change.

### Wave 2 — TRACK-002: import-time evidence + proposals

1. `crates/melodee-persistence/src/catalog.rs` — in `PreparedImport::build` /
   `persist_prepared_import`, emit the D3 evidence rows per Track and the D4 proposals.
   Reuse the evidence/decision persistence SQL shape from `catalog_identity.rs` inside the
   import transaction to keep atomicity.
2. Integration test `crates/melodee-persistence/tests/catalog_audio_identity.rs` — add:
   - an import whose manifest `raw` carries a MusicBrainz ID: assert an
     `ExternalIdentifier` evidence row with the raw value, correct polarity/confidence.
   - two fixtures sharing an MB ID + fingerprint: assert one `Proposed`
     `AudioIdentityMatchDecision` with both evidence IDs, `locked = false`, automated
     authority; re-import of the same item is a no-op (idempotency).
   - assert zero `audio_identity_catalog_assertions` rows after import.
3. Extend `catalog_common` fixture helpers with an optional MusicBrainz-ID overlay on
   `ObservedMetadata.raw` (fixture only — no domain change).

### Wave 3 — parity evidence & docs

1. Update `design/evidence/` phase notes: C# `LibraryInsertJob` + `MetaTagIdentifier`
   mapping table (TXXX/Vorbis/MP4 keys → retained raw values) as behavioral evidence.
2. Update requirement traceability for TRACK-001/TRACK-002 → tested; re-run
   `extract_requirements.py --check`.
3. `/docs` impact: search `docs/src/content` for claims that IDs are not retained; update
   only if found.
4. No new ADR required (ADR-0117/0118 already cover the mechanism); record D1–D5 in the
   phase decision log. If the `raw`-key contract in D1 later becomes a public wire
   contract, supersede with an ADR.

## Workstream A — RELEASE-002 metadata editing proof

- Inspect (do not recreate) `cd0256a` (server routes + patch DTOs + validation) and `a937b81`
  (domain structs + UI forms): `apps/melodee-server/src/admin_catalog.rs`
  (`PUT /api/admin/v1/catalog/{releases|artists|tracks}/{id}/metadata`),
  `crates/melodee-persistence/src/catalog_mutations.rs`,
  `admin-ui/src/screens/catalog/EditPanel.tsx`, `admin-ui/src/api/admin.ts`.
- Verify every Release/Artist/Track UI field name matches the server patch DTOs exactly.
- Playwright: edit metadata → save → reload the entity → persisted values are observed
  (Release, Artist, Track). Validation failure: visible in UI, request rejected with 400,
  catalog state unchanged (read back before/after). Revision bump + search-index refresh and
  audit event (`release_metadata_updated` / `artist_metadata_updated` / `track_metadata_updated`)
  proven where applicable.
- Retain the current **last-write-wins** contract (no optimistic concurrency) unless higher
  authority says otherwise.
- Parity/roadmap wording: distinguish "catalog metadata field editing (proven in W0 + this
  phase)" from "legacy C# MediaEditService tag-rewriting transforms (ADR-0157, Phase 4 — not
  implemented here)".

## Workstream C — close the nine IMPLEMENTED — UNPROVEN rows (local evidence)

Per the 2026-08-23 audit (`design/ROAD-TO-FEATURE-PARITY.md` §6 rows), the nine rows and
their exact remaining gaps:

- **PLAYBACK-001** (`native_stream.rs`, `native_stream_acl.rs`, OpenSubsonic `download`/`stream`,
  Jellyfin stream routes): missing proof = current-C# differential/e2e journey for download,
  seek/range, transcode failure/cancellation, client-profile selection across native /
  OpenSubsonic / Jellyfin. ADR-0147 (streaming architecture) is the accepted design.
- **TRANSCODE-001** (`apps/melodee-server/src/transcode.rs`, `transcode_service.rs`): missing
  proof = request mapping, template failure, cancellation, seek behavior (ADR-0147).
- **PODCAST-001** (refresh/download/cleanup/recovery legs in `podcast_worker.rs`,
  `native_podcasts_api.rs`, `native_podcasts_real_feed.rs`): missing proof = end-to-end
  lifecycle (refresh → download → cleanup → recovery; OPML import; progress) vs. C# behavior.
- **DISCOVERY-001 / DISCOVERY-002** (`native_discovery_api.rs`, `discovery_worker.rs`):
  missing proof = discovery surfaces (charts/recommendations as audited) e2e journey +
  worker refresh cycles vs. C# `ChartUpdate` behavior.
- **SOCIAL-001** (`native/community.rs`, `native_community_api.rs`,
  `notification_wiring.rs`): missing proof = request auto-completion, email delivery
  degradation, public-share authorization journey vs. C#.
- **OPS-001** (`security_headers.rs`, `correlation.rs`, ADR-0126 rate limiting): prove locally
  reproducible security headers, `x-request-id` correlation, and native / OpenSubsonic /
  Jellyfin rate-limit behavior. The three party-specific C# policies are moot (party/jukebox
  controllers excluded by accepted ADR-0133) — record that as an accepted exclusion, not as
  unproven middleware. Do **not** claim external/production-scale rate-limit proof.
- **MIGRATE-001 / MIGRATE-002** (R3/R4 evidence in `design/evidence/r3/`, `r4/`): consolidate
  and re-validate the existing synthetic/rehearsal evidence — idempotent resume, public-ID
  preservation, backup/restore validation (pg dump/restore + verify + rollback drill),
  search rebuild, `melodee legacy verify`. Add missing local regression evidence where
  practical. **External residuals (explicit, not completed):** production-equivalent
  backup/restore, traffic switch, rehearsal, cutover, and rollback on the real ~5 TB library
  (R4.7/R4.8) — operator-authorized, Phase 11 domain.

Method: differential captures against the frozen C# oracle where the wire/behavior contract
is externally facing (OpenSubsonic/Jellyfin/native), plus integration + e2e lifecycle tests
for the Rust surfaces. Fix in-scope defects discovered; preserve separate wire contracts
(native-v1 / OpenSubsonic / Jellyfin are separate products — no shared DTOs).

## Test / validation plan

- `cargo fmt --all` before every gate.
- `make check-fast` after each wave (format, clippy, workspace unit tests, doc validation).
- `make test-integration` with dev Postgres on :5433 — new `catalog_audio_identity` cases
  plus full regression of catalog/identity tests.
- Media corpus: use the media corpus generator with MB-tagged files where supported;
  otherwise rely on the lofty-level unit fixtures.
- `make check` full gate before closing the phase (admin-ui gate included).

## Risks & mitigations

- **Semantic-hash drift**: ID-bearing manifests hash differently than pre-change fixtures.
  Mitigation: fixtures without IDs are byte-identical in `raw`, so existing golden hashes
  hold; add a test asserting ID-bearing manifests are stable across v2→v3 upgrades.
- **Fingerprint evidence availability**: `FingerprintObservation` may be absent.
  Mitigation: evidence rows are conditional per available fact; proposals require ≥2
  dimensions, so a fingerprint-less pair can still propose via MB ID + duration.
- **Pair-scan cost**: the MB ID scan is index-backed and bounded to external-identifier
  evidence; the fingerprint equality scan uses the existing value index. Both are
  O(matches), not cross-product, at catalog scale.

## Execution log (Phase 1)

### Wave 0 — plan amendment (done)
- `design/agents/FEATURE-COMPLETION-PHASE-1-PLAN.md` amended to full Phase 1 scope
  (workstreams A–D), D1–D5 demoted to hypotheses, ADR-0160 scope corrected
  (cross-Release promotion = Phase 2; TRACK-001/002 governed by PRD-FP-001..005 and
  ADR-0117/0118), ADR-0157 Phase-4 boundary and external R4 residuals reconciled.
  Committed.

### Wave 1 — TRACK-001 ID retention (implementation done; tests pending verification)
- `crates/melodee-media/src/lib.rs`:
  - `MUSICBRAINZ_ID_FAMILIES` (recording/release/release_group/artist/release_artist/work)
    mapped to lofty canonical `ItemKey`s.
  - `musicbrainz_id_observations()` inserts present families (raw strings, malformed
    values verbatim) into `ObservedMetadata.raw["musicbrainz_ids"]`.
  - `read_native_metadata` now round-trips ID3v2 primary tags through
    `Id3v2Tag` so TXXX-description and MusicBrainz-owned-UFID frames normalize to
    canonical keys before extraction (lofty quirk; no effect on other formats).
  - Unit tests added: per-format round-trip (mp3 TXXX, flac Vorbis, m4a MP4),
    empty-object-when-untagged, malformed-value-verbatim retention.
- Oracle alignment: C# stores a single opaque `MusicBrainzId` raw string
  (`MetaTagIdentifier.MusicBrainzId` `[TXX, TXXX]`, `SafeParser.ToGuid`); the six
  family keys are the Picard/lofty superset of that mapping.
- **Blocked on shell**: `cargo test -p melodee-media --lib musicbrainz` could not be
  run to completion — the command-execution session began failing every invocation
  (even `pwd`) with `[<literal command string>]: command not found`, which corrupts
  commands by treating them as a single quoted token. Earlier in the session the
  per-format test compiled and ran; the final verification run (with the ID3v2
  round-trip in place) has NOT been executed. Do not report Wave 1 as green until
  it is.

## Out of scope (explicit)

Resolver service execution (W4), MusicBrainz client (W3), admin review UI changes,
alternate-recording promotion, `no_catalog_match` assertion authoring.

## Definition of done

- TRACK-001: unit tests green; IDs verifiable end-to-end in a published manifest
  (integration fixture).
- TRACK-002: integration tests green for evidence rows, idempotent proposals, and no
  fabricated assertions.
- `make check` passes; `design/evidence` note committed; decision log updated;
  documentation impact stated.


