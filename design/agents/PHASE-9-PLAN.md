# Phase 9 Plan — Remaining Parity Domains

Status: **Planning document (pre-authorization)**. Phase 9 implementation is currently forbidden by
the `AGENTS.md` phase boundary until Phase 8 closes and this plan (or a successor work package) is
explicitly assigned. This document is the review + plan requested for Phase 9 completion.

Sources: `IMPLEMENTATION.md` §12, `PRD.md` §31–37/44–46 + Phase 9 exit, `ADR-0123`, `ADR-0124`,
`ADR-0130`, `ADR-0131`, `design/requirements/FEATURE-PARITY-MATRIX.md`,
`design/requirements/REQUIREMENTS-INDEX.md`, `design/evidence/phase-7/PHASE-7-REPORT.md`,
`design/agents/PHASE-8-{OPENSUBSONIC,JELLYFIN}-ROUTE-MATRIX.json`, and a full workspace survey
(2026-08-12).

---

## 1. Current-state review findings

### 1.1 What Phase 7 already closed (do not rebuild)

Per ADR-0123, Phase 7 owns every route compiled into the frozen native-v1 inventory (202/202, zero
deferred). Phase 9 must **consume these as established adapters**, not reimplement them:

- Native auth/users/roles/library-ACL enforcement, bcrypt migration, refresh families
  (`native_users`, `native_user_roles`, `native_library_access_groups`, …, migration 0010).
- Playlists + smart playlists (MQL), user state (star/rate/hate/pin), play queue absence (frozen),
  scrobble/now-playing (0011, 0014, 0016).
- Requests/comments/activity, shares (public bearer), EQ presets, playback settings (0012).
- Podcasts: subscriptions, refresh worker, downloads, resume/history, OPML (0013,
  `podcast_worker.rs`).
- Charts, audio features (BPM read path), recommendations, listening analytics, themes,
  playback backends/endpoints (0015, 0017, 0019).

### 1.2 Phase 8 status (hard gate for Phase 9)

- **OpenSubsonic:** 81/81 endpoints implemented and differentially tested
  (`PHASE-8-OPENSUBSONIC-ROUTE-MATRIX.json`), incl. radio CRUD and podcast/share adapters.
- **Jellyfin:** 0/60 routes implemented (`PHASE-8-JELLYFIN-ROUTE-MATRIX.json`); only
  auth/discovery scaffold exists (`apps/melodee-server/src/jellyfin/`). ADR-0131 accepted.
- **No `design/evidence/phase-8/`, no Phase 8 report, no Phase 8 work-package doc.**
- `AGENTS.md` phase boundary forbids Phase 9 expansion while Phase 8 is active.

### 1.3 Genuine Phase 9 greenfield (zero or negative-only implementation today)

| Domain | State today |
|---|---|
| Party Mode / queue | Intentionally absent — frozen 404 (`native-excluded` goldens, ADR-0123). No tables. |
| Jukebox | Intentional 410 (`opensubsonic/negative.rs`); native discovery answers "not enabled". |
| Scripting hooks | Zero code. PRD-SCRIPT-001..005 all `planned`. |
| Onboarding | Zero. PRD-SETUP-001/002 `planned`. |
| Config import/export | Zero (only theme-zip/OPML/chart-CSV narrow slices). PRD-CONFIG-001..003, PRD-AUTH-007. |
| Notifications/email templates | Zero code and **no requirement ID** (prose only). |
| Internet radio (native surface) | OpenSubsonic CRUD done (Phase 8); native + requirement ID missing. |
| Real Last.fm integration | Routes return captured "unconfigured" shapes only; no Last.fm client. PRD-EXT-001..004 `planned`. |
| User/group administration APIs | OS user management intentionally 501; no group CRUD anywhere; avatar upload missing. |
| Device profiles | Zero. PRD-DEVICE-001..003 `planned`. |
| Localization/i18n, web parity | Zero. Mostly Phase 10-tagged; Phase 9 owns only "web parity not covered by native clients". |
| CLI breadth | Foundation + Phase 2–4 commands exist; user bootstrap/recovery, config, migration commands missing. |

### 1.4 Tracking/documentation gaps that block the Phase 9 exit gate

Phase 9 exit = *"Feature-parity matrix has no unapproved gaps"* (PRD §56) and *"each domain requires
its own parity checklist and tests"* (IMPLEMENTATION §12). Blocking gaps:

