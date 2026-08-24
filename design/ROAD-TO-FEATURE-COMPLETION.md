# Road to Feature Completion

Status: DRAFT (pending the Phase 0 architecture ADRs). Last updated: 2026-08-23.

This is the **forward-looking plan** for taking Melodee from "most features ported and proven" to
**v1.0.0 feature-complete** — every capability of the C# Melodee application available in this
Rust implementation, each implemented in a more modern, performant, Rust-idiomatic way rather than
ported line-for-line.

## 1. Scope and framing

- **Feature-complete means:** the same *user-visible features and behaviors* as the C# Melodee
  application (the frozen oracle at commit `4028b58d`, see
  [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) §2.2), implemented in Rust and proven
  against that baseline.
- **All C# features are in scope for v1.** Nothing is waived. This program closes every blocking
  row by either implementing the feature in Rust (proven) or — only where an accepted ADR already
  records a deliberate, *approved* difference — leaving that row as `DIFFERENT — APPROVED`. No
  further product-scope reductions are assumed.
- **"Done better" is a real requirement, not an exception.** Where the C# architecture is
  architecturally inferior (string-keyed identity lookup, inline blocking enrichment, Quartz job
  scheduling, SignalR coupling, server-rendered Blazor UI), the Rust implementation is allowed —
  and expected — to diverge *architecturally* while preserving *behavior*. Every such divergence
  is recorded in an ADR and proven equivalent-or-better by differential/property tests.
- **This document is the plan.** [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) remains
  the *audit/evidence* record (current state, per-row status, risk register). This document is
  *what we will do and in what order*. They are complementary, not duplicates.
- **Authority order is unchanged** (see [AGENTS.md](../AGENTS.md)): accepted ADRs, `SPEC.md`,
  `PRD.md`, `IMPLEMENTATION.md`, then this plan. A material conflict is recorded, never silently
  resolved.
- **Ground-up rule honored:** each block below is framed as "implement `PRD-XXX` per `SPEC.md`,
  using the C# service and its tests as behavioral evidence" — not "port `CSharpClass.cs`".

## 2. Why this is mostly *wiring*, not *porting*

The "better" architecture for the largest remaining gaps is **already designed and unit-tested in
this repository** — it simply has zero production callers yet. This is the single most important
fact about the plan: the hard architectural thinking is done, and the program is mostly (a) wiring
existing design into production, (b) implementing the missing pieces against those seams, and
(c) proving parity against the frozen oracle.

| Already-designed seam | Location | Status today |
|---|---|---|
| Deterministic, bounded, evidence-scoring Artist resolver (candidate limits, net-score thresholds, provisional-on-ambiguity, negative assertions) | `crates/melodee-catalog/src/resolver.rs` | Implemented + unit-tested; **zero production callers** |
| Provider-neutral `ArtistAuthority` trait — external providers *propose evidence but never allocate Melodee identities* | `crates/melodee-catalog/src/authority.rs` | Implemented + unit-tested; only a `#[cfg(test)]` fake implements it |
| `external_provider_cache` + `ProviderCacheStore` (cache + provenance home, migration 0028) | `crates/melodee-persistence/` | Schema + store exist; **test-exercised only** |
| Durable work-item engine (leases, `FOR UPDATE SKIP LOCKED`, crash-safe) | `crates/melodee-persistence/` + `apps/melodee-server/` | In production; the correct *replacement* for C# Quartz |
| Idempotent transactional catalog mutations with in-transaction audit events | `crates/melodee-persistence/src/catalog_mutations.rs` | In production |
## 3. The gaps (from the 2026-08-23 audit)

28 of 50 matrix rows block v1.0.0: **9 `IMPLEMENTED — UNPROVEN`, 15 `PARTIAL`, 3 `MISSING`,
1 `DIFFERENT — UNAPPROVED`** (full detail in
[ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) §5 and
[FEATURE-PARITY-MATRIX.md](requirements/FEATURE-PARITY-MATRIX.md)). Grouped by the work they
actually require:

- **Genuine bug (P0):** RELEASE-002 — the admin SPA metadata-edit save path targets a nonexistent
  `/metadata` route (`admin-ui/src/api/admin.ts:783,790,797`) plus field-name mismatches
  (`name`↔`canonicalName`, `trackNumber`↔`ordinal`). Edits silently 404 today.
