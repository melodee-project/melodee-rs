# Phase 9 WP-11 — Script event/hook legacy inventory (PRD-SCRIPT-002)

- **Purpose:** satisfy PRD-SCRIPT-002 — inventory all existing legacy script
  event names, input objects, expected return values, side effects, and
  failure semantics **before** implementing the Rust executor.
- **Date:** 2026-08-14.
- **Legacy oracle:** `/home/steven/src/github/melodee` at frozen commit
  `4028b58dbd3302ed431e2c5b32d9dde5634db218` (verified 2026-08-14 against
  `design/evidence/phase-0/legacy-baseline.md`).
- **Method:** static inspection of the frozen tree (source grep for
  `script`/`hook`/`plugin`/`IScript`/`Jint` terms excluding `bin`/`obj`/
  `node_modules`), cross-checked against legacy docs
  (`docs/pages/scripting.md`) and legacy design docs
  (`design/requirements/event-scripting-requirements.md`,
  `design/requirements/event-scripting-implementation.md`). Every behavioral
  claim below carries a `file:line` citation into the frozen tree.
- **Governing authorities for the Rust implementation:** PRD-SCRIPT-001..005,
  ADR-0134 (Phase 9 parity-domain schema: `script_registry` / `script_runs`),
  ADR-0135 (Boa runtime, capability-less sandbox, fail-degraded semantics).
  Ground-up rewrite rule applies: the C# code below is behavioral evidence,
  not an architecture template.

## 0. Two distinct scripting subsystems exist in legacy

The frozen tree contains **two unrelated scripting mechanisms**:

1. **Event Scripting (Jint / JavaScript)** — a self-described "preview
   feature in 2.2.0" (`docs/pages/scripting.md:13`). Administrator-authored
   JavaScript stored as JSON in the Settings table under keys
   `script.{eventName}`, evaluated in-process by Jint. Admin CRUD surface is
   Blazor-only (Admin → Scripts; `Scripts.razor`, `ScriptEditor.razor`,
   `ScriptAdminService.cs`). This is the subsystem PRD-SCRIPT-001..004
   target.
2. **Legacy shell-script plugins (Pre/PostDiscoveryScript)** — older
   bash-script hooks in the inbound directory pipeline, configured via
   `scripting.*` settings, executed with full host privileges
   (`src/Melodee.Common/Plugins/Scripting/`). No sandbox of any kind.

There is **no scripting ADR** in the legacy repo (`design/adr/` holds 7
ADRs, none about scripting). Scripting design intent lives only in the two
`design/requirements/event-scripting-*.md` documents cited above.

## 1. Hook inventory

### 1a. Jint event-scripting hooks

Registered event names: `src/Melodee.Common/Models/Scripting/ScriptEventNames.cs:3-27`
(eight names in `ScriptEventNames.All`).

**Shared contract** (applies to every event in this table):

- The host invokes `check(ctx, scriptConfig)`. A script body without a
  `function check` declaration is auto-wrapped as
  `function check(ctx, scriptConfig) { return (<body>); }`
  (`ScriptEvaluationService.cs:152-161`; identical copy in
  `ScriptValidationService.cs:129-138`).
- Accepted returns: `true` (allow), `false` (deny), or an object
  `{ result: bool, message?: string }` (`ScriptEvaluationService.cs:89-126`;
  documented in `docs/pages/scripting.md:51-58`).
- **Fail-open:** missing setting, disabled config, empty body, parse error,
  runtime exception, timeout, statement-limit trip, or a non-boolean /
  non-conforming return all behave as if the script returned `true`
  (`ScriptEvaluationService.cs:38-56,128-149`;
  `ScriptOrchestrationService.cs:39-73,98-115`;
  `docs/pages/scripting.md:23`). The `IsDefault` flag on
  `ScriptEvaluationResult` distinguishes a real script verdict from a
  fail-open default (`ScriptEvaluationResult.cs:3-22`).
- `ctx` properties are exposed camelCase; `scriptConfig` contains
  `eventName`, `settingKey`, `timeoutMs`, `maxStatements`, `onDeny`
  (`ScriptOrchestrationService.cs:77-84`; the editor/validation path passes
  a smaller object without `settingKey`/`timeoutMs`/`maxStatements` —
  `ScriptValidationService.cs:75-80`, `ScriptEditor.razor:536-540`).

