# Road to Feature Completion

**Document status:** DRAFT planning proposal — **non-authorizing**. Last updated: 2026-08-25.

> ## Current disposition (2026-08-25)
>
> **Feature-Completion Phase 1 is complete** per
> [agents/FEATURE-COMPLETION-PHASE-1-PLAN.md](agents/FEATURE-COMPLETION-PHASE-1-PLAN.md)
> and [evidence/feature-completion/phase-1/PHASE-1-REPORT.md](evidence/feature-completion/phase-1/PHASE-1-REPORT.md).
> Feature-Completion Phase 2 is complete, with its execution map at
> [agents/FEATURE-COMPLETION-PHASE-2-PLAN.md](agents/FEATURE-COMPLETION-PHASE-2-PLAN.md) and evidence
> at [evidence/feature-completion/phase-2/PHASE-2-REPORT.md](evidence/feature-completion/phase-2/PHASE-2-REPORT.md).
> It owns identity convergence, all ADR-0154 providers and purposes, durable enrichment, provider
> lookup, and artwork. The older W0-only authorization text below is historical and superseded.

> ## Listener product decision (ADR-0161)
>
> MeloAmp owns listener browsing, search, playback, playlists, ratings, recommendations, and
> related listener workflows. The Rust server owns APIs, ingestion, storage, streaming, operations,
> and `/admin`. `WEBUI-001` is `DIFFERENT — APPROVED`, relocated to MeloAmp; the duplicate listener
> SPA phase and server backlog blocker are removed from the active plan. Narrow HTML pages remain
> allowed only for a genuinely required public-link or security flow.

> Current Phase 2 row dispositions: `ARTIST-001/002` and `RELEASE-001/002` are implemented on
> the production importer/resolver and audited correction paths; `TRACK-001/002` has six-family
> evidence and bounded Proposed AudioIdentity decisions; `PROVIDER-001`/`ARTWORK-001` has all
> accepted provider purposes, the pinned DecentDB read path, cache/worker/artwork/API fixtures,
> with real credentials and full production dump/warmup rehearsal still external; `CONFIG-001`
> provider settings have runtime consumers; `JOBS-001` provider maintenance is implemented while
> unrelated legacy jobs remain tracked; and `WEBUI-001` is `DIFFERENT — APPROVED` under ADR-0161.

> ## Feature-Completion Phase 4 — terminal disposition (2026-08-25)
>
> Phase 4 was explicitly assigned and is complete against the entry plan at
> [agents/FEATURE-COMPLETION-PHASE-4-PLAN.md](agents/FEATURE-COMPLETION-PHASE-4-PLAN.md). Its
> authoritative evidence is
> [evidence/feature-completion/phase-4/PHASE-4-REPORT.md](evidence/feature-completion/phase-4/PHASE-4-REPORT.md).
> It closed the remaining implementation-level operational gaps: live MPD/MPV backend control,
> release-scoped safe tag transformations, durable scheduled-job history and maintenance,
> stored-artwork sizing, verified Admin UI/operator gaps, local authentication depth, scripting
> and CLI/configuration closure, and local differential evidence. Every owned row is locally
> evidenced or has an explicit accepted difference/operator-owned disposition in the report.
> It did not authorize migration, production cutover, production-library mutation, live provider
> credentials, real Google OIDC, or physical-client acceptance.
>
> The historical Phase 4 proposal below is preserved as historical planning text. Where it says
> metadata tag transforms, job history, or stored-artwork sizing are absent, the terminal Phase 4
> plan/report and current source are authoritative. Phase 11 migration/import, packaging,
> rehearsal, cutover, rollback, and production-library validation are the only planned
> implementation milestone after this closeout.