- **Unwired "better" design (high leverage):** ARTIST-001 (cross-Release identity reuse),
  PROVIDER-001 / ARTIST-002 / ARTWORK-001 / JOBS-001 (provider enrichment), CONFIG-001 (settings
  inert), TRACK-001/002 (external-ID retention + AudioIdentity matching).
- **Evidence, not code (the 9 UNPROVEN rows):** AUTH-001, OPS-001, MIGRATE-001, MIGRATE-002,
  PLAYBACK-001, PODCAST-001, DISCOVERY-001, DISCOVERY-002, SOCIAL-001 — mostly differential
  captures + e2e/golden tests + R4 production legs.
- **Net-new surfaces (largest effort):** WEBUI-001 (end-user web UI), ADMINUI-001 (admin UI
  completion + edit forms), PLAYBACK-002 (MPD/MPV backends).
- **Smaller PARTIAL/MISSING:** MEDIA-001, RADIO-001/002, RELEASE-001, CLI-001, AUDIT-001,
  SCRIPT-001, PACKAGING-001.

## 4. "Done better" reference table

Per feature, the C# approach and the Rust-native replacement this program targets:

| C# feature | C# approach (behavioral evidence only) | Rust-native "better" target |
|---|---|---|
| Cross-Release Artist identity | `LibraryInsertJob.cs` string-keyed `mb:`/`sp:`/`it:`/`name:` dict lookup | Wire `LocalArtistResolver` + `ArtistAuthority` into import: evidence scoring, confidence thresholds, provisional-on-ambiguity, durable decision + negative assertions, audit outbox (extends ADR-0153) |
| Provider enrichment at ingestion | Hardwired search-engine services, inline, blocking in staging | Asynchronous enrichment **worker** on the durable work-item engine; providers behind the `ArtistAuthority`/provider traits; cached + provenance-recorded in `external_provider_cache`; **fail-degraded** (never blocks catalog entry — PRD-EXT-002 invariant); per-provider rate limiting |
| MusicBrainz/external ID retention | Opaque MB-ID columns on catalog rows | Structured **external-ID evidence** per Melodee identity (never a string identity key), feeding the resolver |
| Metadata edit forms + tag rewrite | `MediaEditService` 12 transforms | Idempotent transactional catalog mutations with in-transaction audit (already in `catalog_mutations.rs`), field-level validation, SPA save-path fix + full test coverage |
| End-user web UI | Blazor server-rendered + SignalR | Modern SPA on the ADR-0138/0139 stack, consuming native-v1 over WebSocket/SSE (no SignalR), Carbon components, localized + RTL |
| Scheduled jobs | Quartz, per-job `JobHistories` | Keep the durable work-item engine as the runtime (crash-safe, lease-based — stronger than Quartz); add a per-scheduled-job **run-history** projection for operator visibility |
| Rate limiting / middleware | Named policies | Per-surface token-bucket middleware with documented budgets + differential evidence |

## 5. Phases

### Phase 0 — Architecture ADRs (no code; prerequisite)

Lock the "better" design choices so each build block has an accepted design. Each is drafted with
`python3 scripts/new_adr.py "short title"` and reviewed before the dependent phase starts.

1. **Supersede ADR-0136** — provider enrichment is a **v1 obligation** (build, not waive). Defines
   the provider adapter boundary, the enrichment worker, cache/provenance/rate-limit design on
   `external_provider_cache`, and the fail-degraded contract.
2. **ADR: end-user web UI surface** — SPA architecture, route/serving, session strategy (extend
   ADR-0139), realtime channel (WebSocket vs SSE), component reuse from the admin SPA.
3. **ADR: MPD/MPV playback backends** — backend abstraction + endpoint registry; parity scope for
   C# backend connectivity.
4. **ADR: metadata-edit forms scope** — resolves the Phase 10 deferral: full edit forms, which C#
   tag-rewrite transforms are in-scope, plus the SPA route/field contract.
5. **ADR: scheduled-job run history** — work-item history as source of truth + a per-scheduled-job
   run-history projection (the "better than `JobHistories`" design).
6. **ADR: OS stored-artwork `?size=`** — capture C# behavior, then decide match-vs-native-resize
   (capture-backed, per the compatibility rules in
   [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) §2.4).

**Exit:** all six ADRs accepted (or the decision owner has explicitly deferred one, recorded).

### Phase 1 — Quick wins + the P0 bug (start immediately; no ADR needed)

