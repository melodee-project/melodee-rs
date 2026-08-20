# R3 — Official Phase 11 Migration Tooling

Date: 2026-08-17. Status: Complete (tooling; production rehearsal is R4). Depends on: R2.
Scope per `IMPLMENTATION_OUTSTANDING.md` §8: accept the migration contract ADR (R3.1), durable
run model (R3.2), inventory/plan (R3.3), catalog import (R3.4), user/operational state import
(R3.5), canonical media adoption (R3.6), delta/verification (R3.7), integration tests and docs
(R3.8).

## 1. ADR-0148 — migration contract (R3.1)

[`../../adr/ADR-0148-legacy-migration-contract.md`](../../adr/ADR-0148-legacy-migration-contract.md)
is **Accepted**. It fixes, rather than delegates, every R3.1 bullet: read-only source role,
cutover authority, durable public-ID mapping, in-place media adoption, search rebuild, durable
run/checkpoint/mapping/issue records, idempotent resume keyed by source identity plus source row
evidence, the unchanged meaning of `melodee-cli migrate`, the distinct `melodee-cli legacy`
command group with `inventory`/`plan`/`import`/`verify`/`delta`/`status`, the prohibition on
destructive source operations, explicit dispositions for fields without a Rust representation
(`§D.8` incl. the 2026-08-17 amendment written during R3.5), and non-destructive rollback
semantics.

## 2. Durable run model (R3.2)

Migration `0032_legacy_migration.sql` (forward-only, additive) creates the `legacy_migration`
schema: `runs` (state machine `planned → importing → verifying → delta-ready → cutover-ready →
complete`, terminal `failed`/`cancelled`, uuidv7 PKs, lease fencing identical to the work-item
engine), `checkpoints` (per-domain keyset progress + counters + elapsed), `entity_map`
(non-catalog mappings, `(domain, target_id)` unique), and `issues` (the audit trail — nothing
is silently dropped). Catalog artist/release/track mappings reuse the Phase-4
`legacy_catalog_id_mappings` table, whose entity check already covers the full catalog
vocabulary. The same migration adds the nullable `releases.original_release_date` column
authorized by §D.8.

Schema contract tests: `crates/melodee-persistence/tests/migrations.rs` —

- `migration_0032_forward_from_prior_schema_preserves_existing_data` — a database at migrations
  0001–0031 with representative rows upgrades forward; seeded content is verbatim afterwards;
  existing releases gain `original_release_date = NULL` (no rewrite).
- `legacy_migration_schema_enforces_the_run_contract` — invalid states, non-v7 ids, negative
  counters, malformed row hashes, and unknown severities are rejected declaratively.
- Existing suite remains green (`migrations_run_from_empty_database`, `migrations_are_repeatable`,
  …): 7/7 passed.

## 3. Command surface (R3.3–R3.7)

`melodee-cli legacy` (module `apps/melodee-cli/src/legacy/`):

| Command | Behavior | Evidence |
|---|---|---|
| `inventory` | Read-only schema fingerprint gate (fails closed on any non-frozen schema), per-domain counts/bounds, media totals from stored metadata only, anomaly pre-flight (duplicate public IDs, broken FKs, invalid timestamps, unsupported credentials, unadoptable media metadata). Writes nothing to the target. | `legacy.rs::inventory_reports_counts_anomalies_and_fingerprint`, `legacy_faults.rs::unsupported_source_schema_fails_closed`, `old_schema_fingerprint_is_rejected` |
| `plan` | Records a durable `planned` run with the inventory + library path maps; no target domain rows. | `legacy.rs::plan_records_planned_run_without_target_writes` |
| `import` | Batched, fenced, idempotent import of every mapped domain in dependency order (users → catalog → tracks/media → contributions → user state → extended state). Resume via `--run`; `--batch-size`; `--canonical-root` read-only media validation. The integration assertion also checks migrated starred/rating/play-count values (`track=(true,5,12)`, `release=(false,4,3)`, `artist=(true,5,0)`), not just row counts. | `legacy.rs::full_import_covers_all_domains_and_preserves_identity`, `second_full_import_creates_no_duplicate_authority`, `source_database_remains_strictly_read_only` |
| `verify` | Per-domain counts, exact unmatched samples, public-ID equality, relationship totals, playlist ordering, orphan mappings, duplicate public IDs, adopted-media existence/size (with `--canonical-root`). Clean runs transition to `delta-ready`; mismatches are durable issues. | `legacy.rs::verify_then_delta_is_convergent`, `production_shaped_source_imports_with_bounded_batches` |
| `delta` | Convergent incremental pass: new rows import, authorized catalog display-field updates apply, source deletions become reconciliation issues (never destructive). A delta over an unchanged source is a no-op. | `legacy.rs::verify_then_delta_is_convergent` |
| `status` | Run state, per-domain checkpoints, counters, ownership; `--json` stable for rehearsal comparison. | `legacy.rs::verify_then_delta_is_convergent` |