> ## Canonical Phase 11 current assessment (2026-08-25)
>
> Canonical Phase 11 has now been assigned and its safe local work is in execution. The current
> plan and authoritative evidence are [PHASE-11-PLAN.md](agents/PHASE-11-PLAN.md) and
> [PHASE-11-REPORT.md](evidence/phase-11/PHASE-11-REPORT.md). The migration CLI now has a
> preview-first `legacy validate` surface with confirmation-gated, idempotent target metadata
> repair; the rollback runbook emits topology-bound steps and records durable audit evidence; and
> the 12,000-Track synthetic drill covers inventory, baseline, import, verification, media
> validation, search rebuild, backup/restore, freeze, cutover-ready fencing, and retained-target
> rollback mechanics. A current local amd64 Trixie image also passed the PostgreSQL-backed
> non-root/container-tool/Admin UI/CLI smoke.
>
> This is not production completion. Production/private snapshot and media access, production-
> sized rehearsal, final delta/write freeze/traffic switch, production rollback, live-provider and
> OIDC checks, current candidate SBOM/vulnerability scan, arm64 execution, provenance attestation,
> registry publication, and rollback-asset cleanup remain explicit operator gates. Historical
> Phase 11 planning text below is retained; this addendum and the Phase 11 report are current.

> Historical Phase 1 revision text:
>
> Workstreams A–D were RELEASE-002 metadata-editing proof, TRACK-001/002 MusicBrainz-ID
> retention + AudioIdentity import-time evidence/proposals, local proof for the nine
> `IMPLEMENTED — UNPROVEN` rows, and closeout. Evidence was recorded at
> [evidence/feature-completion/phase-1/PHASE-1-REPORT.md](evidence/feature-completion/phase-1/PHASE-1-REPORT.md)
> The 2026-08-23 note's W0-only authorization paragraph is retained below as historical text.