| Event | Lifecycle point (when it fires) | Input `ctx` payload | Deny (`false`, non-default) effect | Evidence |
|---|---|---|---|---|
| `directoryProcessingStart` | Inbound→staging pipeline, once per stable candidate directory, before plugin processing | `DirectoryProcessingContext`: `path`, `directoryName`, `totalFilesCount`, `totalSizeMegabytes`, `mostRecentModified` (ISO-8601 "O"), `mediaFilesCount`, `totalDurationMinutes`, `trackNumbers` (int[], sorted), `hasTrackNumberGaps`; built by reading real tags through song plugins | Host applies configured `onDeny`: `skip` (leave dir, skip processing — default) or `quarantine` (move to `script.quarantine.path`); `delete` is **deliberately rewritten to `skip`** in the ingestion path. The **only server-side enforcement hook**. | `ScriptEventNames.cs:5`; `DirectoryProcessorToStagingService.cs:637-645,1701-1748`; `DirectoryProcessingContext.cs:3-25`; `DirectoryContextProvider.cs:26-108`; `DenyActionHandler.cs` |
| `directoryProcessingDelete` | **Registered but never evaluated.** Requirements doc intended it to run after `directoryProcessingStart` returned false with `onDeny: delete`; the pipeline never calls it and `delete` is converted to `skip` regardless. Exercised only by mock-based unit tests. | Same `DirectoryProcessingContext` shape (per docs/editor mock) | None in production | `ScriptEventNames.cs:6,19`; zero invocation sites in `src/`; `event-scripting-requirements.md:70-75,256-259`; `docs/pages/scripting.md:139`; `tests/Melodee.Tests.Common/Services/ScriptEvaluation/DirectoryDeletionScriptTests.cs:378-495` |
| `userRegistrationStart` | Blazor `/register` page **initialization** (before any form submit) | `UserRegistrationContext`: `userNameLength` (**always 0**), `emailDomain` (**always ""**), `clientIp`, `userAgent`, `now` | Sets `_isRegistrationDisabled = true` and shows `message`. **UI gate only** — no service/API enforcement. | `BlazorEventScriptService.cs:29-35`; `Register.razor:271-290`; `BlazorEventContexts.cs:3-10` |
| `userLoginStart` | Blazor `/login` page initialization | `UserLoginContext`: `userId` (**always null**), `roles` (**always []**), `clientIp`, `userAgent`, `now` | Disables login form with message. UI gate only. | `BlazorEventScriptService.cs:37-43`; `Login.razor:405-417`; `BlazorEventContexts.cs:12-19` |
| `userProfileUpdateStart` | Blazor `/profile` page initialization | `UserProfileUpdateContext`: `userId` (real), `emailDomain` (real), `profileChangesCount` (**always 0**), `clientIp`, `userAgent`, `now` | Makes profile page read-only with message. UI gate only. | `BlazorEventScriptService.cs:45-51`; `Profile.razor:367-379`; `BlazorEventContexts.cs:21-29` |
| `playlistCreateStart` | Blazor `/playlists` page initialization | `PlaylistCreateContext`: `userId` (real), `nameLength` (**0**), `initialSongCount` (**0**), `now` | Disables playlist-import buttons. UI gate only. Note: checks `!result.Result` **without** the `!IsDefault` guard used by the account pages. | `BlazorEventScriptService.cs:53-59`; `Playlists.razor:172-184`; `BlazorEventContexts.cs:31-37` |
| `podcastChannelAddStart` | Blazor `/podcasts` page initialization | `PodcastChannelAddContext`: `userId` (real), `feedUrl` (**""**), `isNewSubscription` (**true**), `now` | Disables **Add Channel** button. UI gate only; also missing the `IsDefault` guard. | `BlazorEventScriptService.cs:61-67`; `Podcasts.razor:179-191`; `BlazorEventContexts.cs:39-45` |
| `requestCreateStart` | Blazor `/requests` page initialization | `RequestCreateContext`: `userId` (real), `requestType` (**""**), `isFirstRequestToday` (**true**), `dailyRequestCount` (**0**), `now` | Disables **New Request** button. UI gate only; also missing the `IsDefault` guard. | `BlazorEventScriptService.cs:69-75`; `Requests.razor:211-223`; `BlazorEventContexts.cs:56-63` |
| `shareCreateStart` | **Spec-only.** Listed in the requirements doc as event #8 but never registered in `ScriptEventNames`, never serviced by `IBlazorEventScriptService`, never invoked by any page. Only an orphaned `ShareCreateContext` record exists. | (spec: `userId`, `shareType`, `expiresInDays`, `now`) | n/a | `event-scripting-requirements.md:92-93,400-409`; `BlazorEventContexts.cs:47-54` (dead record) |

