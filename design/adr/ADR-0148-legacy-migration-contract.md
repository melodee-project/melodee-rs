# ADR-0148: Legacy migration contract (Phase 11 tooling)

- **Date:** 2026-08-17
- **Status:** Accepted
- **Acceptance:** 2026-08-17, R3.1; implemented literally by the R3.2–R3.8 work packages
  (evidence: withdrawn `design/evidence/r3/README.md` report)
- **Decision owners:** Release owner
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MIG-001, PRD-MIG-002, PRD-MIG-003, PRD-MIG-004, PRD-MIG-005,
  PRD-MIG-006, PRD-MIG-007, PRD-STORAGE-004

## Context

Melodee 1.0 must replace the frozen C# server (baseline commit `4028b58d`, schema fingerprint
`__EFMigrationsHistory` ending at `20260714155936_AddDeleteSourceResidueAfterIngestSetting`)
without losing public identity, catalog relationships, user state, or the existing canonical
media library (approximately 5 TB). `IMPLMENTATION_OUTSTANDING.md` work package R3.1 requires
this ADR to fix — not delegate — every migration contract decision before any migration schema
or importer code begins.

Key facts established by the Phase 0 baseline and the frozen EF Core model snapshot:

- Legacy public identity is the per-entity `ApiKey` GUID (plus `ShareUniqueId` on shares);
  database primary keys are sequential integers and are never externally visible.
- `Albums` carries both `OriginalReleaseDate` and `ReleaseDate`; the Rust schema has one release
  date plus edition handling, so an explicit mapping is required.
- Canonical media lives under the legacy library root as `Directory` (Album) +
  `FileName`/`FileHash`/`FileSize` (Song). The migration must adopt these files in place.
- Legacy `Party*`/`PlayQue*` tables belong to retired Party/Jukebox behavior (ADR-0133,
  ADR-0140, ADR-0141) and are excluded from import as enabled features.
- The legacy `ArtistSearchEngine` context is a search projection, not authority; the Rust
  Tantivy index is rebuilt from migrated PostgreSQL state and never copied.

## Decision

### D.1 Source and target roles

1. The legacy source database is opened **read-only** from an isolated snapshot or replica.
   The tooling connects with `default_transaction_read_only = on` and fails closed if any
   statement attempts a write. The source database and source media are never modified.
2. The Rust target database becomes authoritative only at cutover. Before cutover the target
   may be rebuilt from scratch; the source is never treated as stale authority after freeze.

### D.2 Identity preservation

3. Every externally visible public ID is preserved through an explicit durable mapping:
   legacy `ApiKey`/`ShareUniqueId` GUID → Rust `public_api_id`. Mappings are stored in the
   `legacy_entity_map` table (below), separate from semantic catalog identity.
4. Normalized names, display names, and filesystem paths are never used as mapping or
   uniqueness keys. Legacy integer primary keys are stored as source-row evidence only.
5. Resume is idempotent, keyed by `(source_database_identity, source_table, source_pk)` plus a
   source row-version marker where the source has one (`LastUpdatedAt`); a repeated import of an
   unchanged row is a no-op that re-asserts the same target row.

### D.3 Media adoption

6. Canonical media is adopted in place relative to the operator-configured canonical root.
   The migration validates containment, symlink policy, existence, and expected size; it does
   not move, re-encode, re-hash, fingerprint, or delete media. A full-library hash/inspection
   backfill may be scheduled only as durable low-priority work and is never authority for
   deletion.
7. Legacy `Libraries.Path` values are host-specific configuration; they map to configured Rust
   roots by an operator-supplied root mapping (`--library-path-map <legacyPath>=<relativePrefix>`)
   and are never persisted as semantic identity.
8. `media_assets.byte_hash` is a NOT NULL 64-hex column whose check constraint the migration
   does not weaken. Legacy `Songs.FileHash` is adopted directly when it is already lowercase
   64-hex. Otherwise the migration records the **hash-pending sentinel** — exactly 64 ASCII
   zero characters — meaning "hash not yet computed; never authoritative". Sentinel rows are
   excluded from every dedup/identity decision (a sentinel never equals a real BLAKE3 digest),
   are never evidence for deletion or replacement, and are replaced in place by the optional
   bounded hash-backfill work queue. Computing hashes for the full library is therefore never
   mandatory for cutover.

### D.4 Search

9. Tantivy data is rebuilt from migrated PostgreSQL state after import (`search rebuild`);
   no search index bytes are copied.

### D.5 Durable run model (schema)

All migration run records live in the target database in a dedicated `legacy_migration` schema
created by one forward-only SQL migration (`0032_legacy_migration.sql`). Catalog public-ID
mappings for artists/releases/tracks reuse the existing Phase-4
`legacy_catalog_id_mappings` table, whose entity check already covers the full catalog
vocabulary (`artist`, `release`, `track`); all other domains map through
`legacy_migration.entity_map`. Four new tables:

