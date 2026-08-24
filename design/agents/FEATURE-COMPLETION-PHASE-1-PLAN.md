# Feature-Completion Phase 1 Plan — MusicBrainz ID Retention & AudioIdentity Proposal Wiring

Status: Planned (Phase 1 of `design/ROAD-TO-FEATURE-COMPLETION.md`, W0/W1 scope)
Authority: ADR-0117 (external identifier retention), ADR-0118 (audio identity evidence model),
ADR-0160 (resolver proposal contract), `SPEC.md` manifest/identity invariants.

## Objective

1. **TRACK-001** — `melodee-media` retains MusicBrainz IDs (track/recording, release/album,
   release group, artist, release artist, work) as raw strings through `ObservedMetadata`
   into the durable manifest, without altering any semantic-manifest identity for existing
   fixtures.
2. **TRACK-002** — The catalog import path proposes durable `AudioIdentity` evidence for
   each ingested Track (byte hash, fingerprint, duration, external identifier, catalog
   context) and creates idempotent `AudioIdentityMatchDecision` proposals where two or more
   provisional identities carry overlapping supporting evidence.
3. Close the local parity gap: C# `LibraryInsertJob`/`MetaTagIdentifier` stored MusicBrainz
   IDs as raw strings on `Song`/`Album`; the Rust pipeline currently discards them.

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


