# Phase 9 Plan — Remaining Parity Domains

Status: **Planning document (pre-authorization), refreshed 2026-08-13 after Phase 8 acceptance.**
Phase 9 implementation remains forbidden by the `AGENTS.md` phase boundary until this plan (or a
successor work package) is explicitly assigned. This document is the review + plan for full Phase 9
completion.

Sources: `IMPLEMENTATION.md` §12, `PRD.md` §31–37/44–46 + Phase 9 exit (§56), `ADR-0123`,
`ADR-0124`, `ADR-0128`, `ADR-0130`, `ADR-0131`, `ADR-0132`,
`design/requirements/FEATURE-PARITY-MATRIX.md`, `design/requirements/REQUIREMENTS-INDEX.md`,
`design/evidence/phase-7/PHASE-7-REPORT.md`, `design/evidence/phase-8/PHASE-8-REPORT.md`,
`design/agents/PHASE-8-{OPENSUBSONIC,JELLYFIN}-ROUTE-MATRIX.json`, and a full workspace survey
(2026-08-13).

---

## 1. Current-state review findings

### 1.1 What Phases 7 and 8 already closed (do not rebuild)

Per ADR-0123, Phase 7 owns every route compiled into the frozen native-v1 inventory (202/202, zero
deferred), and Phase 8 owns both adapter surfaces (OpenSubsonic 81/81, Jellyfin 60/60, differential
replay at zero unexpected differences, Feishin v1.15.1 real-client smoke 12/12). Phase 9 must
**consume these as established adapters**, not reimplement them:

- Native auth/users/roles/library-ACL enforcement, bcrypt migration, refresh families
  (`native_users`, `native_user_roles`, `native_library_access_groups`, …, migration 0010).
- Playlists + smart playlists (MQL), user state (star/rate/hate/pin), scrobble/now-playing
  (0011, 0014, 0016); Jellyfin and OpenSubsonic playlist/share/podcast/radio adapter legs.
- Requests/comments/activity, shares (public bearer, ADR-0129), EQ presets, playback settings (0012).
- Podcasts: subscriptions, refresh worker, downloads, resume/history, OPML (0013,
  `podcast_worker.rs`); OpenSubsonic bookmarks/play-queue and podcast endpoints.
- Charts, audio features (BPM read path), recommendations, listening analytics, themes,
  playback backends/endpoints (0015, 0017, 0019).
- OpenSubsonic internet-radio CRUD (Phase 8); intentional 410/501 negative matrix
  (`opensubsonic/negative.rs`).

### 1.2 Phase 8 status (former hard gate — SATISFIED)

Phase 8 is complete and accepted (2026-08-13): both adapter route matrices implemented and
differentially evidenced (81/81 OpenSubsonic, 60/60 Jellyfin), full-golden differential replay at
zero unexpected differences, real-client smoke passed
(`design/evidence/phase-8/REAL-CLIENT-ACCEPTANCE.md`). Report:
`design/evidence/phase-8/PHASE-8-REPORT.md`. The `AGENTS.md` boundary reads "Phase 8 complete —
awaiting next phase assignment." **Phase 9 has no remaining upstream phase gate.**

Residual Phase 8 loose ends relevant to Phase 9 tracking (not Phase 9 scope unless assigned):

- Requirements-index promotion lag: PRD-API-OS-001..006 still `captured`, PRD-API-JF-001..004
  `implemented` (not `tested`), PRD-API-OS-007/PRD-API-JF-006/PRD-PLAYLIST-001/PRD-POD-003
  `in-progress` with notes saying "remains Phase 8 work" — all now satisfiable (W0-1).
- `Album.OriginalReleaseDate` column absence is an explicitly *deferred schema ADR* (Phase 8
  report, 6 known differences) — keep visible in Phase 9 tracking so it is not lost.
- Durable artwork wiring (`images.rs::resolve_image`, captured 404 classes) is tracked with the
  artwork pipeline, not Phase 9; confirm at closeout that it has an owning work item.