- `legacy_migration.runs` — `id uuidv7 PK`, `state text` (see D.6), `source_identity text`
  (host fingerprint + schema fingerprint hash, no credentials), `source_snapshot_marker text`,
  `owner text`, `lease_expires_at timestamptz`, `created_at`, `updated_at`,
  `completed_at timestamptz NULL`, `notes jsonb NULL` (amended to `jsonb` by ADR-0149 so
  baseline/freeze evidence merges atomically).
- `legacy_migration.checkpoints` — `run_id FK`, `domain text` (e.g. `artists`, `releases`,
  `tracks`, `users`, `playlists`, …), `last_source_pk bigint`, `processed bigint`,
  `inserted bigint`, `updated bigint`, `issues bigint`, `elapsed_ms bigint`,
  `updated_at timestamptz`, PK `(run_id, domain)`.
- `legacy_migration.entity_map` — `run_id FK`, `domain text`, `source_pk bigint`,
  `source_public_id uuid NULL`, `target_id uuid`, `source_updated_at timestamptz NULL`,
  `row_hash text NULL`, `created_at`, PK `(run_id, domain, source_pk)`, UNIQUE
  `(domain, target_id)`. Non-catalog domains only; catalog artist/release/track identity
  mappings live in `legacy_catalog_id_mappings` with the same uniqueness guarantees.
- `legacy_migration.issues` — `id uuidv7 PK`, `run_id FK`, `domain text`,
  `source_pk bigint NULL`, `severity text`, `code text`, `message text`,
  `detail jsonb NULL`, `created_at`. Issues are the audit trail for every unrepresentable or
  anomalous source row; nothing is silently dropped.

### D.6 Run state machine

Monotonic, auditable transitions, fenced by `owner` + `lease_expires_at` (a stale owner cannot
advance or mutate a run):

```text
planned -> importing -> verifying -> delta-ready -> cutover-ready -> complete
    |          |            |             |              |
    +----------+------------+-------------+--------------+--> failed
    +----------+------------+-------------+--------------+--> cancelled
```

- `planned`: inventory + dry-run plan recorded; no target domain rows written.
- `importing`: catalog/identity/user import in progress; checkpoints advance per batch.
- `verifying`: verification totals and samples computed.
- `delta-ready`: initial verification clean; deltas may be applied.
- `cutover-ready`: operator acknowledgement recorded after final delta + freeze evidence.
- `complete`: cutover executed (R4 concern); recorded for audit.
- `failed`/`cancelled`: terminal; a new run must be created. Failed targets are preserved for
  forensics; rollback never destructively reverses partially imported rows.

### D.7 Command surface

- `melodee-cli migrate` retains its current Rust schema-migration meaning (unchanged).
- Legacy migration lives under `melodee-cli legacy` with actions: `inventory`, `plan`,
  `import`, `verify`, `delta`, `status`.
- Machine-readable output (`--json`) is stable for rehearsal comparison; human-readable output
  is the default and is actionable.
- Credentials, session keys, password hashes, and non-actionable filesystem details are
  redacted from all output and logs.

### D.8 Field mappings with no direct Rust representation

| Legacy field | Disposition |
|---|---|
| `Albums.OriginalReleaseDate` | This ADR authorizes one additive, nullable, backward-compatible column `releases.original_release_date date NULL` in the same forward-only migration that creates the `legacy_migration` schema. `Albums.OriginalReleaseDate` maps to it; `Albums.ReleaseDate` maps to `releases.release_date` (edition/release date). Existing rows keep NULL; no existing data is altered. |
| `Users.PasswordHash`/`PasswordEncrypted`/`PasswordHashAlgorithm` | Re-sealed into the Rust credential contract. Only hashes in a supported algorithm list are migrated; anything else yields a `credential.unsupported` issue and the user requires a password reset, reported before cutover. |
| `Users.LastFmSessionKey`, `OpenSubsonicSecretProtected`, provider secrets | Re-sealed into Rust secret storage; never copied as raw protection blobs. |
| `Party*`/`PlayQue*` rows | Not migrated as enabled features (ADR-0133/0140/0141); counted in inventory as excluded by decision. |
| Non-Last.fm provider configuration | Preserved as configuration data only under ADR-0136; no provider implementation is created. |
| `ArtistSearchEngine*` tables | Not migrated; search is rebuilt (D.4). |

Every additional legacy field discovered during R3 without a Rust representation gets an
explicit map row, an intentional transformation, or an accepted exclusion recorded in this
section by amendment before the importer handles it.

### Amendment 2026-08-17 (R3.5 implementation dispositions)

Recorded under the acceptance above; these fix the disposition of legacy tables the original
draft did not enumerate:

