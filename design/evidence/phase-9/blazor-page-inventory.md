# Phase 9 WP-14 — Blazor page inventory (PRD-WEB-001)

**Purpose:** complete inventory of every page/route in the legacy Blazor server UI,
with per-page auth level and exposed capability, as the evidence base for the
keep/replace/drop classification feeding PRD-WEB-002 and the Phase 10 admin UI.

**Date:** 2026-08-14

**Source:** `src/Melodee.Blazor` in the legacy oracle `/home/steven/src/github/melodee`
at frozen commit `4028b58dbd3302ed431e2c5b32d9dde5634db218` (re-verified 2026-08-14
via `git log -1`). Ground-up rewrite rule applies: the C#/Blazor code below is
behavioral evidence, not an architecture template. Governing authorities for the
future Rust admin UI: PRD-WEB-001/002, ADR-0133 (party/jukebox waiver),
ADR-0134 (Phase 9 parity-domain schema), ADR-0135 (Boa scripting runtime), and the
compatibility rules in `AGENTS.md` (no route removal without the 4-step
contract-change process).

**Method:** read-only inspection of the frozen tree. All `.razor` files enumerated
under `src/Melodee.Blazor/Components`; routes extracted from `@page` directives; auth
levels from `@attribute [Authorize(...)]` / `[AllowAnonymous]` plus the global
`@attribute [Authorize]` in `Components/_Imports.razor`; representative pages
(Dashboard, Media/Artists, PartyMode, PartySession, Jukebox, MusicPlayer,
Data/Users, Activity/NowPlaying, Onboarding pages, Error) read directly to confirm
purpose and in-markup role gating (`IsAdmin()/IsEditor()` visibility checks).
Controllers, SignalR hubs, middleware, static assets, and JS interop inventoried from
`Controllers/`, `Hubs/`, `Middleware/`, `wwwroot/`, and the `Program.cs` pipeline.
File counts verified with `rg` (130 `.razor` files total; 63 with `@page`). No files
in either repository were modified during research.

## Hosting model summary

Single ASP.NET Core project `src/Melodee.Blazor` (.NET 10): Blazor Server with
`AddInteractiveServerRenderMode` (`MapRazorComponents<App>()`), Radzen component
library, cookie authentication, global `[Authorize]` from `_Imports.razor` unless a
page declares `[AllowAnonymous]`. Pages inject domain services **directly**
(server-side); they do not call the co-hosted REST controllers for page data (no
`HttpClient` usage in `Melodee.Blazor/Services/`). The same process also hosts the
OpenSubsonic, Jellyfin, and native Melodee v1 API controllers, a SignalR party hub,
Quartz jobs, and the Scalar/OpenAPI docs UI.

Auth notation used below: **Admin** = `Authorize(Roles="Administrator")`;
**Editor+** = `Authorize(Roles="Administrator,Editor")`; **User** = any authenticated
user (global default); **Anon** = `[AllowAnonymous]`. "User" pages frequently gate
destructive buttons with in-markup `CurrentUser?.IsAdmin()/IsEditor()` visibility
checks; the table records the attribute-level truth (see Uncertainties §2).

## Page inventory (63 pages, 68 routes)

All files are under `src/Melodee.Blazor/Components/Pages/`. The legacy
`Pages/README.md` defines the folder semantics: `Data/` = processed database records,
`Media/` = staging/inbound `melodee.json` processing, `Admin/` = administrative
tools, `Account/` = account management.

