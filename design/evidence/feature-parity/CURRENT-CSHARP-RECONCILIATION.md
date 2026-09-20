# Current C# feature-parity reconciliation

- **Audit date:** 2026-09-20
- **Legacy behavioral-oracle checkout:** `/home/steven/src/github/melodee`, branch
  `sph.2026-07-11.01`, `4028b58dbd3302ed431e2c5b32d9dde5634db218`
- **Rust replacement checkout:** `/home/steven/src/github/melodee-rs`, branch `main`,
  `7ab01c515295d9f883585157287436f05d37f422`
- **Oracle qualification:** the local C# checkout is ahead of its local `origin/main`
  reference (`d91342e5`) and is the frozen compatibility-oracle commit used by this
  repository. This reconciliation deliberately evaluates the checked-out C# source,
  not the older remote reference.
- **Hamstik organization/project:** `sph` / `MEL` (`Melodee`)
- **Parity-closure epic:** `MEL-1` — *Complete remaining C# feature parity for Rust
  replacement* (Backlog)

`IMPLEMENTATION.md`, requested as governing material, is not present in this checkout.
The audit used `AGENTS.md`, `PRD.md`, `SPEC.md`, the definition of done, the
requirements index and overrides, accepted relevant ADRs, compatibility contracts,
existing docs, source, runtime router composition, migrations, and tests instead.

## Method and scope

Each conclusion follows the required chain: current C# source behavior → Rust
implementation → Rust runtime wiring → tests/compatibility evidence → disposition.
Old reports and the pre-existing matrix were leads only.  C# `Routes.razor` installs
an `AuthorizeRouteView`, `Program.cs` maps the Blazor application/controllers and
registers the services/jobs below; a page was treated as current unless an accepted
ADR retires or relocates it.

The canonical ledger is the updated
[`FEATURE-PARITY-MATRIX.md`](../../requirements/FEATURE-PARITY-MATRIX.md). This
report does not replace it. The counts below include its 53 pre-existing capability
rows, two newly identified C# capabilities, and four replacement-qualification
obligations that must be closed before cutover: **59 rows** total.

| Disposition | Count |
|---|---:|
| PARITY | 39 |
| PARTIAL | 5 |
| MISSING | 1 |
| APPROVED DIFFERENCE | 5 |
| UNVERIFIED | 9 |
| NOT APPLICABLE | 0 |

## Baseline capture

Both worktrees were clean before this audit. No files in the C# checkout were
modified, reset, stashed, cleaned, restored, or otherwise changed.

| Repository | `git status --short` | Branch | HEAD | Remote |
|---|---|---|---|---|
| C# | clean | `sph.2026-07-11.01` | `4028b58dbd3302ed431e2c5b32d9dde5634db218` | `origin` fetch/push `https://github.com/melodee-project/melodee.git` |
| Rust | clean | `main` | `7ab01c515295d9f883585157287436f05d37f422` | `origin` fetch/push `https://github.com/melodee-project/melodee-rs.git` |