> ## Revision note (2026-08-23) — historical (W0-only authorization portion superseded, see 2026-08-25 note above)
>
> The committed Phase 1 draft of this document ("Road to Feature Parity: 50-Row Audit + Work
> Plan") described a hypothetical 50-row v2 re-audit and a Phase 0–5 renumbered plan. The current
> working copy supersedes that draft with the **2026-08-23 audit** recorded in
> [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) (28 of 50 canonical-matrix rows block
> v1.0.0; RELEASE-002 is a genuine P0 bug) and the W0–W9 workstream ownership in §3.1. Where the
> two disagree, the 2026-08-23 audit text is the plan of record.
>
> **Authorized scope inside this non-authorizing document:** per the Phase-boundary update in
> [AGENTS.md](../AGENTS.md), the **W0 workstream (the `RELEASE-002` P0 metadata-edit contract and
> save-path fix — server routes, field contract, SPA save path, and its server/store/UI tests and
> docs) is authorized to proceed now** as a bug fix to existing behavior. Everything else in this
> document (the remaining Phase 1 quick wins, W1–W9, the six proposal-stage "Phase 0" ADRs, and
> all later blocks) remains unassigned and non-authorizing.

> ## ⚠️ Authorization status (read first)
>
> - This document is a **DRAFT planning proposal only**. It does **not** assign, authorize, or
>   schedule the next canonical repository phase. It is not a work assignment, and no block below
>   may be treated as permission to begin implementation.
> - **Canonical Phases 0–10 are complete.** Their evidence is recorded in
>   [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) and the phase reports linked below.
>   Feature-Completion Phase 2 is complete under its phase report; remaining live-provider,
>   production-replica, differential, and cutover legs remain explicitly operator-authorized.
> - **Migration/cutover is the existing, unassigned Phase 11 domain** (ADR-0148/0149/0150). It is
>   *not* a newly numbered phase, and its production legs (rehearsal, cutover, rollback, release
>   acceptance) belong to that existing Phase 11 scope.
> - Where this document uses the words "phase" for its own build order, they are **proposal-stage
>   names** ("decision gate", "stage", "workstream") and are explicitly *not* the canonical Phase
>   0–6 numbering. They do not override the authority order in
>   [AGENTS.md](../AGENTS.md).
> - Several rows below reference **historical product decisions**. Provider breadth, scheduled-job
>   history, and OS `?size=` are governed by accepted ADRs 0154–0160. `WEBUI-001` is resolved by
>   accepted ADR-0161 and relocated to MeloAmp; it is not an active server SPA workstream. The
>   Phase 0 ADRs (ADR-0154…0161) are accepted and recorded in
>   [ADR-0154](adr/ADR-0154-external-provider-enrichment-scope-and-runtime.md) and the
>   [ADR index](adr/README.md). No product decision blocks the Phase 0 ADRs any longer. The
>   implementation of the assigned Phase 2 workstreams is recorded in the Phase 2 report.

This is the **forward-looking proposal** for taking Melodee from "most features ported and proven"
to **v1.0.0 feature-complete** — every *in-scope* capability of the C# Melodee application
available in this Rust implementation, each implemented in a more modern, performant,
Rust-idiomatic way rather than ported line-for-line. It is a proposal for *what the next
authorization may ask for, in what order, and under which accepted decisions* — not an order to
do it.

### Coding-agent delegation unit

This roadmap's workstreams and waves are internal decomposition, not the intended size of separate
coding-agent prompts. This document alone remains non-authorizing, but when the user explicitly
asks for a coding-agent prompt to accomplish the next phase, the prompt must delegate the complete
next authorized phase or program milestone described here. It must include all of that phase's
entry decisions, workstreams, implementation, tests, public documentation, traceability, evidence,
and exit review.

Do not turn a phase request into a prompt for only the first open row, W-number, quick win,
documentation repair, or ADR. Put safe prerequisites in the phase's first wave and let the
receiving agent decompose and execute the work end to end. Use a single-workstream prompt only when
the user explicitly requests task-level granularity or a reserved external decision genuinely
blocks every other safe part of the phase.

## 1. Scope and framing

- **Feature-complete means:** the same *user-visible features and behaviors* as the C# Melodee
  application (the frozen oracle at commit `4028b58d`, see
  [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) §2.2), implemented in Rust and proven
  against that baseline.
- **Scope is four distinct sets, not one blanket claim.** Do not conflate:
  1. **The complete C# capability inventory** — every user-visible feature in the frozen oracle
     (the 50-row matrix in [ROAD-TO-FEATURE-PARITY.md](ROAD-TO-FEATURE-PARITY.md) §6).
  2. **Accepted `DIFFERENT — APPROVED` outcomes** — rows where an *accepted* ADR already records a
     deliberate, approved difference (e.g. the Party/jukebox waiver ADR-0133, the retired web
     routes ADR-0140/0141, the deferred non-Last.fm provider integrations ADR-0136). These are
     *accepted waivers/deferrals*, not parity proof.
  3. **Proposed v1 obligations** — the rows this program would close by implementing them in Rust
     (proven) once the next phase is authorized.
  4. **Post-v1 or still-undecided capabilities** — capabilities whose v1 inclusion is itself an
     open product decision (`WEBUI-001`, `PLAYBACK-002`, provider breadth) or explicitly deferred
     to post-parity work (ADR-0136).
  The program does **not** claim "nothing is waived": it accepts the existing accepted
  waivers/deferrals, and it assumes **no *further* product-scope reductions** beyond those already
  recorded in accepted ADRs — unless a future phase explicitly proposes and accepts a superseding
  ADR.
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
- **Evidence, not code (the 9 `IMPLEMENTED — UNPROVEN` rows):** OPS-001, MIGRATE-001, MIGRATE-002,
  PLAYBACK-001, TRANSCODE-001, PODCAST-001, DISCOVERY-001, DISCOVERY-002, SOCIAL-001. These have a
  reachable production path but lack proof. Ordinary evidence (differential captures vs the frozen
  oracle, e2e/golden tests) can be prepared now. The **external R4 production legs** (real Google
  OIDC, physical clients, production integrations, the real ~5 TB library) are operator-authorized
  and belong after implementation/schema freeze (§5, Stage 6) — they are not part of ordinary
  evidence work.
- **`PARTIAL` rows:** CONFIG-001, AUTH-001 (real-provider OIDC e2e + roles/profiles depth),
  MEDIA-001 (NFO fields, `.txt` lyrics, Blackbeard provenance), RELEASE-001, RELEASE-002,
  TRACK-001/002, PROVIDER-001, ARTWORK-001, JOBS-001, SCRIPT-001, AUDIT-001, ADMINUI-001,
  CLI-001, PACKAGING-001, and the two product-decision rows PLAYBACK-002 / WEBUI-001.
  `AUTH-001` is `PARTIAL`, not `IMPLEMENTED — UNPROVEN`: its Google OIDC flow exists and is
  loopback-tested; what is missing is real-provider proof plus roles/profiles depth.
- **Net-new surfaces (remaining):** ADMINUI-001 (admin UI completion + edit forms) and
  PLAYBACK-002 (MPD/MPV backends — separate product scope). WEBUI-001 is an approved MeloAmp
  relocation under ADR-0161, not an active Rust server surface.

### 3.1 The 28 v1.0.0 blockers — exactly one primary owning workstream each

Each of the 28 blocking rows is assigned to **exactly one primary owning workstream**. Rows that
depend on other workstreams list those as *secondary dependencies* only; a secondary dependency is
never a second primary owner. A workstream below is a **proposal** — it is not started until the
next phase is assigned and authorized. The statuses shown are the audited 2026-08-23 statuses
(24 `PARTIAL`/`IMPLEMENTED — UNPROVEN`/`MISSING`/`DIFFERENT — UNAPPROVED` + 3 open product
decisions; the two product-decision rows and MIGRATE-002's phrasing are exactly as audited).

| Feature ID (status) | Primary owning workstream | Secondary dependencies |
|---|---|---|
| `RELEASE-002` (`PARTIAL`, P0 bug) | W0 — metadata-edit contract & save-path fix | — |
| `TRACK-001` (`PARTIAL`) | W1 — external-ID retention & AudioIdentity | Schema decision ADR-0117; W2 (resolver consumes the evidence) |
| `TRACK-002` (`PARTIAL`) | W1 — external-ID retention & AudioIdentity | W0 (same ADR-0117 schema decision) |
| `ARTIST-001` (`DIFFERENT — UNAPPROVED`) | W2 — local Artist/Release/AudioIdentity resolution | W1 (external-ID evidence); W3 (enrichment worker feeds candidates) |
| `CONFIG-001` (`PARTIAL`) | W3 — provider enrichment worker | Decision gate (provider scope); ADR-0118/0147 where transcode/settings overlap |
| `PROVIDER-001` (`PARTIAL`) | W3 — provider enrichment worker | Decision gate (provider scope); W2 (resolver) |
| `ARTIST-002` (`PARTIAL`) | W3 — provider enrichment worker | W2 (resolver consumes provider evidence) |
| `JOBS-001` (`PARTIAL`) | W3 — provider enrichment worker | Decision gate (scheduled-job run history) |
| `ARTWORK-001` (`PARTIAL`) | W4 — provider-discovered artwork & OS stored-artwork `?size=` | W3 (enrichment); Decision gate (OS `?size=`); ADR-0143 |
| `WEBUI-001` (`DIFFERENT — APPROVED`) | Approved relocation to MeloAmp; no Rust server workstream | ADR-0161; MeloAmp native-v1 acceptance |
| `PLAYBACK-002` (product decision) | W6 — playback backends (MPD/MPV) | Decision gate (MPD/MPV scope) |
| `AUTH-001` (`PARTIAL`) | W7 — auth/roles/profiles closure | R4 external leg (real Google OIDC) after freeze |
| `MEDIA-001` (`PARTIAL`) | W7 — remaining PARTIAL closure | ADR-0117/0118 (metadata schema) |
| `RELEASE-001` (`PARTIAL`) | W7 — remaining PARTIAL closure | W2 (Release resolution) |
| `CLI-001` (`PARTIAL`) | W7 — remaining PARTIAL closure | W3/W2 (catalog query CLI consumes resolver results) |
| `AUDIT-001` (`PARTIAL`) | W7 — remaining PARTIAL closure | Decision gate (scheduled-job run history) |
| `SCRIPT-001` (`PARTIAL`) | W7 — remaining PARTIAL closure | ADR-0135 (scripting boundary) |
| `PACKAGING-001` (`PARTIAL`) | W8 — migration/cutover/packaging (Phase 11) | ADR-0144/0149/0151; R5 release acceptance |
| `OPS-001` (`IMPLEMENTED — UNPROVEN`) | W8 — migration/cutover/packaging (Phase 11) | R4 external legs (per-surface rate-limit proof) |
| `MIGRATE-001` (`IMPLEMENTED — UNPROVEN`) | W8 — migration/cutover/packaging (Phase 11) | R4/R5 external legs; ADR-0148/0149/0150 |
| `MIGRATE-002` (`IMPLEMENTED — UNPROVEN`) | W8 — migration/cutover/packaging (Phase 11) | R4/R5 external legs; ADR-0149 |
| `PLAYBACK-001` (`IMPLEMENTED — UNPROVEN`) | W9 — ordinary differential/golden evidence | — |
| `TRANSCODE-001` (`IMPLEMENTED — UNPROVEN`) | W9 — ordinary differential/golden evidence | ADR-0147 |
| `PODCAST-001` (`IMPLEMENTED — UNPROVEN`) | W9 — ordinary differential/golden evidence | — |
| `DISCOVERY-001` (`IMPLEMENTED — UNPROVEN`) | W9 — ordinary differential/golden evidence | — |
| `DISCOVERY-002` (`IMPLEMENTED — UNPROVEN`) | W9 — ordinary differential/golden evidence | — |
| `SOCIAL-001` (`IMPLEMENTED — UNPROVEN`) | W9 — ordinary differential/golden evidence | — |

**Ownership invariants (binary):** (a) every one of the 28 rows appears exactly once in the
"Primary owning workstream" column; (b) no row appears in that column twice; (c) a row's secondary
dependencies never make it owned by a second workstream. If two workstreams both *touch* a row,
exactly one of them is primary and the other is listed only under "Secondary dependencies".

## 4. "Done better" reference table

Per feature, the C# approach and the Rust-native replacement this program targets:

| C# feature | C# approach (behavioral evidence only) | Rust-native "better" target |
|---|---|---|
| Cross-Release Artist identity | `LibraryInsertJob.cs` string-keyed `mb:`/`sp:`/`it:`/`name:` dict lookup | Initial catalog import stays **local-only** (already-durable evidence; PRD-EXT-002). `LocalArtistResolver` generalizes the ADR-0153 single-release-artist pattern to cross-Release using *trusted external identity* evidence — **never name alone**. External provider candidates enter via the W3 enrichment worker as **evidence only**, never as a merge trigger during import. Convergence requires a shared trusted external identity plus non-conflicting local evidence; conflicting external identities or a locked/negative decision block it. Bias against false merges per PRD-ARTIST-001/011, PRD-RESOLVE-003, ADR-0117, ADR-0153. |
| Provider enrichment at ingestion | Hardwired search-engine services, inline, blocking in staging | A **separate, bounded, fail-degraded enrichment worker** on the durable work-item engine — *not* part of catalog import. Providers sit behind the provider-neutral `ArtistAuthority` boundary; results are recorded as cache/provenance evidence in `external_provider_cache` and **schedule local re-resolution** without ever blocking catalog entry (PRD-EXT-002). Acceptance concerns are explicit: cancellation, bounded concurrency, per-provider rate limiting, retries/backoff, idempotency, and operator-visible failure behavior. |
| MusicBrainz/external ID retention | Opaque MB-ID columns on catalog rows | Structured **external-ID evidence** per Melodee identity (never a string identity key), under the accepted schema decision ADR-0117 (`artist_external_identities`, `tracks`, `media_assets`, `audio_identity`). Feeds the resolver; no new schema is assumed. |
| Metadata edit forms + tag rewrite | `MediaEditService` 12 transforms | Idempotent transactional catalog mutations with in-transaction audit (already in `catalog_mutations.rs`), field-level validation, SPA save-path fix + full test coverage. The **route/field contract is a single decision** owned by the W0 workstream — it is not simultaneously "an ADR decision" and "no ADR needed". |
| End-user web UI | Blazor server-rendered + SignalR | MeloAmp owns the listener application; the Rust server exposes APIs and `/admin`. Narrow HTML only for required public/security flows under [ADR-0161](adr/ADR-0161-meloamp-owns-listener-workflows-and-the-server-exposes-only-admin-and-narrow-html-surfaces.md). |
| Scheduled jobs | Quartz, per-job `JobHistories` | Keep the durable work-item engine as the runtime (crash-safe, lease-based — stronger than Quartz); add a per-scheduled-job **run-history** projection for operator visibility. **Gated on [ADR-0158](adr/ADR-0158-scheduled-job-run-history-storage.md) (accepted).** |
| Rate limiting / middleware | Named policies | Per-surface token-bucket middleware with documented budgets + differential evidence |

## 5. Stages (proposal, not canonical phases)

### Phase 0 — Architecture ADRs (no code; prerequisite)

Lock the "better" design choices so each build block has an accepted design. Each was drafted with
`python3 scripts/new_adr.py "short title"` and accepted. **All seven ADRs are accepted** (recorded
in the [ADR index](adr/README.md) under "Feature Completion Program decisions"):

1. **[ADR-0154](adr/ADR-0154-external-provider-enrichment-scope-and-runtime.md)** — supersedes
   ADR-0136; provider enrichment is a **v1 obligation** (build, not waive). Defines the provider
   adapter boundary, the enrichment worker, cache/provenance/rate-limit design on
   `external_provider_cache`, and the fail-degraded contract.
2. **[ADR-0161](adr/ADR-0161-meloamp-owns-listener-workflows-and-the-server-exposes-only-admin-and-narrow-html-surfaces.md)**
   — supersedes ADR-0155: MeloAmp owns listener workflows; the Rust server does not build a
   duplicate listener SPA.
3. **[ADR-0156](adr/ADR-0156-mpd-mpv-backend-connectivity-and-lifecycle.md)** — MPD/MPV playback
   backends: a `PlaybackBackend` trait + MPD (TCP) / MPV (IPC socket) adapters, singleton cache,
   fail-degraded availability, and additive native playback-control routes.
4. **[ADR-0157](adr/ADR-0157-metadata-transformation-and-safe-rewrite.md)** — metadata
   transformation contract: a dry-run/apply/revert `MediaEditOperation` trait with an
   `media_edit_audit` trail, atomic tag rewrites, and the admin metadata-edit forms (the Phase 10
   deferral). Resolves the W0/RELEASE-002 route/field contract.
5. **[ADR-0158](adr/ADR-0158-scheduled-job-run-history-storage.md)** — scheduled-job run history:
   a `job_run_history` table + in-process tokio scheduler over the existing `worker_pool_state`
   pause seam, with admin run-history routes (the "better than `JobHistories`" design).
6. **[ADR-0159](adr/ADR-0159-opensubsonic-stored-artwork-sizing.md)** — OS stored-artwork
   `?size=`: resize stored artwork to the requested size (capture-backed), with a size-scoped ETag
   and a bounded in-process cache.
7. **[ADR-0160](adr/ADR-0160-cross-release-artist-release-identity-reuse.md)** — cross-Release
   Artist/Release identity reuse: a promotion policy (external-ID evidence primary, conflict-gated
   name evidence secondary, admin decision) that generalizes ADR-0153 across Releases, audited and
   reversible.

**Exit:** all seven ADRs accepted — **satisfied** (ADR-0154…0160 accepted; ADR-0136 superseded).

### Phase 1 — Quick wins + the P0 bug (start immediately; no ADR needed)

- **RELEASE-002 (P0):** fix the broken SPA save path — register the `/metadata` route server-side
  *or* fix `admin-ui/src/api/admin.ts:783,790,797` + the field-name mismatches; add the zero-test
  server + store integration tests and an admin-ui e2e save test. **Done (2026-08-23):** server
  registers `PUT /api/admin/v1/catalog/{entity}/{id}/metadata`; phantom `country`/`release_type`
  fields removed from the contract; patch-struct API with validation (`InvalidRequest`→400);
  server API test (`metadata_update_routes_auth_success_and_errors`) and 4 store integration
  tests (`catalog_metadata.rs`) added. Remaining: UI field-name alignment + tag-rewriting
  transforms.
- **Close the 9 `IMPLEMENTED — UNPROVEN` rows** — evidence, not code: differential captures vs the
  frozen oracle (`python3 scripts/legacy_oracle.py capture`), e2e/golden tests, and R4
  production-integration legs (AUTH-001 real Google OIDC, OPS-001 per-surface rate-limit,
  PLAYBACK-001, PODCAST-001, DISCOVERY-001/002, SOCIAL-001, MIGRATE-001/002 R4).
- **TRACK-001/002:** MusicBrainz/external-ID retention as structured evidence (schema ADR +
  migration + extraction in `melodee-media`, feeding the resolver); AudioIdentity matching on top.

**Exit:** RELEASE-002 `PROVEN`; most UNPROVEN rows `PROVEN`; TRACK gap closed or ADR'd.

### Phase 2 — Artist identity + provider enrichment (core "better" block; complete locally)

Executed under ADRs 0154, 0160, and 0162. This is where `resolver.rs`, the provider boundary,
and `external_provider_cache` go live.

- **ARTIST-001:** `LocalArtistResolver` now has a production reconciliation caller; strong embedded
  evidence converges identities while weak, conflicting, locked, and negative evidence remains
  separate. The acceptance tests prove the cross-Release merge/split and same-name safety cases.
- **PROVIDER-001 / ARTIST-002 / JOBS-001 / ARTWORK-001:** the durable enrichment worker and provider adapters
  (MusicBrainz, Discogs, Spotify, iTunes, Deezer, Wikidata, Brave, Metal-API + Last.fm lookup legs)
  behind the existing provider-neutral boundary; live `external_provider_cache` writes with
  provenance; honor `searchEngine.*` settings (currently inert, `settings.rs:462-484`);
  provider-discovered artwork; enrichment jobs on the work-item engine. Deterministic local fixture
  evidence covers composition, cache, leases, failure recovery, and artwork precedence. External
  credential and real-provider legs remain explicitly listed in the Phase 2 report.

**Exit:** implementation and local acceptance complete; remaining differential and operator legs
are recorded conservatively in the Phase 2 report.

### Historical proposal — Phase 3 end-user web UI (retired by ADR-0161)

The former proposal for a duplicate server listener SPA is retained as historical planning context.
ADR-0161 supersedes it: MeloAmp is the listener application and the Rust server owns `/admin`,
APIs, ingestion, storage, streaming, and operations. Narrow public/security HTML pages remain
bounded and are not a listener application.

**Disposition:** `WEBUI-001` — `DIFFERENT — APPROVED`, relocated to MeloAmp.

### Historical proposal — Phase 4 — MPD/MPV backends + remaining PARTIAL rows

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
                     ├─► Phase 4 (MPD/MPV + PARTIAL)   [needs ADR 3 + Phase 2 for settings]
                     └─► (all) ──► Phase 5 (migration / packaging)
                                          └─► Phase 6 (final re-audit → v1.0.0)
```

## 7. Effort ranking (largest → smallest)

1. **Phase 2 — provider enrichment + Artist identity.** Most *architecturally* significant, but
   the design seams already exist (highest leverage).
2. **Phase 4 — admin UI completion + edit forms.** Sizable UI work.
3. **Phase 1 — quick wins.** Mostly test/evidence work; fast wins; the RELEASE-002 bug is urgent.
4. **Phase 5 — migration / packaging.** Operator-heavy; on the critical path.

## 8. Open items (resolved in Phase 0)

The product decisions below are resolved by the accepted ADRs
([ADR-0154](adr/ADR-0154-external-provider-enrichment-scope-and-runtime.md),
[ADR-0161](adr/ADR-0161-meloamp-owns-listener-workflows-and-the-server-exposes-only-admin-and-narrow-html-surfaces.md),
[ADR-0156](adr/ADR-0156-mpd-mpv-backend-connectivity-and-lifecycle.md)):

- **Provider enrichment (ADR-0154):** the **full active legacy provider set** (MusicBrainz incl.
  DecentDB, Discogs, Spotify, iTunes, Deezer, Wikidata, Brave, Metal-API + Last.fm metadata-lookup
  legs) is in v1 scope, optional and fail-degraded, behind a single Rust-native provider boundary.
- **End-user web UI (ADR-0161):** MeloAmp owns the listener application. The Rust server does not
  build a second listener SPA; `WEBUI-001` is an approved relocation, with narrow HTML only when a
  public/security flow requires it.
- **MPD/MPV (ADR-0156):** **both MPD and MPV** ship in v1 as configuration-selected backends,
  fail-degraded when the backend process is unavailable.

The two remaining "gated on a decision" notes in the §4 reference table (scheduled-job run history,
OS `?size=`) are also resolved by [ADR-0158](adr/ADR-0158-scheduled-job-run-history-storage.md) and
[ADR-0159](adr/ADR-0159-opensubsonic-stored-artwork-sizing.md); cross-Release identity is resolved
by [ADR-0160](adr/ADR-0160-cross-release-artist-release-identity-reuse.md). No product decision
blocks a Phase 0 ADR any longer.

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
- [SPEC.md](../SPEC.md) — technical invariants and interface contracts.
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