### 1b. Legacy shell-script plugin hooks

| Hook | Lifecycle point | Input | Output / effect | Evidence |
|---|---|---|---|---|
| `PreDiscoveryScript` | Start of inbound `ProcessDirectoryAsync`, after the staging-directory existence check, before any directory discovery | Bash script path from `scripting.preDiscoveryScript`; invoked as `bash {script} -d "<directoryPath>" -r {0\|1}` where `-r` is the inverse of `processing.doDeleteOriginal` | Script exception/non-success → `OperationResult` failure that **aborts the entire processing run** (fail-closed — the opposite of the Jint fail-open policy). No return-value contract; arbitrary side effects. | `PreDiscoveryScript.cs:23-66`; `DirectoryProcessorToStagingService.cs:367-390` |
| `PostDiscoveryScript` | End of `ProcessDirectoryAsync`, after processing (also reached after graceful cancellation) | Same invocation shape via `scripting.postDiscoveryScript` | Failure recorded as an error result; arbitrary side effects. | `PostDiscoveryScript.cs:23-66`; `DirectoryProcessorToStagingService.cs:495-514` |

Both hooks are gated by `scripting.enabled` (seeded `false`) plus a
non-empty script path (both seeded `""`); `NullScript` is the placeholder
when unconfigured (`MelodeeDbContext.cs:581-607`; `NullScript.cs:8-24`;
`SettingRegistry.cs:125-127`). `IScriptPlugin.StopProcessing` exists on the
interface but is `false` in every implementation and never consulted
(`IScriptPlugin.cs:7`; `PreDiscoveryScript.cs:21`; `PostDiscoveryScript.cs:21`).

## 2. Runtime and sandboxing

### Jint event scripting

- **Runtime:** Jint **4.12.0** (`Directory.Packages.props:34`; referenced by
  `src/Melodee.Common/Melodee.Common.csproj:36`). The stored `engine` field
  in the config JSON is recorded but ignored — execution always uses Jint
  (`docs/pages/scripting.md:96`).
- **Limits:** strict mode on; `TimeoutInterval` default **50 ms**;
  `MaxStatements` default **10,000**; both configurable per event via the
  stored config (`ScriptConfig.cs:20-24`; `ScriptEvaluationService.cs:68-73`).
  Constraints are reset after host-side setup so the limits apply only to the
  `check()` invocation (`ScriptEvaluationService.cs:80-84`).
- **Isolation:** no CLR access is granted (no `AllowClr`); contexts cross the
  boundary as plain dictionaries/lists/primitives via a JSON serialize →
  re-materialize round-trip (`ScriptValueConverter.cs:8-67`), so scripts
  never see live .NET objects, filesystem, or network APIs. There is **no
  memory limit and no process isolation**; the requirements doc lists
  process/AppDomain isolation only as "future hardening"
  (`event-scripting-requirements.md:185-195`). Performance target stated:
  P99 < 10 ms (`event-scripting-requirements.md:246`).
- **Caching:** prepared scripts (`Engine.PrepareScript`) cached by SHA-256
  hex of the normalized body in cache region `"scripts"` with a 5-minute TTL
  (`ScriptCacheService.cs:17-58`; `ScriptHashing.cs:8-13`). `SettingEtag` is
  computed (`ScriptConfigurationService.cs:60-66`) but never used for
  invalidation — TTL expiry is the only propagation mechanism.
- **Failure semantics:** fail-open to allow in all cases (see §1a contract).
  Failures log event name, setting key, and script SHA-256 hash, never the
  body (`ScriptOrchestrationService.cs:98-106`;
  `event-scripting-requirements.md:236-239`).