1. **FEATURE-PARITY-MATRIX.md is stale.** Rows 20–41 still say `Planned`/no evidence for
   capabilities Phase 7 implemented and had accepted (playlists, scrobble, shares, requests,
   podcasts, users/roles, ACL, charts, recommendations, analytics, EQ, party-negative, playback
   backends). The exit gate reads this matrix — it must be reconciled first or Phase 9 can never
   "pass".
2. **Missing requirement IDs** for internet radio, notifications/templates, and the CLI capability
   set (PRD §31 and §46 are prose-only). Gate G counts unclassified capabilities as gaps.
3. **7 in-progress Phase-9-tagged requirements** need closure or re-tagging: PRD-AUTH-007,
   PRD-PLAYLIST-001, PRD-REQUEST-002, PRD-POD-003, PRD-PARTY-001, PRD-WORK-001, PRD-WORK-005.
4. **12 `captured` Phase-8 IDs** (PRD-API-OS-001..006, PRD-API-JF-001..005) block overall parity
   accounting until Phase 8 finishes.

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
- **Device profiles (PRD-DEVICE-002/003):** must not claim transcoding control that doesn't exist;
  wiring profiles to stream selection is an ADR-gated post-parity improvement, not Phase 9 scope.

---

## 2. Execution strategy

Phase 9 is executed in four waves. Wave 0 is mandatory gating work; Waves 1–3 are domain work
packages ordered by dependency and risk. Every WP follows the agent execution protocol
(IMPLEMENTATION §16): requirement IDs + ADRs first, acceptance tests before implementation,
smallest safe slice, `./scripts/check.sh` green, docs updated in the same change.

### Wave 0 — Unblock and reconcile (no new features)

| # | Item | Deliverable |
|---|---|---|
| W0-1 | **Close Phase 8**: implement Jellyfin 60/60 routes, real-client smoke (OS + Jellyfin), create `design/evidence/phase-8/` + PHASE-8-REPORT, flip PRD-API-JF-* statuses | Phase 8 exit criteria met; `AGENTS.md` boundary updated to "Phase 9 active" |
| W0-2 | **Reconcile FEATURE-PARITY-MATRIX.md**: backfill Phase 7/8 evidence columns and statuses for all stale rows (§1.4.1) | Matrix reflects reality; no false `Planned` rows |
| W0-3 | **Requirement-ID gap fixes**: add PRD IDs for internet radio (PRD-RADIO-001..), notifications/templates (PRD-NOTIF-001..), CLI capability set (PRD-CLI-001..); re-run `extract_requirements.py`, commit regenerated index | PRD + index consistent; every Phase 9 capability tracked |
| W0-4 | **Decision bundle** (see §3): party/jukebox waiver-or-build ADR, JS runtime ADR + security review, OS user-management decision, ADR-0109 disposition, Last.fm real-vs-stub decision | Accepted ADRs before dependent WPs start |
| W0-5 | **Phase 9 parity checklists**: one checklist per IMPLEMENTATION §12 domain under `design/evidence/phase-9/checklists/` seeding the WP acceptance criteria | 12 checklists, each mapping capability → requirement IDs → tests |

### Wave 1 — Low-risk domain completion (builds on existing schema/code)

| WP | Domain | Requirements | Scope (smallest coherent slices) |
|---|---|---|---|
| WP-01 | Playlists/shares/requests breadth | PRD-PLAYLIST-001..003, PRD-REQUEST-001/002, PRD-SHARE-001..003 | Close adapter parity after Phase 8 (Jellyfin playlist/share surfaces); verify cross-API ID stability; close the 3 in-progress IDs |
| WP-02 | Podcasts/resume/history remainder | PRD-POD-001..003 | OS/JF podcast adapter closure, stuck-download recovery CLI, retention/OPML parity checklist |
| WP-03 | Users/groups/ACL administration | PRD-AUTH-003/004/007, PRD-USER-001 | Group CRUD + membership admin API (Admin surface, not frozen OS contract), avatar upload/change with `avatarLastChanged` semantics, social-linking breadth per ADR-0124 note |
| WP-04 | Charts/recommendations/analytics/audio features/EQ | PRD-DISCOVERY-001..003, PRD-WORK-003 | Scheduled chart refresh + aggregate maintenance in scheduler; document intentional limits of BPM/features; EQ parity checklist closeout |
| WP-05 | CLI breadth | PRD-CLI-001..(new), PRD-WORK-004/005 | User bootstrap/recovery, group management, config validate, notification test-send, maintenance triggers — all lease-coordinated per PRD-WORK-004 |
| WP-06 | Internet radio | PRD-RADIO-001..(new) | Native-API radio CRUD reusing `opensubsonic_radio_stations`-backed store or a schema-ADR-backed shared table; admin-gated mutations per captured behavior |