| Route | File | Purpose | Auth | Capability |
|---|---|---|---|---|
| `/` | `Dashboard.razor` | Landing dashboard: statistics, recent activity, admin health warnings | User (admin blocks gated) | Overview |
| `/about` | `About.razor` | Version/build information | User | Server info |
| `/stats` | `Stats.razor` | Library and listening statistics | User (editor/admin blocks gated) | Reporting |
| `/search`, `/search/{Query}` | `Search.razor` | Global search across artists/albums/songs (MQL-backed) | User | Search |
| `/share/{ShareUniqueId}` | `Shared.razor` | Public view of a shared item | Anon | Share consumption |
| `/user/{UserApiKey:guid}` | `UserDetail.razor` | User profile view (pins, stats, role badges) | User | User profile |
| `/usergroup/{ApiKey:guid}` | `UserGroupDetail.razor` | User-group detail view (breadcrumbs from `/admin/usergroups`) | User | User group detail |
| `/musicplayer` | `MusicPlayer.razor` | HTML5 audio player with scrobbling; also opened as a dialog from `MelodeeComponentBase` | User | In-browser playback |
| `/jukebox` | `Jukebox.razor` | Server-side jukebox playback control (OpenSubsonic jukebox backend); explicit "not enabled" empty state | User (admin controls gated) | Jukebox control |
| `/party` | `PartyMode.razor` | Party-mode session list/create; explicit "not enabled" empty state | User | Party sessions |
| `/party/{SessionApiKey:guid}` | `PartySession.razor` | Live party session (queue/player over SignalR `/party-hub`) | User | Real-time collaborative playback |
| `/activity/nowplaying` | `Activity/NowPlaying.razor` | Now-playing panel across users | User | Activity monitoring |
| `/Error` | `Error.razor` | Generic error page (`UseExceptionHandler`/`UseStatusCodePages` target) | Anon (effectively) | Error display |
| `/account/login`, `/login` | `Account/Login.razor` | Login incl. Google OAuth via `/api/v1/auth/google` | Anon | Authentication |
| `/account/register` | `Account/Register.razor` | Self-registration | Anon | Authentication |
| `/account/logout` | `Account/Logout.razor` | Logout | Anon | Authentication |
| `/account/forgot-password` | `Account/ForgotPassword.razor` | Password reset request | Anon | Authentication |
| `/account/reset-password` | `Account/ResetPassword.razor` | Password reset with token | Anon | Authentication |
| `/account/profile` | `Account/Profile.razor` | Own profile edit, avatar, device-profile dialog, Google account link/unlink via native API | User | Account self-service |
| `/onboarding` | `Onboarding/Index.razor` | First-run setup wizard (admin, paths, branding, security, settings, verify steps) | User (see Uncertainties §1) | Initial setup |
| `/onboarding/blocking` | `Onboarding/Blocking.razor` | Fallback page when setup status cannot be determined | User (see Uncertainties §1) | Setup guard |
| `/media/library/{ApiKey:guid}` | `Media/Library.razor` | Staging/inbound library browser (file-based `melodee.json` models) | Editor+ | Inbound staging review |
| `/media/album/{LibraryApiKey:guid}/{ApiKey:guid}` | `Media/AlbumDetail.razor` | Staging album detail/validation before import | Editor+ | Tag validation, import prep |
| `/media/album/{LibraryApiKey:guid}/{ApiKey:guid}/edit` | `Media/AlbumEdit.razor` | Edit staging album metadata (`melodee.json`) | Editor+ | Tag editing |
| `/media/artist/{ArtistId:int}/edit` | `Media/ArtistEdit.razor` | Edit staging artist metadata | User | Tag editing |
| `/admin/mediaartists` | `Media/Artists.razor` | Artist search-engine database management (add/export/merge candidates) | User (editor buttons gated) | Search-engine artist mgmt |
| `/data/artists` | `Data/Artists.razor` | Database artist grid | User (editor actions gated) | Library browsing |
| `/data/artist/{ApiKey:guid}/{FromUrl?}` | `Data/ArtistDetail.razor` | Artist detail: albums, images, relationships, lock/unlock, rescan, validate, JSON export | User (editor/admin menu) | Library browsing + curation |
| `/data/artist/{ApiKey:guid}/edit` | `Data/ArtistEdit.razor` | Edit database artist | Editor+ | Metadata editing |
| `/data/albums` | `Data/Albums.razor` | Database album grid | User (editor actions gated) | Library browsing |
| `/data/album/{ApiKey:guid}/{FromUrl?}` | `Data/AlbumDetail.razor` | Album detail: songs, images, jukebox enqueue, lock/rescan/delete, set cover | User (editor/admin menu) | Library browsing + curation |
| `/data/album/{ApiKey:guid}/edit` | `Data/AlbumEdit.razor` | Edit database album | Editor+ | Metadata editing |
| `/data/songs/{Filter?}` | `Data/Songs.razor` | Database song grid with optional filter segment | User | Library browsing |
| `/data/song/{ApiKey:guid}` | `Data/SongDetail.razor` | Song detail | User | Library browsing |
| `/data/playlists` | `Data/Playlists.razor` | Playlist list (incl. dynamic/smart), import dialogs | User | Playlist management |
| `/data/playlist/{ApiKey:guid}` | `Data/PlaylistDetail.razor` | Playlist detail/edit, cover image, lock | User (editor menu) | Playlist management |
| `/data/shares` | `Data/Shares.razor` | Share management | User | Share management |
| `/data/users` | `Data/Users.razor` | User account grid | User (editor actions gated) | User management |
| `/data/useredit/{ApiKey:guid}`, `/data/useredit/new` | `Data/UserEdit.razor` | Create/edit user account | User (effectively admin via UI gating) | User management |
| `/data/libraries` | `Data/Libraries.razor` | Library list (inbound/staging/storage) | User | Library management |
| `/data/library/{ApiKey:guid}` | `Data/LibraryDetail.razor` | Library detail, scan status, trigger scan | User | Library scan trigger |
| `/data/libraryedit/{ApiKey:guid}` | `Data/LibraryEdit.razor` | Create/edit library definition | Admin | Library management |
| `/requests` | `Data/Requests.razor` | Request list | User | Requests |
| `/requests/new` | `Data/RequestNew.razor` | Create request | User | Requests |
| `/requests/{ApiKey:guid}` | `Data/RequestDetail.razor` | Request detail + comments | User | Requests |
| `/requests/{ApiKey:guid}/edit` | `Data/RequestEdit.razor` | Edit/fulfill request | User | Requests |
| `/data/radiostations` | `Data/RadioStations.razor` | Internet radio station management | User | Radio management |
| `/data/podcasts` | `Data/Podcasts.razor` | Podcast channel list, add/discover/OPML import dialogs | User | Podcast management |
| `/data/podcasts/{ChannelApiKey:guid}` | `Data/PodcastDetail.razor` | Channel detail, episodes, channel settings dialog | User | Podcast management |
| `/data/podcasts/search` | `Data/PodcastEpisodeSearch.razor` | Search podcast episodes | User | Podcast search |
| `/charts` | `Data/Charts.razor` | Chart list (visibility/multi-select for admins) | User (admin controls gated) | Charts |
| `/charts/{ApiKey:guid}` | `Admin/ChartDetail.razor` | Chart detail with inline admin edit controls | User (admin controls gated) | Charts |
| `/charts/create`, `/charts/edit/{ApiKey:guid}` | `Admin/ChartEditor.razor` | Create/edit charts; chart image via `/api/v1/charts/{key}/image` | Admin | Chart editing |
| `/charts/missing-report`, `/charts/missing-report/{ChartApiKey:guid}` | `Data/ChartsMissingReport.razor` | Chart entries missing from the library | Admin | Reporting |
| `/charts/ranked-report` | `Data/ChartsRankedReport.razor` | Ranked chart report | User | Reporting |
| `/admin/dashboard` | `Admin/Dashboard.razor` | Admin operations dashboard | Admin | Admin overview |
| `/admin/settings` | `Admin/Settings.razor` | Full settings registry editing (add/detail dialogs) | Admin | Settings editing |
| `/admin/scripts` | `Admin/Scripts.razor` | Script/event hook list | Admin | Scripting |
| `/admin/scripts/{EventName}` | `Admin/ScriptEditor.razor` | Edit script for an event (Monaco editor, Jint runtime) | Admin | Script editing |
| `/admin/themes` | `Admin/Themes.razor` | Theme pack management and import | Admin | Theme management |
| `/admin/jobs` | `Admin/Jobs.razor` | Quartz job list/history, manual triggers | Admin | Job management |
| `/admin/usergroups` | `Admin/UserGroups.razor` | User group list + editor panel | Admin | User group management |
| `/admin/doctor` | `Admin/Doctor.razor` | Diagnostics/health "doctor" checks | Admin | Diagnostics |