- **RELEASE-002 (P0):** fix the broken SPA save path — register the `/metadata` route server-side
  *or* fix `admin-ui/src/api/admin.ts:783,790,797` + the field-name mismatches; add the zero-test
  server + store integration tests and an admin-ui e2e save test.
- **Close the 9 `IMPLEMENTED — UNPROVEN` rows** — evidence, not code: differential captures vs the
  frozen oracle (`python3 scripts/legacy_oracle.py capture`), e2e/golden tests, and R4
  production-integration legs (AUTH-001 real Google OIDC, OPS-001 per-surface rate-limit,
  PLAYBACK-001, PODCAST-001, DISCOVERY-001/002, SOCIAL-001, MIGRATE-001/002 R4).
- **TRACK-001/002:** MusicBrainz/external-ID retention as structured evidence (schema ADR +
  migration + extraction in `melodee-media`, feeding the resolver); AudioIdentity matching on top.

**Exit:** RELEASE-002 `PROVEN`; most UNPROVEN rows `PROVEN`; TRACK gap closed or ADR'd.

### Phase 2 — Artist identity + provider enrichment (core "better" block)

Gated on Phase 0 ADRs 1 and 4. This is where the unused `resolver.rs` / `authority.rs` /
`external_provider_cache` finally go live.

- **ARTIST-001 (`DIFFERENT — UNAPPROVED` → `PROVEN`):** wire `LocalArtistResolver` +
  `ArtistAuthority` into catalog import (extend the ADR-0153 singleton pattern to cross-Release),
  replacing fresh-`Uuid::now_v7()`-per-observation at `catalog.rs:1057`. Add a **data-quality
  property test**: same artist across N releases → exactly one identity. Top correctness priority
  — a silent, compounding data bug.
- **PROVIDER-001 / ARTIST-002 / JOBS-001 / ARTWORK-001:** enrichment worker + provider adapters
  (MusicBrainz, Discogs, Spotify, iTunes, Deezer, Wikidata, Brave, Metal-API + Last.fm lookup legs)
  behind the existing provider-neutral boundary; live `external_provider_cache` writes with
  provenance; honor `searchEngine.*` settings (currently inert, `settings.rs:462-484`);
  provider-discovered artwork; enrichment jobs on the work-item engine. Differential evidence that
  staging enrichment matches or exceeds C#.

**Exit:** ARTIST-001 `PROVEN`; PROVIDER/ARTIST-002/ARTWORK-001/JOBS-001 `PROVEN`.

### Phase 3 — End-user web UI (net-new surface; largest single block)

Per Phase 0 ADR 2: the full user product (dashboard, MQL search, music player, stats, public share
landing, account flows) as a modern SPA on the native-v1 API. Per-screen checklist,
Playwright/visual, Carbon-first, localized + RTL.

**Exit:** WEBUI-001 `PROVEN` (per-screen) with e2e + visual evidence.

### Phase 4 — MPD/MPV backends + remaining PARTIAL rows

- **PLAYBACK-002:** backend abstraction + endpoint registry (per ADR 3).
- **ADMINUI-001:** close the expanded admin gap (libraries/playlists/podcasts/shares/requests/
  chart-editor/pins/media-artist screens) + full metadata-edit forms (shares the Phase 1
  RELEASE-002 save-path fix).
- **CONFIG-001** (settings go live via Phase 2), **MEDIA-001** (NFO fields, `.txt` lyrics,
  Blackbeard provenance), **RADIO-001/002** (charts/recommendations), **RELEASE-001** (Release
  dedup), **CLI-001** (missing commands), **AUTH-001** (roles/profiles depth), **AUDIT-001**
  (run history per ADR 5), **SCRIPT-001** (close to the ADR-0135 boundary).

**Exit:** each row `PROVEN` or an explicitly recorded approved difference.

### Phase 5 — Migration / cutover / packaging

MIGRATE-001/002 R4 production rehearsal + differential closure + R5 cutover; a
`LibraryValidate --fix` equivalent; PACKAGING-001 RC acceptance (close the 8-critical / 187-high
finding-policy failures) + release artifacts (ADR-0144/0149/0151). Operator-owned; on the critical
path for a real v1.

**Exit:** MIGRATE-001/002 `PROVEN`, PACKAGING-001 `PROVEN`.

### Phase 6 — Final re-audit → v1.0.0