### Wave 2 — New subsystems (require Wave 0 ADRs)

| WP | Domain | Requirements | Scope |
|---|---|---|---|
| WP-07 | External metadata providers + Last.fm | PRD-EXT-001..004, PRD-AUTH-002 (Last.fm bullet), PRD-AUTH-007 | Provider inventory ADR/evidence; FOSS Last.fm client (scrobble + auth session), cache/rate-limit/fail-degraded layer, provenance storage; no provider required for catalog entry (PRD-EXT-002) |
| WP-08 | Onboarding + config import/export | PRD-SETUP-001/002, PRD-CONFIG-001..003, PRD-AUTH-007 | First-run preflight (paths, PG, media tools, search readiness, admin account); completion-state bypass after migration; settings service (env/file + DB-backed); export with preview/validation and secret redaction/import handling |
| WP-09 | Notifications/templates | PRD-NOTIF-001..(new) | Template storage/rendering, FOSS email transport, event wiring (shares, requests, password reset), CLI test-send |
| WP-10 | Device profiles | PRD-DEVICE-001..003 | Profile CRUD + default/validation parity with legacy preview semantics; explicit *non*-claim of transcoding control (PRD-DEVICE-002); ADR stub for future stream negotiation (PRD-DEVICE-003) |
| WP-11 | Scripting hooks | PRD-SCRIPT-001..005 | Script event inventory (evidence doc from legacy source); runtime per accepted ADR; sandboxed executor (CPU/memory/time limits, no fs/process/net by default); per-hook migration guide |

### Wave 3 — Contract-sensitive items (require product decision first)

| WP | Domain | Requirements | Scope |
|---|---|---|---|
| WP-12 | Party Mode / jukebox | PRD-PARTY-001..004, PRD-USER-004 | Either (a) **waiver ADR**: preserve frozen 404/410 absence with documented rationale + client plan, or (b) **build**: superseding ADR, schema ADR, sessions/queues/moderation, conditional `jukeboxControl`, reactivation tests against frozen `native-excluded` goldens |
| WP-13 | OS user management (if approved) | PRD-AUTH-003 | Convert intentional 501s to implemented CRUD **only** with the 4-step contract-change process; otherwise document permanent intentional absence |
| WP-14 | Web parity residue + ADR-0109 | PRD-WEB-001/002 | Blazor page inventory classification (keep/replace/drop); validate or amend ADR-0109 scope; confirm themes/i18n/RTL rows are Phase 10 and out of Phase 9's exit gate |

### Wave 4 — Closeout

| # | Item |
|---|---|
| W4-1 | Per-domain parity checklists all green; matrix has **no unapproved gaps** (Gate G) |
| W4-2 | Requirements index: all Phase-9-tagged IDs `tested`/`implemented`, or covered by accepted waiver ADRs |
| W4-3 | `design/evidence/phase-9/PHASE-9-REPORT.md` per the release-gate evidence package format (commands, versions, test reports, compatibility diffs, checklist results) |
| W4-4 | `/docs` user-facing updates for every user-visible WP (same change as code, per AGENTS.md); internal design docs + traceability updated |

---

## 3. Decision bundle (Wave 0-4) — recommended positions

| Decision | Recommendation | Authority |
|---|---|---|
| Party Mode / jukebox | **Waiver ADR** preserving frozen absence. Rationale: baseline C# build doesn't compile the controllers (D6); no client can depend on them; building is a new feature, not parity. Keeps Gate G green at lowest risk. If product wants Party Mode, it becomes a post-parity feature with its own ADR chain. | Supersedes nothing; references ADR-0123 reactivation clause |
| JS runtime | Evaluate `boa` vs `rquickjs` (both FOSS-permissive); security review of sandboxing story; ADR per PRD-SCRIPT-003 | New ADR + `cargo deny` + FOSS gate |
| OS user management 501s | **Keep intentional 501**; user administration lives on the Admin API surface (Phase 10 UI + WP-03). Avoids touching a frozen captured contract. | Compatibility note + matrix row |
| ADR-0109 Carbon | Accept as-is (Phase 0 already revalidated FOSS status); move its shell-validation acceptance into Phase 10 entry criteria, resolving the Phase 9/10 scope ambiguity | Amend ADR-0109 |
| Last.fm | Implement real client behind config flag; captured "unconfigured" shapes remain the disabled-state contract | WP-07 evidence |
| Notifications | New capability (legacy prose-only); minimal email-template subsystem, no new infrastructure services (forbidden: brokers/Redis) | New PRD IDs (W0-3) |