## 4. Import domains (R3.4/R3.5)

- **Catalog:** artists (same-name artists stay distinct identities), releases (primary artist
  link, `OriginalReleaseDate` → `original_release_date`, `ReleaseDate` → `release_date`),
  tracks (per-release ordinals computed album-wide — stable across batch sizes and resumes),
  audio identities, media assets, contributions.
- **Identity/users:** users with verbatim public IDs, portable bcrypt credentials preserved,
  unsupported credential algorithms substituted with an unguessable bcrypt placeholder plus a
  `credential.unsupported` issue (pre-cutover reset report), native role flags.
- **User state:** playlists + items (ordering preserved; DEFERRABLE unique constraint handled
  without `ON CONFLICT`), shares (legacy ShareType NotSet/out-of-range → issue; 32-hex
  ShareUniqueId enforced), bookmarks, starred/rating/hated/play-count states, track play
  history (idempotent `legacy-play-<pk>` event ids), pins (Artist/Album/Song set `pinned`;
  playlist/podcast pins reference-only).
- **Operational:** podcast channels (+ episodes with download-status mapping, episode
  bookmarks, episode play history with now-playing dedup), radio stations, user groups +
  memberships, smart playlists (entity vocabulary mapping), playback settings, equalizer
  presets (validated band JSON), social logins, requests + comments + seen state.
- **Re-sealed secrets:** `Users.LastFmSessionKey` is re-sealed into `native_lastfm_links`
  (ChaCha20-Poly1305 under the target `security.secretKey`, SHA-256 digest for lookup). Without
  a configured server secret each affected user is a `lastfm.unsealable` error issue — never a
  copied legacy protection blob.
- **Reference-only/excluded (ADR-0148 §D.8 amendment):** legacy settings
  (`settings.reference_only`), per-user device profiles (`device_profile.reference_only` — the
  1.0 target is global), and one `excluded.summary` issue per non-empty telemetry/session/
  retired table (search history, share activity, job history, scan history, charts, uploaded
  playlist staging, refresh/Jellyfin tokens, library access bindings, request participants,
  player registry, all Party/Jukebox tables).

## 5. Canonical media adoption (R3.6)

Adoption is in place and metadata-driven: `Albums.Directory` + `Songs.FileName` resolve through
`--library-path-map` to canonical-relative paths; traversal/absolute/backslash components are
rejected (`media.unadoptable`). With `--canonical-root` mounted, each adopted file is validated
read-only (containment through symlink resolution, existence, expected size) — missing or
mismatched files are warnings, an escape skips the track. Without the flag, one informational
`media.verification_skipped` issue per run makes the metadata-only nature auditable. Legacy
hashes are adopted only when already valid 64-lowercase-hex; everything else becomes the
never-authoritative hash-pending sentinel (64 zeros), excluded from dedup/identity/deletion
decisions.

Evidence: `legacy.rs::canonical_root_validation_is_read_only_and_actionable` — sparse 5 GiB
adopted file (scale metadata without bytes), a missing file, a size mismatch, and a symlink
escape, with the media tree byte-identical afterwards;
`legacy.rs::adoption_without_canonical_root_records_verification_skipped`;
`legacy_faults.rs::malformed_media_metadata_becomes_issue_not_unsafe_path`.

## 6. Integration suite summary (R3.8)

Suites spawn the real CLI binary against disposable PostgreSQL databases: the source is seeded
from the frozen legacy DDL fixture (extracted from the baseline EF Core model snapshot, 52 user
tables) and the target is freshly migrated.