Re-run the full audit and gates: `make check`, `make check-fast`,
`python3 scripts/validate_repository.py` (after the 45 pre-existing link errors in
`ROAD-TO-FEATURE-PARITY.md` are repaired), and `python3 scripts/extract_requirements.py` if PRD
IDs change. Flip every matrix row to `PROVEN PARITY` or an explicitly recorded `DIFFERENT —
APPROVED` for an intentional native improvement. Update §1 of
[ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) from "cannot be asserted" to a v1.0.0
completeness statement, with the accepted-difference list in its §8.

**Exit (Definition of Done):** all 50 rows terminal; verdict updated; all gates green.

## 6. Dependency graph

```text
Phase 0 (ADRs 1-6) ──┬─► Phase 1 (quick wins + RELEASE-002 bug; independent)
                     ├─► Phase 2 (ARTIST / PROVIDER)   [needs ADR 1, 4]
                     ├─► Phase 3 (end-user web UI)     [needs ADR 2]
                     ├─► Phase 4 (MPD/MPV + PARTIAL)   [needs ADR 3 + Phase 2 for settings]
                     └─► (all) ──► Phase 5 (migration / packaging)
                                          └─► Phase 6 (final re-audit → v1.0.0)
```

## 7. Effort ranking (largest → smallest)

1. **Phase 3 — end-user web UI.** Net-new product surface; the largest block.
2. **Phase 2 — provider enrichment + Artist identity.** Most *architecturally* significant, but
   the design seams already exist (highest leverage).
3. **Phase 4 — admin UI completion + edit forms.** Sizable UI work.
4. **Phase 1 — quick wins.** Mostly test/evidence work; fast wins; the RELEASE-002 bug is urgent.
5. **Phase 5 — migration / packaging.** Operator-heavy; on the critical path.

## 8. Open items before Phase 0 ADRs can be accepted

- **Provider enrichment (ADR 1):** full C# provider set (all 8 + Last.fm lookup) vs a Rust-native
  "better" default (e.g. MusicBrainz + Last.fm + one image source first, rest behind the same
  adapter boundary). Decision owner: product.
- **End-user web UI (ADR 2):** confirm the "modern SPA on native-v1 API" direction and first-class
  v1 status. Decision owner: product.
- **MPD/MPV (ADR 3):** confirm which backends ship in v1. Decision owner: product.

## 9. Definition of done (program-level)

Per `design/agents/DEFINITION-OF-DONE.md`, feature-complete is reached when, for every one of the
28 blocking rows: the requirement is implemented in Rust per `SPEC.md`, the behavior is proven
against the frozen C# oracle (differential/e2e/golden/property tests as appropriate), any
architectural divergence has an accepted ADR, requirement traceability
([REQUIREMENTS-INDEX.md](requirements/REQUIREMENTS-INDEX.md)) is updated, the affected
documentation is updated in the same change, and all gates (`make check`) pass.

---

Related documents:
- [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) — current-state audit and risk register.
- [FEATURE-PARITY-MATRIX.md](requirements/FEATURE-PARITY-MATRIX.md) — the 50-row matrix (SSOT).
- [REQUIREMENTS-INDEX.md](requirements/REQUIREMENTS-INDEX.md) — requirement status traceability.
- [SPEC.md](SPEC.md) — technical invariants and interface contracts.
- [ADR-0136](adr/ADR-0136-defer-non-last-fm-external-provider-integrations-to-post-parity.md) —
  the deferral this program supersedes (ADR 1 of Phase 0).
- [ADR-0135](adr/ADR-0135-boa-javascript-runtime-and-sandboxing-for-scripting-hooks.md) —
  scripting runtime boundary (SCRIPT-001).
- [ADR-0138](adr/ADR-0138-admin-spa-tooling-and-static-serving.md) and
  [ADR-0139](adr/ADR-0139-admin-spa-session-strategy-and-admin-api-gating.md) — SPA tooling and
  session strategy that the end-user web UI (ADR 2) extends.
- [ADR-0153](adr/ADR-0153-reuse-unambiguous-release-artist-identity-for-matching-tracks.md) —
  the Artist-identity pattern Phase 2 generalizes to cross-Release.
- [PHASE-10-REPORT.md](evidence/phase-10/PHASE-10-REPORT.md) — current admin UI state (Phase 4
  baseline).
- [PHASE-9-REPORT.md](evidence/phase-9/PHASE-9-REPORT.md) — current provider/parity state (Phase 2
  baseline).