## Non-page components (67 total) — summary

Counted at category level only, per WP-14 scope:

- **`Components/Components/` (29):** reusable UI — data cards (album/artist/song and
  compact variants), `UserPinCardComponent`, `TagsInput`, `DebounceInput`,
  `MonacoEditor` + `CodeEditor` (JS-interop code editors), `Markdown`,
  `ImageSearchUpload`, `IdentifyAlbum`, `AddArtistRelationshipDialog`,
  `DictionaryModal`, `AdvancedTimer`, `CondensedStatistic`, `ThemeSelector`,
  `LanguageSelector`, `ThirdPartyLink`, the party trio (`PartyPlayer`, `PartyQueue`,
  `PartySessionEndpointSelector`), and 7 MQL helper components under
  `Components/Mql/` (search input, syntax guide, field/error references, error
  display, field table, error card).
- **`Components/Dialogs/` (7):** settings dialogs (`AddSettingDialog`,
  `SettingDetailDialog`), `ArtistLookupDialog`, `BulkArtistManagementDialog`,
  `DecentDbMigrationDialog`, `DeviceProfileDialog`, `ScriptOverrideBodyDialog`.
- **`Components/Onboarding/` (10):** wizard step components (`OnboardingWelcome`,
  `OnboardingExplain`, `OnboardingAdmin`, `OnboardingPaths`, `OnboardingBranding`,
  `OnboardingSecurity`, `OnboardingSettings`, `OnboardingVerify`) plus
  `OnboardingGuard` and `OnboardingRedirect`.