### 1.3 Genuine Phase 9 greenfield (zero or negative-only implementation today)

| Domain | State today |
|---|---|
| Party Mode / queue | Intentionally absent — frozen 404 (`native-excluded` goldens, 35 pinned cases, ADR-0123). No tables. |
| Jukebox | Intentional 410 (`opensubsonic/negative.rs`); legacy C# ADR-0001 never implemented it. |
| Scripting hooks | Zero code, zero captures. PRD-SCRIPT-001..005 all `planned`. |
| Onboarding | Zero. PRD-SETUP-001/002 `planned`. |
| Config import/export | Zero (only theme-zip/OPML/chart-CSV narrow slices). PRD-CONFIG-001..003, PRD-AUTH-007. **The Phase 0 "configuration-inventory.md" deliverable (190 `SettingRegistry.cs` keys) was never produced** — WP-08 must generate it from the legacy oracle. |
| Notifications/email templates | Zero code and **no requirement ID** (PRD prose §7 inventory row only). |
| Internet radio (native surface) | OpenSubsonic CRUD done (Phase 8, golden `os-radio-*`); native surface + requirement IDs missing. |
| Real Last.fm integration | Routes return captured "unconfigured" shapes only (`native-scrobble/` goldens); no Last.fm client. PRD-EXT-001..004 `planned`. Legacy oracle at `../melodee` (frozen commit) is available for re-capture of configured-state behavior per `design/runbooks/PHASE-0-LEGACY-CAPTURE.md`. |
| User/group administration | OS user management intentionally 501; no group CRUD anywhere; avatar **read** exists (OS `getAvatar`), avatar **upload/change** missing; native `/admin/users` is list-only. |
| Device profiles | Zero. PRD-DEVICE-001..003 `planned`. |
| Localization/i18n, web parity | Zero code. THEME/I18N requirement IDs are Phase-10-tagged; Phase 9 owns only PRD-WEB-001/002 inventory/classification. |
| CLI breadth | Foundation + Phase 2–6 commands exist (`apps/melodee-cli`, 9 command groups); user bootstrap/recovery, group management, config validate/export, notification test-send, stuck-download recovery missing. |

### 1.4 Tracking/documentation gaps that block the Phase 9 exit gate

Phase 9 exit = *"feature-parity matrix has no unapproved gaps"* (PRD §56) and *"each domain
requires its own parity checklist and tests"* (IMPLEMENTATION §12). Blocking gaps:

1. **FEATURE-PARITY-MATRIX.md is stale.** Rows for playlists, scrobble, shares, requests, podcasts,
   users/roles, ACL, charts, recommendations, analytics, EQ, party-negative, playback backends still
   say `Planned`/no evidence although Phases 7/8 implemented and were accepted for them. The exit
   gate reads this matrix — it must be reconciled first or Phase 9 can never "pass".
2. **Missing requirement IDs** for internet radio, notifications/templates, and the CLI capability
   set (PRD §31 and §46 are prose-only), plus §7 inventory rows without IDs (avatars, audio
   features/BPM, EQ presets). Gate G counts unclassified capabilities as gaps.
3. **7 in-progress Phase-9-tagged requirements** need closure or re-tagging: PRD-AUTH-007,
   PRD-PLAYLIST-001, PRD-REQUEST-002, PRD-POD-003, PRD-PARTY-001, PRD-WORK-001, PRD-WORK-005.
4. **12 unpromoted Phase-8 IDs** (§1.2) block overall parity accounting until the index is updated.
5. **Stale repo artifacts** (cleanup, not scope): `crates/melodee-api-v1/` is an empty orphan
   scaffold (not a workspace member, untracked); top-level `migrations/README.md` still claims
   migrations "will live here after Phase 0" (real ones are in
   `crates/melodee-persistence/migrations/`).

### 1.5 Decisions Phase 9 cannot make unilaterally