- **Deny-action safety:** the quarantine handler enforces path-traversal
  containment against `script.quarantine.path`
  (`DenyActionHandler.cs:120-129`); the safe-delete service honors
  `script.dryRun.enabled` (`DenyActionHandler.cs:58-63`). The requirements
  doc's mandated 30-day deletion **audit table has no implementation** —
  only log lines exist (`event-scripting-requirements.md:430-441`; no audit
  store found in the frozen tree).
- **Admin/test surface:** Blazor-only CRUD plus an editor "test" that runs
  the script against admin-supplied mock JSON
  (`ScriptEditor.razor:520-548`). The spec'd REST validation endpoint
  `POST /api/scripts/validate` was **never implemented** (no controller in
  the frozen tree; `event-scripting-requirements.md:466-467`).

### Shell-script plugins

- **Runtime:** `/bin/bash` via shell invocation, as the server user, with the
  full environment (`PreDiscoveryScript.cs:48`; `PostDiscoveryScript.cs:48`).
- **Sandboxing:** **none whatsoever** — arbitrary filesystem/network/process
  access. This subsystem is incompatible with the ADR-0135 capability-less
  sandbox by construction.
- **Failure semantics:** fail-closed for `PreDiscoveryScript` (abort the
  processing run), error-recorded for `PostDiscoveryScript`.

## 3. Configuration keys

| Key | Meaning | Evidence |
|---|---|---|
| `script.{eventName}` | Settings-table key holding the entire `ScriptConfig` JSON document (`version`, `enabled`, `engine`, `timeoutMs`, `maxStatements`, `default{enabled,body,onDeny}`, `overrides[]`) | `ScriptConfigurationService.cs:17,34`; `ScriptAdminService.cs:42`; `ScriptConfig.cs:14-37` |
| `script.dryRun.enabled` | When true, the safe-delete service logs instead of deleting; does **not** suppress quarantine moves | `DenyActionHandler.cs:58-63`; `docs/pages/scripting.md:128` |
| `script.quarantine.path` | Quarantine destination root for `onDeny: quarantine` | `DenyActionHandler.cs:113-122` |
| `scripting.enabled` | Master switch for the legacy shell-script plugins (seeded `false`) | `SettingRegistry.cs:125`; `MelodeeDbContext.cs:581-588` |
| `scripting.preDiscoveryScript` | Filesystem path to the pre-discovery bash script (blank = disabled) | `SettingRegistry.cs:127`; `MelodeeDbContext.cs:590-598` |
| `scripting.postDiscoveryScript` | Filesystem path to the post-discovery bash script (blank = disabled) | `SettingRegistry.cs:126`; `MelodeeDbContext.cs:600-607` |
| `library.inboundPath.<libraryId>` | Named in the requirements doc as the deletion-containment root; **not enforced** anywhere in shipped code | `event-scripting-requirements.md:426-429` (spec only) |

## 4. Gaps, dead code, and uncertainties

1. **`directoryProcessingDelete` is a registered no-op** — declared in
   `ScriptEventNames.All`, editable in the admin UI, unit-tested via mocks,
   but never invoked by the pipeline; legacy docs admit this
   (`docs/pages/scripting.md:139`). The requirements doc's two-stage
   start→delete flow was never wired.
2. **`shareCreateStart` is spec-only** — never registered; only the orphaned
   `ShareCreateContext` record remains.
3. **Overrides are dead in the runtime.** `ScriptConfig.Overrides`
   (per-`libraryId`/`pathPrefix`) and `ScriptOverrideSelector` have zero
   production call sites; orchestration always uses the `default` body and
   hard-codes `SelectedOverrideId = null`
   (`ScriptOrchestrationService.cs:110`). Legacy docs warn users not to rely
   on overrides (`docs/pages/scripting.md:98-100`).
   `ScriptOverrideSelectorTests.cs` tests dead code.
4. **Stored-but-ignored config fields:** `engine` (always Jint) and
   `default.enabled` (never consulted) — `docs/pages/scripting.md:93-98`.
5. **UI-gate semantic inconsistency:** `Playlists.razor:180`,
   `Podcasts.razor:187`, and `Requests.razor:219` check `!result.Result`
   without the `!result.IsDefault` guard used by Register/Login/Profile.
   Behaviorally identical today (fail-open defaults return `Result=true`)
   but a latent fragility if default semantics ever change.