The respective 15-commit logs and remote listings were captured during the audit.
The immediately relevant heads are C# `4028b58d chore(deps): upgrade
Microsoft.OpenApi …` and Rust `7ab01c5 feat: Accept ordinary Unknown Releases and
reconcile obsolete default review rule`.

## Capability reconciliation

`—` in the Hamstik column means no backlog work is appropriate. Every incomplete
row maps to a child of Backlog epic `MEL-1` in the confirmed `MEL` / `Melodee`
project.

| Capability | Legacy evidence | Rust implementation and wiring | Disposition | Concrete gap / proof boundary | Hamstik |
|---|---|---|---|---|---|
| Onboarding | `OnboardingStateService`, setup pages | persistent onboarding marker, admin API/UI, CLI | PARITY | preflight and completion tests | — |
| Libraries/ACL | C# `LibraryService`, library pages | group/library predicates and Admin UI | PARITY | ACL API tests | — |
| Inbound | `LibraryInboundProcessJob` | acquisition → staging → work → publication | PARITY | durable/restart/reuse tests | — |
| Staging/review | C# staging jobs/pages | review state/history, CLI, Admin UI | PARITY | durable review tests | — |
| Media normalization | converter services | media + ingestion crates | PARITY | real-media tests | — |
| Artwork | C# artwork/file paths | catalog image overrides/publication | PARITY | API/compatibility tests | — |
| Metadata transforms | `MediaEditService` | routed `admin_metadata.rs`, `TagRewrite`, panel | UNVERIFIED | no DB-backed canonical-file apply/restart/revert proof | MEL-8 |
| Storage/indexing | C# media/storage coupling | immutable publication, Postgres/outbox/Tantivy | PARITY | direct integration evidence | — |
| Artists | artist pages/service | catalog APIs/Admin UI merge/split | PARITY | transaction tests | — |
| Listener UI | Blazor listener pages | MeloAmp native client | APPROVED DIFFERENCE | ADR-0161 relocation | — |
| Releases | album pages/service | catalog/admin APIs/UI | PARITY | UI/API tests | — |
| Tracks | song pages/service | catalog/admin APIs/UI | PARITY | UI/API tests | — |
| Search | search page/service | Rust search projection | PARITY | parser/rebuild tests | — |
| MQL | C# MQL corpus | Rust MQL parser/translator | PARITY | compatibility corpus | — |
| Ratings/favorites | native state handlers | native/OS/JF state stores | PARITY | differential state tests | — |
| Playlists | pages/services/native routes | native/OS/JF playlist stores | PARITY | route and import tests | — |
| Smart playlists | C# smart-playlist service | bounded MQL CRUD/evaluation | PARITY | runtime tests | — |
| Native play queue | frozen controller absence | frozen 404; OS/JF queues | APPROVED DIFFERENCE | ADR-0123 | — |
| Scrobbling | native/OS scrobble paths | transactional Rust paths | PARITY | differential tests | — |
| Last.fm | link/session/scrobble service | `lastfm.rs`, sealed links, dispatch | UNVERIFIED | no real account/configured-state acceptance | MEL-9 |
| Metadata providers | nine provider services | descriptors/cache/enrichment worker | UNVERIFIED | no real credentials/quota/current-C# evidence | MEL-10 |
| Charts | chart pages/jobs/files | chart store/import/refresh | PARITY | generated aggregates accepted by ADR-0148 | — |
| Recommendations | C# discovery | materialized candidates | PARITY | route/store tests | — |
| Analytics | stats/activity pages | listening facts and stats | PARITY | API/store tests | — |
| Audio features | exposed C# feature data | native audio-feature paths | PARITY | compatibility tests | — |
| Equalizer | preset/preferences endpoints | Rust preference store | PARITY | API tests | — |
| Radio | stations page/service | Admin/OS store | PARITY | API/store tests | — |
| Shares | anonymous `/share/{id}` player | secure bounded JSON resolver | APPROVED DIFFERENCE | ADR-0129 deliberately rejects HTML player | — |
| Requests/comments | request pages/services | native/community state | PARITY | API/store tests | — |
| Podcasts | pages/jobs/library | worker, native/OS/JF APIs | UNVERIFIED | no candidate real-feed/retry/retention acceptance | MEL-11 |
| Admin users/roles | user pages/services | admin/native/OS management | PARITY | API/UI/compatibility tests | — |
| Groups/ACL | groups/pages/library bindings | membership and predicate store | PARITY | integration tests | — |
| Account profile/avatar | `/account/profile`, User Images library | avatar API; read-only `/user/me` | PARTIAL | no self-register/edit, bio/time-zone, or avatar-file import | MEL-2 |
| Per-user device profiles | profile tab/service | global admin metadata only | PARTIAL | ADR-0148 records every source row reference-only | MEL-5 |
| Party Mode | `/party`, SignalR hub | frozen native absence | APPROVED DIFFERENCE | ADR-0133/ADR-0141 | — |
| Jukebox | `/jukebox` | OS 410 | APPROVED DIFFERENCE | ADR-0133/ADR-0140 | — |
| Playback backend | legacy backend management | endpoint/backend routes | PARITY | API tests | — |
| Script hooks | C# scripting/events | bounded Boa sandbox | PARITY | live/inert mapping documented/tested | — |
| Config import/export | settings/onboarding import | preview import, redacted export, API/CLI | PARITY | tests | — |
| Notification templates | `EmailTemplateService` Templates library | DB templates/SMTP/admin/CLI | PARTIAL | custom library bodies have no importer/disposition | MEL-6 |
| API docs | legacy OpenAPI | docs site/API reference | PARITY | docs validation | — |
| Diagnostics | Doctor page/service | health, doctor, system UI | PARITY | API/UI tests | — |
| Maintenance jobs | Quartz jobs | periodic workers | PARITY | scheduler/worker tests | — |
| Job controls/history | admin jobs page | durable controls/history | PARITY | API/UI tests | — |
| Custom themes | Theme library/service | DB pack import/export/Admin UI | PARTIAL | no C# theme-files inventory/import/disposition | MEL-7 |
| Appearance | C# themes | Admin UI provider | PARITY | visual tests | — |
| Localization/RTL | C# resources | i18next/RTL Admin UI | PARITY | UI tests | — |
| OpenSubsonic | controllers/contracts | independent adapter | PARITY | route evidence + differential | — |
| Native API v1 | controllers/contracts | independent native adapter | PARITY | route matrix + differential | — |
| Jellyfin | controller subset | independent Jellyfin adapter | PARITY | route evidence + differential | — |
| OCI artifacts | deployment image | Dockerfile/release workflow | UNVERIFIED | no exact-SHA multiarch, scan, SBOM, attestation, arm64 run | MEL-12 |
| CLI administration | C# admin operations | Rust CLI | PARITY | command/integration tests | — |
| Browser reset completion | anonymous C# account pages | reset API only | PARTIAL | reset URL targets an unserved Rust HTML path | MEL-3 |
| Custom content blocks | `FileCustomBlockService` and Templates library | no Rust search hit/store/render/migration | MISSING | no approved retirement | MEL-4 |
| Migration rehearsal | production C# state/media | CLI importer + synthetic drill | UNVERIFIED | no authorized private snapshot/filesystem rehearsal | MEL-13 |
| Cutover/rollback | legacy authority switch | freeze/rollback tooling | UNVERIFIED | no production backup/restore/traffic rollback drill | MEL-14 |
| Candidate differential | current C# behaviors | capture/compare harness | UNVERIFIED | no complete replay/report for the two SHAs above | MEL-15 |
| Scale/fault acceptance | production-scale C# replacement | load/fault suites | UNVERIFIED | no documented target-scale budget acceptance | MEL-16 |

## C# Blazor route/page audit

The source inventory found **62 routable `.razor` pages covering 67 declared routes**.
The grouping below accounts for every page. A group can have an approved relocation and still
be listed as partial where a separate C#-specific behavior remains, such as custom blocks.

| C# routes/pages | Role and major actions | Rust replacement surface | Disposition |
|---|---|---|---|
| `/`, `/about`, `/stats`, `/search[/{Query}]`, `/musicplayer`, `/activity/nowplaying` | listener dashboard, browse/search/playback/statistics | MeloAmp/native APIs (ADR-0161) | approved relocation; dashboard custom block remains missing |
| `/account/login`, `/login`, `/account/logout` | browser authentication/session | native client auth/cookie APIs | core parity; login custom blocks remain missing |
| `/account/register` | public account creation, optional invite/first-admin behavior | no public Rust registration route | MISSING |
| `/account/profile` | self edit, avatar, language/theme/bio/time zone, device profiles | avatar APIs; global device admin metadata | PARTIAL |
| `/account/forgot-password`, `/account/reset-password` | anonymous reset request and browser token completion | native reset request/validate/confirm API | PARTIAL |
| `/onboarding`, `/onboarding/blocking` | setup state/checks | admin onboarding API/UI/CLI | PARITY |
| `/admin/dashboard`, `/admin/doctor`, `/admin/jobs`, `/admin/settings`, `/admin/scripts[/{EventName}]`, `/admin/themes`, `/admin/usergroups` | administration/diagnostics | Rust Admin UI and API | PARITY |
| `/data/artists*`, `/media/artist*`, `/data/artist*` | artist browse/edit | Admin UI + catalog APIs | PARITY |
| `/data/albums*`, `/media/album*` | release browse/edit | Admin UI + catalog APIs | PARITY |
| `/data/songs*` | track browse/detail | Admin UI + catalog APIs | PARITY |
| `/data/libraries*`, `/media/library*` | media roots/library administration | Admin UI/ACL/configuration | PARITY |
| `/data/playlists*` | playlist browse/detail | MeloAmp/native/OS/JF APIs | approved relocation |
| `/data/podcasts*` | podcasts, episode search/detail | MeloAmp/native/OS/JF APIs | approved relocation; real-feed qualification pending |
| `/data/radiostations` | station management | Admin/OS radio APIs | PARITY |
| `/charts*` (including create/edit/detail/reports) | chart curation/reports | Admin API/UI and worker | PARITY |
| `/requests*`, `/data/shares`, `/share/{ShareUniqueId}` | community requests, shares, public resolution | native/Admin APIs; secure JSON share alias | requests PARITY; public HTML player approved difference |
| `/data/users*`, `/user/{id}`, `/usergroup/{id}` | user and group management | Admin UI/API | PARITY |
| `/party`, `/party/{Session}` | party sessions | retired | APPROVED DIFFERENCE, ADR-0133/0141 |
| `/jukebox` | server-side playback | retired | APPROVED DIFFERENCE, ADR-0133/0140 |

The page accounting is **52 fully replaced, 5 partially replaced pages/workflows, 1 missing
page/workflow, and 4 approved relocations/retirements**. The partial count is dashboard/login
custom-block placement plus profile and the two reset pages; the approved group is the secure
public-share representation and the three Party/Jukebox retirements. It does not mean a second
server listener SPA is required.

## Filesystem/library audit

| Legacy library/state | Current C# use and ownership | Rust counterpart / cutover disposition | Result |
|---|---|---|---|
| Inbound | external source watched by inbound jobs | `MELODEE__INGEST__INBOUND_ROOT`; acquisition removes successful sources into staging | PARITY |
| Staging | durable review/source location and auto-move job | independently configured staging root; review/quarantine stays there | PARITY |
| Work area | C# transient processing scratch behavior | independently configured attempt-isolated normalization work root | PARITY |
| Canonical storage | C# storage library | independently configured canonical root/immutable publication | PARITY |
| Playlists | database plus uploaded-file staging artifacts | playlists import; upload artifacts are correctly summarized as staging under ADR-0148 | PARITY |
| Podcasts | feed/download library and user resume/history | dedicated Rust root and state importer; absolute host paths requeue under ADR-0148 | UNVERIFIED pending real cutover/feed proof |
| User Images | avatar GIFs at `User.ToAvatarFileName(UserImages.Path)` | DB avatar authority, but no filesystem importer | PARTIAL |
| Themes | operator-owned Theme-library directories/packs | Rust DB theme packs but no legacy pack inventory/import/disposition | PARTIAL |
| Templates | localized password-reset text/HTML and `custom-blocks/**/*.md` | notification-template DB has no library importer; no custom-block equivalent | PARTIAL / MISSING |
| Chart Images | generated chart library images | Rust charts regenerate aggregates/images; accepted ADR-0148 disposition | PARITY |
| Media/sidecars | canonical files and metadata | adopted immutable media; tag transform path needs operational proof | UNVERIFIED |

The new inbound model is implemented rather than merely documented: `IngestionConfig` requires
disjoint inbound/staging/work/canonical roots; acquisition writes an intent and atomically moves
or verified-copies to `staging/releases/<work-item>/source` before the inbound source is
removed; `inbound_active` reconciliation filters only active discovery states; and acquisition
tests cover retained `NeedsReview`/`Quarantined` staging sources and a reused source path as a
new acquisition. This is not an outstanding inbound gap.

## Approved differences

- Listener-server SPA → MeloAmp, with Rust retaining APIs and `/admin`: ADR-0161.
- Native v1 queue absence and Party Mode retirement: ADR-0123 and ADR-0133; legacy party
  routes are retired by ADR-0141.
- Jukebox page retirement while OpenSubsonic preserves `410`: ADR-0133 and ADR-0140.
- Public share path remains the secure bounded JSON compatibility representation, not the C# HTML
  player: ADR-0129.
- Regenerated chart aggregates/images, playlist-upload staging artifacts, session artifacts, and
  host-specific downloaded podcast paths have explicit migration dispositions: ADR-0148. These
  do not waive the still-required actual cutover qualification.

## Data-migration gaps

The runtime can support the following data, but current migration/cutover evidence does not
preserve it sufficiently for C# retirement:

- C# User Images-library avatars are not enumerated/imported to `native_user_avatars`.
- `Users.Description`/bio and `Users.TimeZoneId` are neither target fields nor imported; C# users
  can edit both.
- Per-user `UserDeviceProfiles` are deliberately only migration issues, not usable user-owned
  target profiles.
- Operator-authored Templates-library email bodies have no importer.
- Theme-library pack directories have no importer or accepted retirement/disposition.
- Custom-block Markdown has no target representation or decision.
- Legacy downloaded podcast files require a proven, operator-approved relative-root adoption or
  intentionally re-downloadable cutover handling; absolute paths requeue by design.

## Hamstik inventory and backlog

Hamstik CLI discovery used the installed skill, `--help`, and explicit `--org sph --project MEL`
commands. The confirmed target is `MEL` / `Melodee`; it had no existing work items, so there was no
duplicate epic or child to reuse. Backlog epic `MEL-1` and the fifteen evidence-backed child items
below were then created. No existing item was changed and no item was marked In Progress or Done.

### Child MEL-2: complete C# account self-service and profile-data preservation

- **Type / priority / parent:** feature / high / `MEL-1`.
- **Outcome:** C# users can register when enabled, edit their own supported profile values, and
  retain avatar/profile data through migration without expanding admin authority.
- **Legacy behavior:** `/account/register` calls `UserProfileService.RegisterAsync`; `/account/profile`
  edits username, email, `Description`, `TimeZoneId`, language/theme, avatar, and device tab data.
  Avatars live in `LibraryType.UserImages` via `ToAvatarFileName`.
- **Current Rust state:** `native/auth.rs` exposes authentication and read-only `/api/v1/user/me`;
  `native/avatars.rs` supports current uploads. `legacy/import_users.rs` imports language/theme
  but not bio/time zone and never reads avatar files.
- **Gap / required behavior:** add an authorized, configuration-controlled self-service surface and
  explicit migration/cutover handling for every representable C# account field/avatar, or obtain
  an Accepted ADR for an intentional field retirement before cutover.
- **Acceptance criteria:** normal and disabled registration paths; self-only authorization; profile
  validation/audit/restart behavior; migrated image-byte/content-type verification; explicit
  reports for non-representable fields; native/OS/JF compatibility unchanged; Admin UI/MeloAmp and
  public docs updated; unit, PostgreSQL integration, and migration fixture tests pass.
- **Compatibility impact:** Native API, OpenSubsonic avatar behavior, Admin UI, MeloAmp, migration.
- **Evidence required:** C# fixture with profile data/avatar migrated and exercised after restart;
  route/API authorization tests; a documented cutover report.
- **Out of scope:** recreating the legacy Blazor listener SPA or silently granting admin rights.

### Child MEL-3: provide browser password-reset completion

- **Type / priority / parent:** feature / high / `MEL-1`.
- **Outcome:** the password-reset URL emitted by Rust is usable in a bounded browser security flow.
- **Legacy behavior:** anonymous `/account/forgot-password` and `/account/reset-password` request,
  validate, and complete token-based resets.
- **Current Rust state:** `native/auth.rs` routes reset request/validate/confirm but sends
  `{publicOrigin}/reset-password?token=…`, for which no handler is registered.
- **Gap / required behavior:** serve a minimal accessible completion page (or make a supported
  client endpoint the emitted URL) that validates and confirms without token leakage or a listener SPA.
- **Acceptance criteria:** valid/expired/replayed/malformed tokens; generic request response;
  rate-limit and CSRF/session boundary; localized error states; notification link round-trip;
  browser and API tests; public docs.
- **Compatibility impact:** Native API, Admin UI, MeloAmp, notification templates.
- **Evidence required:** browser-level end-to-end reset from emitted message URL and security-log review.
- **Out of scope:** a general listener web application.

### Child MEL-4: decide and preserve legacy custom content blocks

- **Type / priority / parent:** feature / medium / `MEL-1`.
- **Outcome:** every active C# Templates-library `custom-blocks/**/*.md` asset is migrated/rendered
  in its replacement surface or is retired by a new Accepted product decision with owner notice.
- **Legacy behavior:** `FileCustomBlockService` resolves and sanitizes active blocks such as
  `dashboard.top`, `login.top`, `register.top`, and recovery-page blocks.
- **Current Rust state:** no custom-block type, importer, route, renderer, or migration issue exists.
- **Gap / required behavior:** inventory source files, preserve bounded/sanitized rendering where
  needed, or explicitly decide each placement's retirement before data is discarded.
- **Acceptance criteria:** traversal-safe import, size/content sanitization, localization/access
  rules, upgrade report, replacement-surface tests, and docs/ADR as appropriate.
- **Compatibility impact:** Admin UI/MeloAmp/browser account pages/migration.
- **Evidence required:** fixture tree imported or an Accepted ADR with affected-file inventory.
- **Out of scope:** recreating arbitrary server-side Blazor components.

### Child MEL-5: preserve or replace per-user device profiles

- **Type / priority / parent:** feature / high / `MEL-1`.
- **Outcome:** C# per-user device profiles/defaults/priority survive with explicit target semantics.
- **Legacy behavior:** `/account/profile` has per-user CRUD/default/priority through
  `UserDeviceProfileService`.
- **Current Rust state:** `admin_device_profiles.rs` is global metadata; migration records
  `device_profile.reference_only` per ADR-0148.
- **Gap / required behavior:** define a user-owned target mapping and migration, or supersede the
  current reference-only disposition with an Accepted, user-notified retirement decision.
- **Acceptance criteria:** ownership isolation, default selection, ordering, restart/migration
  idempotency, authorization, issue/report behavior, Admin UI/client/docs, and integration tests.
- **Compatibility impact:** Admin UI, MeloAmp, migration, possible stream/device selection.
- **Evidence required:** multi-user migration fixture proving no cross-user leakage and cutover report.
- **Out of scope:** implementing transcoding/device-selection policy unless the accepted design needs it.

### Child MEL-6: migrate legacy notification-template library content

- **Type / priority / parent:** feature / high / `MEL-1`.
- **Outcome:** operator-authored C# localized reset templates survive into Rust notification templates.
- **Legacy behavior:** `EmailTemplateService` reads `Templates/<culture>/PasswordReset.{txt,html}`
  before falling back to built-ins.
- **Current Rust state:** `notification_templates` and admin/CLI editing are present, but legacy
  import has no Templates-library scanner.
- **Gap / required behavior:** inventory, validate, import/map bodies and subjects safely, or report
  each unsupported asset before an Accepted retirement.
- **Acceptance criteria:** locale/placeholder mapping, bounded input, secret-safe diagnostics,
  preview/dry-run, idempotent migration, fallback behavior, test-send evidence, and docs.
- **Compatibility impact:** notification delivery, browser reset, migration, Admin UI/CLI.
- **Evidence required:** fixture custom template sends the expected redacted body after migration.
- **Out of scope:** unrelated notification-channel expansion.

### Child MEL-7: preserve legacy custom theme packs through cutover

- **Type / priority / parent:** feature / medium / `MEL-1`.
- **Outcome:** Theme-library packs are imported into Rust's database-owned pack format or have an
  Accepted, operator-visible retirement/export plan.
- **Legacy behavior:** `ThemeService` discovers, imports, exports, serves, and deletes packs under
  the C# Theme library; `ThemesController` exposes the functionality.
- **Current Rust state:** native discovery/Admin UI support bounded archive import/export in
  `native/discovery.rs`, but the legacy importer has no Theme-library stage.
- **Gap / required behavior:** build an inventory/translation and ensure user theme preference
  references remain valid after cutover.
- **Acceptance criteria:** malicious archive/file rejection, asset/metadata fidelity, preference
  remap/report, idempotency, rollback safety, UI/API tests, and migration documentation.
- **Compatibility impact:** native theme API, Admin UI, MeloAmp, migration.
- **Evidence required:** real-format legacy pack fixture imported/exported and applied after restart.
- **Out of scope:** new visual theme features.

### Child MEL-8: qualify metadata transforms against canonical storage

- **Type / priority / parent:** task (validation) / high / `MEL-1`.
- **Outcome:** prove the implemented safe metadata transformations are safe and recoverable on the
  actual PostgreSQL/canonical-file path; implement only defects the validation exposes.
- **Legacy behavior:** `MediaEditService` mutates current library tags for the listed operations.
- **Current Rust state:** `admin_metadata.rs` has preview/apply/list/revert, atomic file replacement,
  audit, and decoded-audio fingerprints; it is merged in `main.rs` and rendered by the Admin UI.
- **Gap / required behavior:** exercise apply, failure rollback, process restart, audit/revert, and
  catalog/search consistency through a real target database and canonical root.
- **Acceptance criteria:** every supported operation, authorization, injected I/O/FFmpeg failure,
  no decoded-audio drift, restart before/after commit, exact revert, and no public ID/path change.
- **Compatibility impact:** Admin UI, canonical media, migration; Native/OpenSubsonic/Jellyfin
  public contracts must remain unchanged.
- **Evidence required:** committed integration/fault evidence and an operator runbook.
- **Out of scope:** arbitrary free-form tag editing.

### Child MEL-9: qualify Last.fm configured-state behavior

- **Type / priority / parent:** task (validation) / medium / `MEL-1`.
- **Outcome:** prove real account linking/scrobbling against current C# behavior and provider limits.
- **Legacy behavior:** C# provider/session lifecycle and scrobbling are active.
- **Current Rust state:** `lastfm.rs`, encrypted/digested link store, and dispatch are wired; tests
  use a mock server.
- **Gap / required behavior:** use controlled credentials/query corpus to validate wire, session
  invalidation, rate limits, cache/failure behavior, and identity ownership.
- **Acceptance criteria:** no secret logging, retry/no-retry semantics documented, link/unlink,
  invalid session, rate-limit and outage evidence; commit redacted result.
- **Compatibility impact:** Native API, OpenSubsonic, Jellyfin, MeloAmp, migration secrets.
- **Evidence required:** redacted live acceptance report linked from the matrix.
- **Out of scope:** a new queue/retry architecture unless a defect demands it.

### Child MEL-10: qualify external metadata providers

- **Type / priority / parent:** task (validation) / medium / `MEL-1`.
- **Outcome:** qualify all enabled external enrichment providers using representative controlled
  credentials and current-C# comparison; implement only defects exposed.
- **Legacy behavior:** current C# provider inventory/settings and graceful-disable behavior.
- **Current Rust state:** provider descriptors, parsers, cache, durable tasks, rate-limit handling,
  candidate evidence, and DecentDB path are wired and fixture-tested.
- **Gap / required behavior:** prove current real provider behavior, quota/caching/failure handling,
  and Melodee-owned identity under configured conditions.
- **Acceptance criteria:** each enabled provider has representative successes/failures, bounded
  concurrency/cache behavior, quota evidence, redacted configuration, and an evidence report.
- **Compatibility impact:** provider configuration, catalog enrichment, Admin UI, migration.
- **Evidence required:** redacted live-provider acceptance plus classified C# differences.
- **Out of scope:** adding additional providers.

### Child MEL-11: qualify podcast feeds and downloaded-media cutover

- **Type / priority / parent:** task (validation) / medium / `MEL-1`.
- **Outcome:** prove real feed refresh/download/retention/recovery and intentional existing-download
  cutover handling.
- **Legacy behavior:** C# podcast jobs use the Podcast library; users retain subscriptions,
  downloads, bookmarks, and play history.
- **Current Rust state:** worker/API/OS/JF paths and importer are present; absolute host paths
  requeue according to ADR-0148.
- **Gap / required behavior:** prove representative public feeds, redirects/failures, storage root,
  retention/orphan recovery, and controlled re-download/adoption during cutover.
- **Acceptance criteria:** real-feed corpus, restart/lease recovery, quota/retention, imported
  resume/history, explicit affected download report, and docs/runbook.
- **Compatibility impact:** Native API, OpenSubsonic, Jellyfin, MeloAmp, migration/storage.
- **Evidence required:** redacted deployment acceptance report and cutover issue counts.
- **Out of scope:** a new podcast product model.

### Child MEL-12: publish and execute current-candidate multi-architecture release artifacts

- **Type / priority / parent:** task (validation) / high / `MEL-1`.
- **Outcome:** immutable current-SHA amd64/arm64 OCI artifacts are built, scanned, attested, and
  executed before C# replacement.
- **Legacy behavior:** deployable C# container/server is a current replacement prerequisite.
- **Current Rust state:** Dockerfile, local amd64 smoke, and release-only workflow exist.
- **Gap / required behavior:** execute the ADR-0144 release flow for this candidate, including arm64.
- **Acceptance criteria:** manifest digest, non-root readiness/media-tool/Admin UI smoke on both
  architectures, SBOM/vulnerability report, checksum/attestation, and release-owner acceptance.
- **Compatibility impact:** deployment, Admin UI, CLI, all APIs.
- **Evidence required:** immutable artifact identifiers and attached redacted release evidence.
- **Out of scope:** force-publishing an unapproved release.

### Child MEL-13: rehearse authorized C# migration and filesystem-data preservation

- **Type / priority / parent:** task (validation) / urgent / `MEL-1`.
- **Outcome:** an authorized representative C# snapshot and file libraries migrate safely into a
  disposable Rust target with reconciled identities/media/user-owned data and explicit issues.
- **Legacy behavior:** C# database plus canonical media and listed Library roots hold cutover data.
- **Current Rust state:** legacy CLI import/verify/freeze tooling and synthetic fixtures exist.
- **Gap / required behavior:** exercise actual authorized data including public IDs, media, user
  assets, templates/themes, and all intentional exclusions; resolve or approve every error.
- **Acceptance criteria:** source read-only, target isolation, backup/restore, idempotent retry,
  public-ID/media verification, issue classification, no private data committed, and docs.
- **Compatibility impact:** migration, every API/client, filesystem roots.
- **Evidence required:** access-controlled rehearsal report with counts and approval sign-off.
- **Out of scope:** production cutover or destructive source cleanup.

### Child MEL-14: rehearse cutover and rollback under operator control

- **Type / priority / parent:** task (validation) / urgent / `MEL-1`.
- **Outcome:** operators can freeze, switch traffic, validate Rust, roll back traffic/authority, and
  prove legacy authority remains intact.
- **Legacy behavior:** C# remains source of authority until a successful cutover.
- **Current Rust state:** `legacy/freeze.rs`, migration run states, rollback CLI/runbook, and
  synthetic drill exist.
- **Gap / required behavior:** run the authorized topology-specific backup/restore/traffic exercise.
- **Acceptance criteria:** approvals, exact topology validation, timed freeze/final delta, Rust
  smoke, rollback command/audit, restored C# writes, retained failed target, and no secret logs.
- **Compatibility impact:** migration, all clients and deployment.
- **Evidence required:** signed operator rehearsal record and durable rollback audit ID.
- **Out of scope:** actual production cutover without separate approval.

### Child MEL-15: run full current-candidate C# differential qualification

- **Type / priority / parent:** task (validation) / high / `MEL-1`.
- **Outcome:** every supported native, OpenSubsonic, and Jellyfin scenario is replayed against the
  stated C# and Rust SHAs with every difference classified.
- **Legacy behavior:** C# controllers/API contracts are the behavioral oracle.
- **Current Rust state:** capture/compare scripts, route manifests, 424 promoted golden captures,
  and compatibility tests exist.
- **Gap / required behavior:** rebuild/replay the full candidate comparison, not historical reports.
- **Acceptance criteria:** pinned SHAs, reproducible environment, zero unexplained differences,
  accepted ADR reference for each intentional one, and no normalizer masks meaningful data.
- **Compatibility impact:** Native API, OpenSubsonic, Jellyfin, MeloAmp.
- **Evidence required:** committed/redacted differential report and updated route evidence.
- **Out of scope:** changing a wire contract solely to satisfy a fixture.

### Child MEL-16: accept performance and fault-recovery budgets at replacement scale

- **Type / priority / parent:** task (validation) / high / `MEL-1`.
- **Outcome:** documented scale, latency, resource, failure, restart, and recovery budgets are met
  on a representative replacement topology.
- **Legacy behavior:** C# is a running production baseline whose replacement must not regress unsafe
  operational behavior.
- **Current Rust state:** bounded workers, benchmark harnesses, 200-Release fault scenarios, and
  query-shape tests exist.
- **Gap / required behavior:** execute the required target-scale acceptance rather than extrapolate
  from local/synthetic tests.
- **Acceptance criteria:** PRD-SCALE/PERF budgets, ingestion/search/catalog/provider worker load,
  fault matrix, resource caps, cancellation/restart convergence, and reproducible measurements.
- **Compatibility impact:** operational behavior, migration/cutover, all clients under load.
- **Evidence required:** versioned benchmark/fault report approved by the release owner.
- **Out of scope:** unbounded stress testing against production services.

## Validation performed for this audit

- Read-only baseline Git state/branch/HEAD/log/remotes recorded for both repositories.
- Hamstik skill installed/checked; CLI project and work-item reads used `--no-input` with explicit
  organization/project values. The confirmed `MEL` project was empty, then received Backlog epic
  `MEL-1` and its 15 scoped Backlog children (`MEL-2` through `MEL-16`).
- Current C# route extraction found 62 page files/67 route declarations; services/controllers/jobs,
  libraries, account pages, and controllers were inspected directly.
- Rust router composition, Admin UI calls, migration importer, filesystem configuration, ingress
  state machine, compatibility assets, and relevant direct tests were inspected.
- `cargo test -p melodee-ingestion --test acquisition needs_review` passed (1 test), and
  `cargo test -p melodee-ingestion --features test-support --test acquisition
  delayed_cleanup_after_path_reuse_preserves_new_occurrence` passed (1 test).
- `cargo test -p melodee-server --lib admin_metadata` passed (5 tests). Those focused unit tests
  reinforce the implementation finding but do not remove the canonical-file/PostgreSQL end-to-end
  evidence boundary recorded above.
- `python3 scripts/validate_repository.py` passed and `python3 scripts/extract_requirements.py
  --check` reported the 239-row requirements index current.
- `make check-fast` passed (repository/requirements/override/client-artifact checks, formatting,
  workspace Clippy, and database-free Rust library/binary tests). It correctly reported the five
  externally pending client-acceptance artifacts; this audit does not treat that pending state as
  pass evidence.
- `./scripts/check.sh` was not run because this repository has no such file. Repository
  documentation/requirements validation passed with the reconciliation change.

## Final answer

**Yes.** If the C# application were shut down today, evidence-backed losses or unproven
replacement obligations remain: self-service registration/profile data and avatar migration,
per-user device profiles, operator-authored Templates/Theme-library content, custom content
blocks, usable browser reset completion, and the validation/qualification boundaries above.
Every one maps to a concrete Backlog child of `MEL-1` in the confirmed `MEL` / `Melodee` project.
Approved differences are listed above and are not backlog items.