- **`Components/Layout/` (2):** `MainLayout`, `CheckAuthorization`.
- **Dialog/editor components living inside `Pages/` folders (13):** Admin
  (`ChartImportDialog`, `ChartItemResolveDialog`, `MissingAlbumIdentifyDialog`,
  `ThemeImportDialog`, `UserGroupEditor` — no `@page`, it is a panel component), Data
  (`AddPodcastChannelDialog`, `ArtistValidationDialog`, `DynamicPlaylistImportDialog`,
  `PlaylistImportDialog`, `PodcastChannelSettingsDialog`, `PodcastDiscoveryDialog`,
  `PodcastOpmlDialog`), Media (`UploadAlbumDialog`).
- **Infrastructure (5):** `App.razor`, `Routes.razor`, `_Imports.razor` (applies the
  global `[Authorize]`), `CustomBlock.razor`, and
  `Pages/MelodeeComponentBase.razor` (base class all pages inherit; also opens the
  MusicPlayer dialog).
- **`Components/Shared/` (1):** `OptimizedImage.razor`.

Totals: 130 `.razor` files = 63 pages + 67 non-page components (verified with `rg`).

## Controllers vs. Blazor UI

The Blazor UI is server-side and injects domain services directly; the co-hosted REST
controllers are the three **public** API surfaces, not page backends. Controller file
counts (including base classes/parsers, excluding `Models/`/`Extensions/`
subfolders): OpenSubsonic 18 (`/rest/*`), Jellyfin 15 (paths rewritten by
`JellyfinRoutingMiddleware`), native Melodee v1 29 (`/api/v1/*`), plus
`ThemesController` and `CommonBase`. A `Controllers/README.md` enforces DTO-only
responses (never EF entities).

API endpoints the Blazor UI **does** consume (all shared public API, none
Blazor-exclusive):

- `AuthController` — `POST /api/v1/auth/google` from the Login page.
- `UsersController` — `POST`/`DELETE /api/v1/Users/me/link/google` from the Profile
  page.
- `ChartsController` — `GET /api/v1/charts/{apiKey}/image` from ChartEditor.
- `SongsController` — `GET /song/stream/{apiKey}/{userApiKey}/{authToken}` used by the
  MusicPlayer page for audio playback.
- Image URLs of the form `/images/{apiKey}/{size}` are rendered throughout the pages
  (served by the shared image endpoint set; exact owning controller not pinned down —
  see Uncertainties §3).