6. **Placeholder contexts:** all six Blazor events fire at page-init with
   mostly placeholder values (registration username/email, login
   userId/roles, playlist name, feed URL, request counts are zeroed/empty).
   Scripts cannot actually validate the data their names imply; legacy docs
   explicitly warn against writing such rules
   (`docs/pages/scripting.md:156-167`). These are cosmetic UI gates, **not**
   authorization: the native API and OpenSubsonic paths never evaluate them.
7. **`hasTrackNumberGaps` definition conflict:** the requirements doc
   requires the sequence to start at 1
   (`event-scripting-requirements.md:295-308`), but the shipped
   `DirectoryContextProvider.CalculateHasTrackNumberGaps` "allows start from
   any number" (`DirectoryContextProvider.cs:117`). The stricter helper
   `DirectoryContextHelpers.DetectTrackNumberGaps` exists but is dead code
   (no call sites outside the design doc).
8. **Spec'd features never shipped:** no `POST /api/scripts/validate`
   endpoint; no deletion audit table/retention job; no etag-based cache
   invalidation (TTL only); no `startEventResult` field in the delete
   context.
9. **Asymmetric failure semantics:** Jint hooks fail-open everywhere
   (including the catch-all at `DirectoryProcessorToStagingService.cs:1743-1747`),
   while `PreDiscoveryScript` fails closed (aborts the run). The Rust
   implementation must choose per-hook semantics deliberately (ADR-0135
   fixes fail-degraded for the Boa executor; shell hooks have no
   counterpart).
10. **Editor/real context drift:** the ScriptEditor's directory mock ctx
    includes `libraryId` and `relativePath` (`ScriptEditor.razor:556-558`)
    that the real `DirectoryProcessingContext` never provides — a script
    relying on them passes the editor test but sees `undefined` in
    production.

## 5. Legacy bugs and behaviors that must NOT be preserved

- **Do not register `directoryProcessingDelete` as a live event.** It was
  never evaluated; shipping a wired delete-on-deny event would *change*
  legacy behavior, and a destructive hook conflicts with the repository's
  data-safety rules (no destructive operation against an existing library
  outside the durable Phase 5 decision/audit/authorization contract).
- **Do not invent `shareCreateStart`.** It is spec residue, not behavior.
- **Do not replicate placeholder contexts.** Firing a hook named
  `userRegistrationStart` with `emailDomain=""` is a legacy defect the docs
  had to warn about; the Rust hooks must fire at a lifecycle point where the
  payload is real, or not claim the validation capability.
- **Do not port the dead override machinery** (`overrides`,
  `ScriptOverrideSelector`) as if it were behavior — it never ran. If
  per-library/per-path script selection is wanted, it is new design
  requiring its own ADR.
- **Do not preserve the missing `IsDefault` guard** inconsistency in the
  three data-page gates (item 5 above); fail-degraded results must be
  distinguishable from real denials uniformly.
- **Do not preserve the editor/real ctx drift** (item 10): the Rust test
  surface must validate against the exact production payload shape.
- **Do not preserve the `hasTrackNumberGaps` ambiguity silently** (item 7):
  pick one definition, record it in the hook contract, and note the
  divergence from shipped legacy behavior in the migration guide.
- **Do not preserve TTL-only propagation** as the documented behavior;
  ADR-0134's `script_registry` plus `script_runs` audit replaces both the
  Settings-JSON storage and the missing audit trail.
- **Shell-script hooks are not portable into the Boa sandbox** — see the
  migration guide (`script-hook-migration-guide.md`) for the operator
  migration path.

## 6. Test corpus reference

Legacy tests proving the contracts above (frozen tree,
`tests/Melodee.Tests.Common/Services/ScriptEvaluation/`):
`ScriptEvaluationServiceTests.cs` (return-shape and fail-open matrix),
`ScriptOrchestrationServiceTests.cs` (default/override-less orchestration,
`IsDefault` semantics), `ScriptValidationServiceTests.cs`,
`ScriptAdminServiceTests.cs` (CRUD over `script.*` settings),
`DirectoryContextProviderTests.cs` (ctx aggregation),
`SafeDeleteServiceTests.cs` (dry-run), `DirectoryDeletionScriptTests.cs`
(mock-only `directoryProcessingDelete` coverage — evidence of the no-op,
not of live behavior), `ScriptOverrideSelectorTests.cs` (dead-code tests).