- **Party Mode / Jukebox:** reactivation requires a superseding ADR + client compatibility plan
  (ADR-0123); current absence is *accepted* behavior. Gate G explicitly allows "an explicit
  approved ADR/release waiver" — the cheapest compliant path may be a waiver ADR, not an
  implementation. **Product decision required.**
- **OpenSubsonic user management (currently 501):** changing intentional 501 behavior requires
  compatibility test + product decision + accepted ADR + migration/client plan (AGENTS.md).
- **JS runtime selection:** requires ADR + security review (PRD-SCRIPT-003); new production
  dependency must pass the FOSS gate.
- **ADR-0109 (Carbon):** still `Proposed`; its validation shells are labeled Phase 9 scope while
  the admin UI itself is Phase 10 — scope ambiguity to resolve by ADR amendment.
- **ADR-0132 (Jellyfin ApiKey alias):** still `Proposed` although the code shipped under it;
  accept or amend as part of the Wave 0 decision bundle.
- **Device profiles (PRD-DEVICE-002/003):** must not claim transcoding control that doesn't exist;
  wiring profiles to stream selection is an ADR-gated post-parity improvement, not Phase 9 scope.

---

## 2. Execution strategy

Phase 9 is executed in four waves. Wave 0 is mandatory gating work; Waves 1–3 are domain work
packages ordered by dependency and risk. Every WP follows the agent execution protocol
(IMPLEMENTATION §16): requirement IDs + ADRs first, acceptance tests before implementation,
smallest safe slice, `cargo fmt --all` + `make check` green, docs updated in the same
change.

### Wave 0 — Unblock and reconcile (no new features)

| # | Item | Deliverable |
|---|---|---|
| W0-1 | **Promote Phase 7/8 requirement statuses**: flip PRD-API-OS-001..007, PRD-API-JF-001..006, and the satisfied legs of PRD-PLAYLIST-001/PRD-POD-003/PRD-REQUEST-002 in `status-overrides.json`; re-run `extract_requirements.py --check` | Index reflects accepted Phase 7/8 evidence |
| W0-2 | **Reconcile FEATURE-PARITY-MATRIX.md**: backfill Phase 7/8 evidence columns and statuses for all stale rows (§1.4.1); clean up stale artifacts (§1.4.5: orphan `crates/melodee-api-v1/`, `migrations/README.md`) | Matrix reflects reality; no false `Planned` rows |
| W0-3 | **Requirement-ID gap fixes**: add PRD IDs for internet radio (PRD-RADIO-001..), notifications/templates (PRD-NOTIF-001..), CLI capability set (PRD-CLI-001..), avatars and audio-features/EQ rows as needed; re-run `extract_requirements.py`, commit regenerated index | PRD + index consistent; every Phase 9 capability tracked |
| W0-4 | **Decision bundle** (see §3): party/jukebox waiver-or-build ADR, JS runtime ADR + security review, OS user-management decision, ADR-0109 disposition, ADR-0132 disposition, Last.fm real-vs-stub decision | Accepted ADRs before dependent WPs start |
| W0-5 | **Phase 9 parity checklists**: one checklist per IMPLEMENTATION §12 domain under `design/evidence/phase-9/checklists/` seeding the WP acceptance criteria | 12 checklists, each mapping capability → requirement IDs → tests |

### Wave 1 — Low-risk domain completion (builds on existing schema/code; parallelizable)