- **SignalR:** `Hubs/PartyHub.cs` mapped at `/party-hub` (with
  `Hubs/PartyNotificationService.cs`) — Blazor-specific real-time infrastructure for
  Party mode; the only hub in the app.
- **`ThemesController`** (`/api/v1/themes`, anonymous GET) — the closest thing to a
  Blazor-serving controller: it feeds theme packs to `ThemeSelector` /
  `ThemeClientService`, but it sits under the public versioned API.

Blazor-specific support code that is not controllers: `Services/AuthService` +
`CustomAuthStateProvider` (cookie auth state), `LocalStorageService` /
`Security/CookieStorageAccessor` (JS interop), `MainLayoutProxyService`,
`OnboardingStateService`, `PartyModeService`, `ThemeClientService`, `DoctorService`,
`MelodeeCircuitHandler`, `LocalizationService`, `Services/Email/`,
`Services/CustomBlocks/`, and `Middleware/` (Jellyfin routing, cookie, security
headers, correlation ID).

## Static assets, JS interop, localization

- **`wwwroot/`:** `app.css`, `favicon.ico`, `robots.txt`; `images/` (placeholders:
  album/artist/user/playlist/chart/release, missing/not-found images, logo);
  `icons/` (third-party service icons: Spotify, Last.fm, MusicBrainz, Discogs,
  Deezer, iTunes, AMG, Wikidata). Static files served with 1-year `Cache-Control` plus
  a last-modified ETag (`Program.cs`).
- **`wwwroot/js/` (10 interop modules):** `auth.js`, `googleAuth.js` (Google
  sign-in), `CookieStorageAccessor.js`, `musicPlayer.js` (audio element control with
  scrobble callbacks), `podcastPlayer.js`, `monacoEditor.js` + `codeEditor.js`
  (script editing), `FileDropZone.js`, `downloadFile.js`, `themeManager.js`.
- **Localization:** 10 cultures (`en-US` default, `de-DE`, `es-ES`, `fr-FR`, `it-IT`,
  `ja-JP`, `pt-BR`, `ru-RU`, `zh-CN`, `ar-SA`) via cookie request-culture provider;
  `IStringLocalizer<SharedResources>` / `L()` used pervasively in pages.
- **Pipeline:** Serilog request logging, CORS (`MelodeeCors`), rate limiter,
  antiforgery, cookie policy (Strict SameSite; Lax for `/scalar`, `/openapi`),
  security headers middleware, correlation ID logging, `UseStatusCodePages`
  redirecting non-API requests to `/Error`, health checks at `/health`, OpenAPI +
  Scalar docs. Quartz jobs scheduled at startup from database configuration (library
  inbound process/insert, staging auto-move/revalidation, podcast
  refresh/download/cleanup/recovery, chart update, artist housekeeping, now-playing
  cleanup) — surfaced by the Admin/Jobs page.

## Dead / broken / experimental candidates

- **`Pages/MusicPlayer.razor` (`/musicplayer`)** — the standalone route appears
  unlinked from navigation; the component is primarily opened as a dialog via
  `MelodeeComponentBase` (`DialogService.OpenAsync<MusicPlayer>`). The routable page
  is effectively vestigial; the dialog usage is live.
- **`Pages/Error.razor` (`/Error`)** — stock ASP.NET template content, including the
  boilerplate "Development Mode" warning text; functional but uncustomized.
- **`Media/Artists.razor` (`/admin/mediaartists`)** — route says `/admin/*` but has
  no role attribute; only hides buttons for non-editors. Inconsistent auth posture
  vs. the real Admin pages.
- **`UserGroupDetail.razor` (`/usergroup/{key}`)** — routable with only default auth,
  yet it is the detail view for the admin-only `/admin/usergroups`. Auth
  inconsistency.
- **`Data/Users.razor` + `Data/UserEdit.razor` (`/data/users`,
  `/data/useredit/*`)** — user-management pages with only default `[Authorize]` at
  the attribute level; mutation controls hidden for non-editors, relying on
  service-layer checks rather than route authorization.
- **Party/Jukebox pages** (`/party`, `/party/{key}`, `/jukebox`) — render explicit
  "not enabled" empty states when the feature is disabled in settings:
  dead-by-configuration rather than broken. Capability itself waived for Phase 9 by
  ADR-0133.