- `apps/melodee-cli/tests/legacy.rs` — **11/11 passed** (inventory, fingerprint gate, read-only
  source proof, plan, full import across every mapped domain with verbatim identity, media
  validation on/off, second-import idempotency, verify→delta convergence, production-shaped
  500-artist/1000-release/12000-track import at batch size 997 with clean verify and
  crash-resume convergence).
- `apps/melodee-cli/tests/legacy_faults.rs` — **7/7 passed** (concurrent-owner fencing, stale
  lease reclaim + restart convergence, orphan references as issues, target public-ID conflict
  without overwrite, traversal rejection, empty-source verify, old-schema rejection).
- `crates/melodee-persistence/tests/migrations.rs` — **7/7 passed** (incl. the two new
  contract tests above).

Process-death coverage: the CLI exits between every command (each command is a separate
process), mid-run death is simulated by expiring the lease with committed checkpoints intact,
and restart convergence is asserted. Concurrent invocation: `fencing_blocks_a_second_live_owner`.

## 7. Boundary to R4

R3 delivers rehearsal-ready tooling. Not claimed here: production-sized (~5 TB) rehearsal,
dual-run differential traffic, physical-client acceptance, production OIDC/Last.fm/SMTP
integration, write-freeze/cutover/rollback drills, and the `cutover-ready` operator
acknowledgement transition — all R4 (`IMPLMENTATION_OUTSTANDING.md` §9). `PRD-MIG-007`
(rollback) remains open until R4 proves the return path.

## 8. Requirement traceability

Closed by R3 with the evidence above: `PRD-MIG-001` (media preserved in place), `PRD-MIG-002`
(public identity), `PRD-MIG-003` (user state), `PRD-MIG-004` (catalog relationships),
`PRD-MIG-005` (no mandatory upfront hashing). `PRD-MIG-006` (search rebuildable) was already
tested. `PRD-MIG-007` stays open for R4. Registry regenerated via
`python3 scripts/extract_requirements.py`.

## 9. Gate results

- Focused: `cargo test -p melodee-cli --test legacy --test legacy_faults` — 18/18 passed.
  `cargo test -p melodee-persistence --test migrations` — 7/7 passed.
- Full: `./scripts/check.sh` — see the commit message of the R3 landing commit for the exact
  counts at merge.

## 10. Compatibility impact

None of the external contracts change. No native v1, OpenSubsonic, or Jellyfin wire DTO, route,
status code, or auth flow was touched; the migration writes only target-internal tables and the
accepted additive `releases.original_release_date` column. Legacy public IDs are preserved so
existing clients keep working post-cutover without a synchronized upgrade.

## 11. Data/migration impact

New schema `legacy_migration` (+4 tables) and one nullable column on `releases`; forward-only;
no existing row altered (proven by the forward-migration test). The tooling never writes to the
source database (read-only session enforcement proven by test) and never writes to media roots.

## 12. Security and data-loss impact

Traversal, symlink-escape, and malformed-path inputs are rejected before any filesystem
conclusion; secrets are re-sealed or reported, never copied raw; issue detail redacts
credentials. No destructive operation exists in the tooling: deletions are issues, rollback is
non-destructive by design (ADR-0148 §D.9).

## 13. Documentation impact

- Updated — `docs/src/content/docs/installation/legacy-migration.md` (new operator guide),
  `docs/src/content/docs/installation/upgrading.md`,
  `docs/src/content/docs/getting-started/quick-start.md`,
  `docs/src/content/docs/troubleshooting/index.md`, `docs/src/content/docs/index.md`
  (migration tooling references with truthful rehearsal status),
  `design/adr/README.md` (release-closure ADR index), ADR-0148 (accepted + §D.8 amendment),
  requirement registry artifacts.

## 14. Material unresolved risks

- Production-sized rehearsal and rollback proof — owner R4.
- `cutover-ready` operator acknowledgement transition (schema exists; the freeze/cutover
  commands that drive it are R4 scope) — owner R4.
- Legacy `Libraries.Path` join: adoption resolves relative directories under the longest-match
  root mapping; multi-library sources with distinct roots should pass absolute `--library-path-map`
  entries (documented) — owner R4 rehearsal validation.