| WP | Domain | Requirements | Scope (smallest coherent slices) |
|---|---|---|---|
| WP-01 | Playlists/shares/requests breadth | PRD-PLAYLIST-001..003, PRD-REQUEST-001/002, PRD-SHARE-001..003 | Cross-API ID stability verification (native ↔ OS ↔ JF); close the in-progress IDs; address the public share-assembly latency risk noted in the Phase 7 report (bounded 10,000-item assembly with per-Track sequential DTO loads) with a set-based load + perf evidence |
| WP-02 | Podcasts/resume/history remainder | PRD-POD-001..003 | Close PRD-POD-003 (OS leg evidence); stuck-download recovery CLI; retention/OPML parity checklist; real-feed exercise (Phase 7 known limitation: real podcast feeds unexercised) |
| WP-03 | Users/groups/ACL administration | PRD-AUTH-003/004/007, PRD-USER-001 | Group CRUD + membership admin API (Admin surface, not the frozen OS contract), avatar upload/change with `avatarLastChanged` semantics, admin user-management breadth beyond list-only |
| WP-04 | Charts/recommendations/analytics/audio features/EQ | PRD-DISCOVERY-001..003, PRD-WORK-003 | Scheduled chart refresh + aggregate maintenance in the scheduler; document intentional limits of BPM/features; EQ parity checklist closeout |
| WP-05 | CLI breadth | PRD-CLI-001..(new, W0-3), PRD-WORK-004/005 | User bootstrap/recovery, group management, config validate, notification test-send (with WP-09), maintenance triggers; close PRD-WORK-005 pool pause/resume — all lease-coordinated per PRD-WORK-004 |
| WP-06 | Internet radio | PRD-RADIO-001..(new, W0-3) | Native-API radio CRUD over a schema-ADR-backed shared table (today's `opensubsonic_radio_stations` is adapter-owned); admin-gated mutations per captured OS behavior |

### Wave 2 — New subsystems (require Wave 0 ADRs)

| WP | Domain | Requirements | Scope |
|---|---|---|---|
| WP-07 | External metadata providers + Last.fm | PRD-EXT-001..004, PRD-AUTH-002 (Last.fm linking), PRD-AUTH-007 | Provider inventory evidence (legacy ADR-0003/0004 + oracle re-capture of configured-state Last.fm flows); FOSS Last.fm client (scrobble + auth session lifecycle per legacy ADR-0004), cache/rate-limit/fail-degraded layer, provenance storage; no provider required for catalog entry (PRD-EXT-002); captured "unconfigured" shapes remain the disabled-state contract |
| WP-08 | Onboarding + config import/export | PRD-SETUP-001/002, PRD-CONFIG-001..003, PRD-AUTH-007 | **First: produce the missing 190-key configuration inventory** from legacy `SettingRegistry.cs` (Phase 0 gap). Then: first-run preflight (paths, PG, media tools, search readiness, admin account); completion-state bypass after migration (PRD-SETUP-002); settings service (env/file + DB-backed); export with preview/validation and secret redaction/warnings on import |
| WP-09 | Notifications/templates | PRD-NOTIF-001..(new, W0-3) | Template storage/rendering, FOSS email transport (no brokers/Redis — forbidden), event wiring (shares, requests, password reset), CLI test-send (with WP-05) |
| WP-10 | Device profiles | PRD-DEVICE-001..003 | Profile CRUD + default/validation parity with legacy preview semantics; explicit *non*-claim of transcoding control (PRD-DEVICE-002); ADR stub for future stream negotiation (PRD-DEVICE-003) |
| WP-11 | Scripting hooks | PRD-SCRIPT-001..005 | Script event inventory (evidence doc from legacy source, PRD-SCRIPT-002, first); runtime per accepted ADR (PRD-SCRIPT-003); sandboxed executor — CPU/memory/time limits, no fs/process/net by default (PRD-SCRIPT-004, prototype before wiring the full inventory); per-hook migration guide (PRD-SCRIPT-005) |

### Wave 3 — Contract-sensitive items (require product decision first)

| WP | Domain | Requirements | Scope |
|---|---|---|---|
| WP-12 | Party Mode / jukebox | PRD-PARTY-001..004, PRD-USER-004 | Either (a) **waiver ADR**: preserve frozen 404/410 absence with documented rationale + client plan, or (b) **build**: superseding ADR, schema ADR, sessions/queues/moderation, conditional `jukeboxControl`, reactivation tests against frozen `native-excluded` goldens |
| WP-13 | OS user management (only if approved) | PRD-AUTH-003 | Convert intentional 501s to implemented CRUD **only** via the 4-step contract-change process; otherwise document permanent intentional absence in the matrix |
| WP-14 | Web parity residue + ADR-0109 | PRD-WEB-001/002 | Blazor page inventory classification (keep/replace/drop; ADR before removing any route); validate or amend ADR-0109 scope; confirm THEME/I18N/RTL rows are Phase 10 and out of Phase 9's exit gate |

### Wave 4 — Closeout

| # | Item |
|---|---|
| W4-1 | Per-domain parity checklists all green; matrix has **no unapproved gaps** (Gate G) |
| W4-2 | Requirements index: all Phase-9-tagged IDs `tested`/`implemented`, or covered by accepted waiver ADRs (incl. closing the 7 in-progress IDs from §1.4.3) |
| W4-3 | `design/evidence/phase-9/PHASE-9-REPORT.md` per the release-gate evidence package format (commands, versions, test reports, compatibility diffs, checklist results, honest limitations) following the Phase 7/8 report structure; independent review decision recorded |
| W4-4 | `/docs` user-facing updates for every user-visible WP (same change as code, per AGENTS.md); internal design docs + traceability updated; `AGENTS.md` phase boundary advanced; confirm the deferred `Album.OriginalReleaseDate` schema ADR and durable-artwork wiring have owning work items |

---

## 3. Decision bundle (W0-4) — recommended positions

| Decision | Recommendation | Authority |
|---|---|---|
| Party Mode / jukebox | **Waiver ADR** preserving frozen absence. Rationale: baseline C# build doesn't compile the controllers (D6); legacy ADR-0001 never implemented jukebox; no client can depend on them; building is a new feature, not parity. Keeps Gate G green at lowest risk. If product wants Party Mode, it becomes a post-parity feature with its own ADR chain. | References ADR-0123 reactivation clause; closes PRD-PARTY-001/002, PRD-USER-004 |
| JS runtime | Evaluate `boa` vs `rquickjs` (both FOSS-permissive); security review of the sandboxing story; ADR per PRD-SCRIPT-003 | New ADR + `cargo deny` + FOSS gate |
| OS user management 501s | **Keep intentional 501**; user administration lives on the Admin API surface (WP-03 + Phase 10 UI). Avoids touching a frozen captured contract. | Compatibility note + matrix row |
| ADR-0109 Carbon | Accept as-is (Phase 0 already revalidated FOSS status); move its shell-validation acceptance into Phase 10 entry criteria, resolving the Phase 9/10 scope ambiguity | Amend ADR-0109 |
| ADR-0132 ApiKey alias | Accept (code already shipped and differentially evidenced under it in Phase 8) | Status flip, no code change |
| Last.fm | Implement real client behind config flag; captured "unconfigured" shapes remain the disabled-state contract; re-capture configured-state behavior from the legacy oracle before implementing | WP-07 evidence |
| Notifications | New capability (legacy prose-only); minimal email-template subsystem, no new infrastructure services (brokers/Redis forbidden) | New PRD IDs (W0-3) |

## 4. Cross-cutting requirements for every WP

- **Schema changes:** each needs its own migration + rollback/forward plan + data-integrity tests
  + accepted schema ADR (AGENTS.md). Expected new tables: notifications/templates, device
  profiles, onboarding state, script registry/sandbox config, provider cache/provenance, shared
  radio stations (WP-06), group/avatar administration extensions, (party only if WP-12 builds).
- **Contract changes:** any change to intentional 404/410/501 behavior needs the 4-step process
  (compatibility test → product decision → accepted ADR → migration/client plan).
- **Tests:** unit (domain invariants) → persistence integration (TestDb harness) → server API
  suites (`apps/melodee-server/tests/`) → golden/differential fixtures where a captured contract
  exists. New capture work (Last.fm configured state, radio native surface) follows
  `design/runbooks/PHASE-0-LEGACY-CAPTURE.md` against the frozen oracle at `../melodee`
  (`scripts/legacy_oracle.py recreate` fails closed on baseline drift).
- **Docs:** `/docs` updated in the same PR for user-visible behavior; `FEATURE-PARITY-MATRIX.md`,
  `REQUIREMENTS-INDEX.md` (via `extract_requirements.py`), and checklists updated per WP.
- **Gate discipline:** `cargo fmt --all` then `make check` before any WP is called done.

## 5. Sequencing summary and critical path

```
W0-1..W0-3 (reconcile, parallel) ──► W0-4 (decision bundle) ──► Wave 1 (WP-01..06, parallelizable)
                                      │                     ──► Wave 2 (WP-07..11, needs ADRs)
                                      │                     ──► Wave 3 (WP-12..14, needs product decisions)
                                      └─────────────────────► Wave 4 (closeout + report)
```

Critical path: **decision bundle (W0-4) → WP-12 waiver-or-build → closeout (W4)**. With the
recommended waiver, Phase 9's heaviest remaining engineering is WP-11 (scripting sandbox) and
WP-07/WP-08 (provider + config subsystems). Everything in Wave 1 is parallelizable across agents
once W0-1..W0-3 land, since those WPs touch disjoint modules (`native/`, `opensubsonic/`,
`jellyfin/`, `melodee-cli`, `persistence`).

Suggested PR slicing (per IMPLEMENTATION §15): one WP per PR at most; Wave 0 as three PRs
(W0-1+W0-2 reconciliation, W0-3 requirement IDs, W0-4 ADRs + W0-5 checklists).

## 6. Risks

1. **Party Mode ambiguity** is the largest scope swing: waiver ADR ≈ one WP; full build ≈ the
   largest WP in the phase (sessions, moderation, backends, jukebox engine, schema). Resolve in
   W0-4 before any other Wave 3 work.
2. **JS runtime security review** may force a design iteration; sandboxing (PRD-SCRIPT-004) is
   non-negotiable and should be prototyped before the full hook inventory is wired.
3. **Matrix vs. reality drift** (§1.4.1) means Gate G currently *fails on paper* despite real,
   accepted implementations; W0-1/W0-2 must land before any WP claims completion.
4. **Missing configuration inventory** (190 legacy setting keys, Phase 0 gap) means WP-08 starts
   with an evidence-generation step, not code — underestimating this is the main scheduling risk
   in Wave 2.
5. **ADR-0109 scope ambiguity** (validation labeled Phase 9, implementation Phase 10) can block
   closeout if not amended early.
6. **Last.fm re-capture depends on the legacy oracle** and a real/configured Last.fm test account;
   if configured-state capture is impossible, fall back to implementing against the legacy source
   as behavioral evidence (legacy ADR-0004) and record the capture gap as a limitation.

---

## Review report (per AGENTS.md required workflow)

- **Files changed:** `design/agents/PHASE-9-PLAN.md` (this document; refreshed for Phase 8
  acceptance — §1.2 rewritten, W0-1 re-scoped from "close Phase 8" to requirement-status promotion,
  sequencing/risks updated, §1.4.5 stale-artifact cleanup and the missing configuration-inventory
  gap added).
- **Requirements satisfied:** none implemented — this is the planning artifact only.
- **ADRs applied/proposed:** proposes future ADRs for party/jukebox waiver-or-build, JS runtime,
  device-profile stream negotiation, provider inventory, shared radio schema; proposes amending
  ADR-0109 and accepting ADR-0132.
- **Tests run:** none (planning only). Repository state verified by read-only survey (2026-08-13).
- **Compatibility impact:** none. Plan explicitly routes all intentional 404/410/501 changes
  through the 4-step contract-change process.
- **Data/migration impact:** none now; §4 enumerates the schema-ADR obligations Phase 9 will incur.
- **Documentation impact:** Updated — this plan under `design/agents/` (internal engineering
  docs). No `/docs` (public) changes: no user-visible behavior changed.
  `FEATURE-PARITY-MATRIX.md` staleness is documented here; its reconciliation is scheduled as
  W0-2 rather than done in this planning change.
- **Remaining risks:** §6.