## Uncertainties

1. **Onboarding auth:** `/onboarding` and `/onboarding/blocking` inherit the global
   `[Authorize]`, yet onboarding must run before an admin exists; access is
   presumably mediated by the `OnboardingGuard`/`OnboardingRedirect` components,
   `OnboardingStateService`, and `Layout/CheckAuthorization.razor` — that flow was
   not fully traced.
2. **Service-layer authorization:** several pages (Users, UserEdit, ChartDetail,
   PartySession) rely on injected-service permission checks that were not audited;
   the "User" auth level in the table is the attribute-level truth, not necessarily
   the effective one.
3. **`/images/*` ownership:** pages reference `/images/{apiKey}/{size}` throughout,
   but the exact controller owning the route was not pinned down (likely shared
   between native and OpenSubsonic image handling).
4. **Purpose inference:** for a few pages (Requests workflow semantics, Charts
   ranked/missing reports, Doctor checks), purposes are inferred from names and
   headers rather than full content reads.
5. **Counts:** 63 page files / 68 routes / 67 non-page `.razor` files (130 total)
   verified with `rg`; controller counts (18 OpenSubsonic, 15 Jellyfin, 29 native,
   1 Themes) are file counts including base classes and parsers, excluding
   `Models/`/`Extensions/` subfolders.

## Preliminary classification (PRD-WEB-002 input)

**This classification is a recommendation pending product decision.** It is an input
to PRD-WEB-002 and the Phase 10 planning ADR, not an action. **No route is removed,
changed, or added in Phase 9.** Any DROP requires its own accepted ADR plus the
4-step contract-change process (compatibility test, product decision, ADR,
migration/client plan) per `AGENTS.md` before any removal work starts.

- **KEEP** — capability must exist in the Phase 10 Rust admin UI.
- **REPLACE** — capability must exist but must be redesigned; the current
  implementation embodies server-side-Blazor or legacy-runtime patterns with no Rust
  analog.
- **DROP** — candidate removal; requires its own ADR before any route removal.