## 4. Cross-cutting requirements for every WP

- **Schema changes:** each needs its own migration + rollback/forward plan + data-integrity tests
  + accepted schema ADR (AGENTS.md). Expected new tables: notifications/templates, device
  profiles, onboarding state, script registry/sandbox config, provider cache/provenance,
  (party only if WP-12 builds).
- **Contract changes:** any change to intentional 404/410/501 behavior needs the 4-step process
  (compatibility test → product decision → accepted ADR → migration/client plan).
- **Tests:** unit (domain invariants) → persistence integration (TestDb harness) → server API
  suites (`apps/melodee-server/tests/`) → golden/differential fixtures where a captured contract
  exists. New adapters plug into `compatibility/scenarios/*.json` + `compare_captures.py`.
- **Docs:** `/docs` updated in the same PR for user-visible behavior; `FEATURE-PARITY-MATRIX.md`,
  `REQUIREMENTS-INDEX.md` (via `extract_requirements.py`), and checklists updated per WP.
- **Gate discipline:** `cargo fmt --all` then `./scripts/check.sh` before any WP is called done.

## 5. Sequencing summary and critical path

```
W0-1 (close Phase 8) ──► W0-2..W0-5 (reconcile + decisions) ──► Wave 1 (WP-01..06, parallelizable)
                                                                ──► Wave 2 (WP-07..11, needs ADRs)
                                                                ──► Wave 3 (WP-12..14, needs product decisions)
                                                                ──► Wave 4 (closeout + report)
```

Critical path: Phase 8 Jellyfin completion → decision bundle → WP-12 waiver-or-build → closeout.
Everything in Wave 1 is parallelizable across agents once W0-2/W0-3 land, since those WPs touch
disjoint modules (`native/`, `opensubsonic/`, `jellyfin/`, `melodee-cli`, `persistence`).

## 6. Risks

1. **Phase 8 Jellyfin is the long pole** (0/60). Phase 9 cannot legally start before it; the plan
   front-loads it as W0-1 rather than pretending it is done.
2. **Party Mode ambiguity** is the largest scope swing: waiver ADR ≈ one WP; full build ≈ the
   largest WP in the phase (sessions, moderation, backends, jukebox engine, schema).
3. **JS runtime security review** may force a design iteration; sandboxing (PRD-SCRIPT-004) is
   non-negotiable and should be prototyped before the full hook inventory is wired.
4. **Matrix vs. reality drift** (§1.4.1) means Gate G currently *fails on paper* despite real
   implementations; W0-2 must land before any WP claims completion.
5. **ADR-0109 scope ambiguity** (validation labeled Phase 9, implementation Phase 10) can block
   closeout if not amended early.

---

## Review report (per AGENTS.md required workflow)

- **Files changed:** `design/agents/PHASE-9-PLAN.md` (this document, new).
- **Requirements satisfied:** none implemented — this is the planning artifact only.
- **ADRs applied/proposed:** proposes future ADRs for party/jukebox waiver-or-build, JS runtime,
  device-profile stream negotiation, provider inventory; proposes amending ADR-0109.
- **Tests run:** none (planning only). Repository state verified read-only by survey.
- **Compatibility impact:** none. Plan explicitly routes all intentional 404/410/501 changes
  through the 4-step contract-change process.
- **Data/migration impact:** none now; §4 enumerates the schema-ADR obligations Phase 9 will incur.
- **Documentation impact:** Updated — added this plan under `design/agents/` (internal engineering
  docs). No `/docs` (public) changes: no user-visible behavior changed. `FEATURE-PARITY-MATRIX.md`
  staleness is documented here; its reconciliation is scheduled as W0-2 rather than done in this
  planning change.
- **Remaining risks:** §6.