| Legacy table/field | Disposition |
|---|---|
| `UserGroups`, `UserGroupMembers` | Imported as `native_library_access_groups` / `native_user_library_groups`. |
| `LibraryAccessControls` | Excluded: legacy Libraries have no Rust entity (media adopted in place); summarized as an `excluded.summary` issue. |
| `SmartPlaylists` | Imported; entity vocabulary `Song/Album/Artist` → `songs/albums/artists`; unknown entities are issues. |
| `PodcastEpisodes` | Imported; `Downloading` requeues as `Queued` (no target lease without ownership); `Downloaded` with a host-specific absolute path requeues as `NotDownloaded` with an informational issue. Episode bookmarks and play history import; stale extra now-playing flags are dropped with an issue. |
| `UserPlaybackSettings` | Imported; out-of-vocabulary `AudioQuality`/`ReplayGain` values map to target defaults with an informational issue. |
| `UserEqualizerPresets` | Imported when `BandsJson` parses as a JSON array of 1–64 bands; otherwise an error issue. |
| `UserSocialLogins` | Imported; provider normalized to the lowercase target vocabulary; malformed provider/subject is an issue. |
| `UserPins` | Artist/Album/Song pins set `pinned` on the matching user-state row (created when absent); Playlist/PodcastChannel pins are reference-only issues (no 1.0 target flag). |
| `Requests`, `RequestComments`, `RequestUserStates` | Imported (category 1–5 and status 1–4 vocabularies are numerically identical; NotSet/out-of-range is an issue). `RequestParticipants` is derived from comments/seen state and summarized as excluded. |
| `UserDeviceProfiles` | Reference-only per-row issues: the 1.0 `device_profiles` table is global metadata (ADR-0134) with no per-user ownership, so importing would invent shared semantics. Operators recreate profiles in the Admin UI when still needed. |
| `SearchHistories`, `ShareActivities`, `JobHistories`, `LibraryScanHistories` | Telemetry; summarized as `excluded.summary` issues. |
| `Charts`, `ChartItems` | Rust charts are regenerated aggregates; summarized as excluded. |
| `PlaylistUploadedFiles`, `PlaylistUploadedFileItems` | Upload staging artifacts; summarized as excluded. |
| `RefreshTokens`, `JellyfinAccessTokens` | Session artifacts never migrate; users re-authenticate. Summarized as excluded. |
| `Players` | Device registry re-establishes on client reconnect; summarized as excluded. |
| `LastFmSessionKey` | Re-sealed into `native_lastfm_links` under the target `security.secretKey` (digest + ciphertext). Without a configured server secret (≥ 32 bytes), each affected user is a `lastfm.unsealable` error issue reported before cutover. |

### D.9 Rollback

Rollback restores traffic and write authority to the legacy system (proxy/DNS switch back after
a rehearsed freeze). It never attempts to reverse partially imported target rows destructively;
the failed/abandoned Rust target is preserved for forensic analysis.

## Alternatives considered

### Copy the legacy database schema into the Rust server

Rejected: the rewrite rule forbids translating the EF Core model into Rust authority; the Rust
schema is already accepted and green. The migration is a mapping boundary, not a schema
continuation.

### ETL export to files (JSON/CSV dumps) instead of direct database reads

Rejected: doubles storage for a 5 TB-library-scale catalog, loses referential checking during
import, and complicates delta detection. Direct read-only snapshot reads plus durable
checkpoints are simpler, auditable, and restart-safe.

### Move/re-encode media during migration for a "clean" layout

Rejected: explicitly prohibited by the 1.0 scope (approximately 5 TB; no mandatory
re-encoding or relocation). Adoption is in place; only newly published Releases use the
Rust canonical layout.

## Rationale

A durable, fenced run model in the target PostgreSQL database makes migration restart-safe and
auditable using the same lease/fencing primitives the rest of the system already proves.
Keying resume on source identity plus source row/version evidence satisfies the idempotency
requirement without trusting display names. Keeping public-ID mapping in its own table keeps
compat-layer identity preservation testable in isolation.

## Consequences

### Positive

- Repeatable, resumable migration with an auditable issue trail.
- Zero source mutation by construction (read-only transaction enforcement).
- Public IDs preserved for native/OpenSubsonic/Jellyfin clients without synchronized upgrades.

### Negative / trade-offs

- Four new internal tables and a CLI command group to maintain.
- Unsupported legacy credential algorithms force password resets for affected users; this is
  reported, not hidden.

### Follow-up work

- R3.2–R3.8 implement this contract literally: schema migration, run model, CLI group,
  inventory/plan, catalog import, user/operational import, media adoption, delta/verify, and
  the integration suite.
- R4 rehearses the full cutover/rollback on production-shaped data.

## Validation

- Integration suites: empty/minimal/mixed/production-shaped legacy fixtures; import twice (no
  duplicate authority); process death and restart at every checkpoint boundary; concurrent-run
  fencing; read-only source enforcement; unsupported-schema fail-closed.
- Verification compares source/target counts, public IDs, relationships, user state, playlist
  ordering, share state, and media references with exact mismatch samples.
- Gate: `make check` plus the migration fixture suites.

## References

- [PRD.md](../../PRD.md)
- [SPEC.md](../../SPEC.md)
- [SPEC.md](../../SPEC.md) §14 (Phase 11) — the release-closure ledger
  (`IMPLMENTATION_OUTSTANDING.md` §8, R3) was removed in the `a42e733` restructure; it remains
  available in git history at `ce3f0cd`. R3 completion evidence: withdrawn
  `design/evidence/r4/adr-0149-decision-package.md` report
- Phase 0 legacy baseline: withdrawn evidence report
  `design/evidence/phase-0/legacy-baseline.md`
- ADR-0133, ADR-0136, ADR-0140, ADR-0141, ADR-0145