| Page (route) | Classification | Rationale |
|---|---|---|
| `/` Dashboard | KEEP | Core landing/overview capability. |
| `/about` | KEEP | Version/build info; trivial. |
| `/stats` | KEEP | Statistics reporting. |
| `/search`, `/search/{Query}` | KEEP | Global/MQL search UI over existing native search API. |
| `/share/{ShareUniqueId}` | KEEP | Public share view; tied to preserved share contracts. |
| `/user/{UserApiKey}` | KEEP | User profile view. |
| `/usergroup/{ApiKey}` | KEEP | User group detail; fix the missing role attribute in the redesign. |
| `/musicplayer` | REPLACE | Keep in-browser preview playback + scrobble capability; drop the vestigial standalone route, rebuild as embedded player. |
| `/jukebox` | DROP-candidate | Capability waived by ADR-0133; removal needs its own ADR. |
| `/party` | DROP-candidate | Capability waived by ADR-0133; removal needs its own ADR. |
| `/party/{SessionApiKey}` | DROP-candidate | Capability waived by ADR-0133; also retires the only SignalR hub. |
| `/activity/nowplaying` | KEEP | Activity monitoring. |
| `/Error` | REPLACE | Keep the capability; replace stock template page with a localized, themed error page. |
| `/account/login`, `/login` | KEEP | Authentication entry point. |
| `/account/register` | KEEP | Self-registration. |
| `/account/logout` | KEEP | Session termination. |
| `/account/forgot-password` | KEEP | Password reset request. |
| `/account/reset-password` | KEEP | Password reset. |
| `/account/profile` | KEEP | Account self-service; device-profile editing moves to the admin surface per ADR-0134 §5. |
| `/onboarding` | REPLACE | WP-08 onboarding is API/CLI-first; the Blazor wizard has no direct Rust analog. |
| `/onboarding/blocking` | REPLACE | Same as onboarding; guard states expressed via API/CLI. |
| `/media/library/{ApiKey}` | KEEP | Inbound staging review. |
| `/media/album/.../{ApiKey}` (detail) | KEEP | Staging album validation. |
| `/media/album/.../edit` | KEEP | Staging tag editing. |
| `/media/artist/{ArtistId}/edit` | KEEP | Staging artist editing; note int route param vs. Guid elsewhere. |
| `/admin/mediaartists` | KEEP | Search-engine artist management; fix the route/auth mismatch in the redesign. |
| `/data/artists` | KEEP | Library browsing. |
| `/data/artist/{ApiKey}/{FromUrl?}` | KEEP | Artist detail + curation. |
| `/data/artist/{ApiKey}/edit` | KEEP | Artist editing. |
| `/data/albums` | KEEP | Library browsing. |
| `/data/album/{ApiKey}/{FromUrl?}` | KEEP | Album detail + curation; jukebox-enqueue menu item follows the jukebox disposition. |
| `/data/album/{ApiKey}/edit` | KEEP | Album editing. |
| `/data/songs/{Filter?}` | KEEP | Library browsing. |
| `/data/song/{ApiKey}` | KEEP | Song detail. |
| `/data/playlists` | KEEP | Playlist management incl. dynamic playlists. |
| `/data/playlist/{ApiKey}` | KEEP | Playlist detail/edit. |
| `/data/shares` | KEEP | Share management. |
| `/data/users` | KEEP | User management; enforce admin route auth in the redesign. |
| `/data/useredit/{ApiKey}`, `/data/useredit/new` | KEEP | User create/edit; enforce admin route auth in the redesign. |
| `/data/libraries` | KEEP | Library management. |
| `/data/library/{ApiKey}` | KEEP | Library detail + scan trigger. |
| `/data/libraryedit/{ApiKey}` | KEEP | Library create/edit. |
| `/requests` | KEEP | Requests workflow. |
| `/requests/new` | KEEP | Requests workflow. |
| `/requests/{ApiKey}` | KEEP | Requests workflow. |
| `/requests/{ApiKey}/edit` | KEEP | Requests workflow. |
| `/data/radiostations` | KEEP | Radio station management. |
| `/data/podcasts` | KEEP | Podcast management. |
| `/data/podcasts/{ChannelApiKey}` | KEEP | Podcast channel/episodes. |
| `/data/podcasts/search` | KEEP | Podcast episode search. |
| `/charts` | KEEP | Charts. |
| `/charts/{ApiKey}` (ChartDetail) | KEEP | Charts; split admin controls cleanly in the redesign. |
| `/charts/create`, `/charts/edit/{ApiKey}` | KEEP | Chart editing. |
| `/charts/missing-report[...]` | KEEP | Reporting. |
| `/charts/ranked-report` | KEEP | Reporting. |
| `/admin/dashboard` | KEEP | Admin overview. |
| `/admin/settings` | KEEP | Settings registry editing. |
| `/admin/scripts` | REPLACE | WP-11 scripting runs on the Boa runtime (ADR-0135) with no Monaco/Jint analog; script management redesigned API-first. |
| `/admin/scripts/{EventName}` (ScriptEditor) | REPLACE | Same as Scripts; editor surface redesigned for the Boa runtime. |
| `/admin/themes` | KEEP | Theme management; confirmed Phase 10 scope per WP-14 (THEME/I18N/RTL rows out of Phase 9's exit gate). |
| `/admin/jobs` | KEEP | Job management/triggers. |
| `/admin/usergroups` | KEEP | User group management. |
| `/admin/doctor` | KEEP | Diagnostics. |

Disposition totals: **KEEP 54, REPLACE 6, DROP-candidate 3** (63 pages). Component-level
follow-ons implied by the classifications (not separately classified here): the party
components (`PartyPlayer`, `PartyQueue`, `PartySessionEndpointSelector`), `PartyHub`,
and `PartyModeService` follow the party DROP decision; `MonacoEditor`/`CodeEditor`
and the `monacoEditor.js`/`codeEditor.js` interop follow the script-editor REPLACE
decision; the `Components/Onboarding/` step components follow the onboarding REPLACE
decision; `MusicPlayer` dialog support in `MelodeeComponentBase` follows the player
REPLACE decision.
