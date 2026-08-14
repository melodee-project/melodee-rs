# Phase 9 WP-11 — Script hook migration guide (PRD-SCRIPT-005)

- **Purpose:** per-hook migration disposition for every legacy scripting
  hook, as required by PRD-SCRIPT-005 and the WP-11 plan
  (`design/agents/PHASE-9-PLAN.md` §3).
- **Date:** 2026-08-14.
- **Basis:** the legacy inventory in
  `design/evidence/phase-9/script-events-legacy-inventory.md` (PRD-SCRIPT-002;
  frozen legacy commit `4028b58dbd3302ed431e2c5b32d9dde5634db218`).
- **Governing decisions:** ADR-0135 (Boa runtime; capability-less sandbox —
  no filesystem/network/process/module objects; wall-clock timeout on a
  bounded worker, default 250 ms; leaked-thread circuit breaker; fail-degraded
  semantics; `script_runs` audit) and ADR-0134 (`script_registry` /
  `script_runs` schema). Items marked **uncertain** below require a product
  decision or follow-up ADR before implementation.
- **Implementation status (2026-08-14, WP-11):** the executor, registry,
  audit, circuit breaker, admin API (`/api/admin/v1/scripts/*`, including the
  test-run endpoint that replaces the legacy editor "test" button), CLI
  (`melodee scripts list|test`), and the `directoryProcessingStart` pipeline
  gate are implemented per this guide; the disposition table's
  `directoryProcessingStart` row and §4 record the decisions the
  implementation made where this guide was previously uncertain. The five
  other "migrated-with-changes" UI-gate events remain unwired (accepted in
  the registry, marked inert) pending their server-side boundaries.

## 1. Contract translation rules (apply to every migrated hook)

| Legacy (Jint) | Rust (Boa, per ADR-0135) |
|---|---|
| `check(ctx, scriptConfig)` entry point; bare expressions auto-wrapped into `function check(ctx, scriptConfig) { return (<body>); }` | Same entry point and same auto-wrap, reimplemented in the Rust executor. The wrap rule is user-visible contract (legacy docs teach it) and must be preserved. |
| Returns `true` / `false` / `{ result: bool, message?: string }` | Identical return contract; non-conforming returns are fail-degraded to the hook's documented default and recorded in `script_runs`. |
| camelCase `ctx` properties from JSON-round-tripped C# records | Identical camelCase payload shapes; payload is host-injected plain data, size-capped. |
| `scriptConfig` = `{ eventName, settingKey, timeoutMs, maxStatements, onDeny }` | Keep `eventName` and `onDeny` (registry key replaces `settingKey`). `timeoutMs` maps to the wall-clock budget; **`maxStatements` has no Boa equivalent** (Boa has no instruction fuel) — the field is accepted for config compatibility but enforced as no-op; CPU runaway is bounded by the wall-clock timeout plus circuit breaker (ADR-0135 §3). |
| Limits: strict mode, 50 ms `TimeoutInterval`, 10,000 `MaxStatements`, applied only to `check()` | Strict-mode execution; wall-clock timeout (default 250 ms, per-script configurable within a hard ceiling); no statement counter. |
| Storage: whole config JSON in Settings row `script.{eventName}`, etag computed but unused, 5-minute prepared-script TTL | `script_registry` row per event with immediate propagation to new invocations; compiled-artifact caching is an internal detail. |
| Audit: log lines only (event, key, SHA-256 of body); spec'd deletion audit table never built | `script_runs` audit row per execution (decision, duration, failure, script hash) — this is new capability, closing a legacy gap, not a behavior change. |
| Fail-open everywhere | Fail-degraded: the triggering operation completes as if the hook returned its documented default; failure recorded (ADR-0135 §4). Equivalent observable behavior for all Jint hooks. |

### ECMAScript coverage note (Jint 4.12.0 vs Boa)

Both engines implement modern ECMAScript; typical legacy predicate scripts
(comparisons, boolean logic, `Date` parsing of ISO-8601 strings, array
methods like `.includes`/`.length`) run identically on both. Known
translation hazards:

- Boa does not implement every legacy-corner behavior (ADR-0135
  "Negative / trade-offs"); any stored script relying on Jint-specific quirks
  must be caught by the WP-11 validation pass against real stored scripts.
- Jint's strict mode was host-enabled (`options.Strict = true`); Boa scripts
  should be wrapped/executed in strict mode by the executor to match.
- Date/time behavior: legacy contexts pass `now`/timestamps as ISO-8601
  strings and scripts call `new Date(ctx.now)`; keep timestamps ISO-8601 UTC
  strings so this idiom keeps working.
- **Uncertain:** no corpus of real operator scripts exists in the frozen
  repo (scripts live in operator databases). Migration fidelity can only be
  validated against the documented contract and the legacy test corpus, not
  against production scripts in the wild.

## 2. Per-hook dispositions

### 2a. Jint event hooks

| Legacy event | Disposition | Notes |
|---|---|---|
| `directoryProcessingStart` | **Migrated-with-changes (implemented WP-11)** | Fires at the Rust pipeline point equivalent to the legacy pre-plugin-processing gate: in `crates/melodee-ingestion/src/inspect.rs`, once per claimed stable candidate directory, after the snapshot signature is re-verified and immediately before media inspection (`inspect_release`). The ctx preserves the shipped legacy fields (`path`, `directoryName`, `totalFilesCount`, `totalSizeMegabytes`, `mostRecentModified`, `mediaFilesCount`, `totalDurationMinutes`, `trackNumbers`, `hasTrackNumberGaps`), built from the verified snapshot plus per-file tag probes exactly like the legacy `DirectoryContextProvider` (top-level files only; unreadable tags logged and skipped; MiB/minutes rounded to 2 decimals; timestamps ISO-8601 UTC — RFC 3339 rather than .NET "O", both parse via the documented `new Date(...)` idiom). **Decisions recorded:** (1) `hasTrackNumberGaps` uses the **shipped** legacy definition — sequences may start at any number, adjacent sorted pairs must differ by exactly 1, duplicates are gaps (resolves uncertainty #1 in favor of compatibility with real stored scripts; the stricter spec-doc definition was never shipped); (2) `libraryId`/`relativePath` are **not** added — the legacy editor mock advertised them but production never supplied them (inventory §4.10), and the Rust test surface must validate against the real payload; additive extension would need a product note; (3) `onDeny`: `skip` (default) returns the work item to settling unprocessed — it is re-evaluated on a later pass, matching legacy "leave the directory" behavior; `quarantine` physical moves are **deferred** (they require the durable Phase 5 decision/audit/authorization contract) — the deny decision is recorded in `script_runs` and the skip applies; `delete` stays rewritten to `skip` (legacy safety rewrite, inventory §1a/§5). The registry therefore carries no `onDeny` column in this slice and the injected `scriptConfig.onDeny` is the constant `"skip"`; a future quarantine action lands with its own schema/config decision; (4) tag probing is bounded at 4096 media files per ctx (count/size fields always cover every file) — legacy was unbounded; |
| `directoryProcessingDelete` | **Not migrated** | Registered but never evaluated in legacy (inventory §1a, §4.1); `delete` is forcibly rewritten to `skip`. Wiring it now would be new behavior, and a destructive hook conflicts with the durable Phase 5 decision/audit/authorization contract. Document as "registered legacy name with no runtime behavior; do not recreate." |
| `userRegistrationStart` | **Migrated-with-changes** | Legacy fired at Blazor page-init with placeholder values (`userNameLength=0`, `emailDomain=""`) as a cosmetic UI gate. The Rust hook fires at the server-side registration boundary with the real payload (username length, email domain, client IP, user agent, timestamp). This changes the lifecycle point and payload quality on purpose — the placeholder behavior is a documented legacy defect (inventory §4.6), not a contract to preserve. Effect on deny: reject the registration with the script `message` surfaced to the client. |
| `userLoginStart` | **Migrated-with-changes** | Same reasoning: move from page-init (`userId=null`, `roles=[]`) to the server-side login boundary with real values where knowable pre-session (userId/roles only after credential check — **uncertain** whether the hook fires pre-auth with nulls or post-credential with real values; needs a product decision). Deny rejects the login with message. |
| `userProfileUpdateStart` | **Migrated-with-changes** | Fire at the server-side profile-update boundary with real `userId`, `emailDomain`, and an actual change count (or changed-field list — legacy spec'd `changedFields` but shipped `profileChangesCount`; **uncertain**, pick per hook contract). Deny rejects the update. |
| `playlistCreateStart` | **Migrated-with-changes** | Fire at the server-side playlist-create boundary with real `userId`, `nameLength`, `initialSongCount`. Deny rejects creation. The legacy page-init button-disabling has no Phase 9 counterpart (admin UI is Phase 10; the playlist API surface is the enforcement point). |
| `podcastChannelAddStart` | **Migrated-with-changes** | Fire at the server-side channel-add boundary with real `userId`, `feedUrl`, `isNewSubscription`. Deny rejects the add. Legacy spec also offered `urlScheme`/`urlHost`/`isHttps` derived fields (`event-scripting-requirements.md:388-398`) that were never shipped — **uncertain**; additive if adopted. |
| `requestCreateStart` | **Migrated-with-changes** | Fire at the server-side request-create boundary with real `userId`, `requestType`, `isFirstRequestToday`, `dailyRequestCount`. Deny rejects the request. |
| `shareCreateStart` | **Not migrated** | Spec-only in legacy; never registered, never invoked (inventory §1a, §4.2). If share gating is wanted it is new design requiring its own ADR. |

**Blazor page-init gating as such is not migrated.** The six UI gates were
client-side cosmetics on the legacy admin/web app; the Phase 10 admin UI may
re-introduce form-disabling driven by the server-side hook outcomes, but no
page-init evaluation is carried over. Native/OpenSubsonic/Jellyfin routes
never evaluated these hooks in legacy; whether the Rust server-side hooks
apply on those compatibility surfaces is **uncertain** and must be decided
per surface (default assumption: hooks fire at the core service boundary and
therefore apply uniformly; record deviations if compatibility evidence
demands them).

### 2b. Legacy shell-script plugin hooks

| Legacy hook | Disposition | Notes |
|---|---|---|
| `PreDiscoveryScript` (`scripting.preDiscoveryScript`) | **Not migrated (as shell)** | Arbitrary bash with full host access has no counterpart in the ADR-0135 capability-less sandbox, and adding one would require a new ADR and a security story the parity scope does not include. Operator migration paths: (1) re-express the gate as a Boa `directoryProcessingStart` script when the logic is a pure predicate over the documented `ctx` (covers the common "reject directories that look wrong" use case); (2) run the external script outside Melodee (cron/systemd/watch-folder tooling) against the inbound directory before Melodee sees it. Note the semantic inversion: legacy pre-discovery failure **aborted** the run (fail-closed); Boa hooks fail-degraded. Operators relying on abort-on-error must be told the Boa equivalent cannot abort the pipeline on script error. |
| `PostDiscoveryScript` (`scripting.postDiscoveryScript`) | **Not migrated (as shell)** | Same rationale. Post-processing side effects (notifications, file moves, external tagging) must live outside the server process. No Boa hook is proposed for a post-processing notification in WP-11 scope — **uncertain** whether a future read-only "directoryProcessingCompleted" notification hook is warranted; out of scope pending product decision. |
| `scripting.enabled` master switch | **Not migrated** | Exists only to gate the shell plugins. The Boa executor is per-event enabled/disabled via `script_registry`. |

## 3. Translation example — `directoryProcessingStart`

Legacy Jint script (from `docs/pages/scripting.md:130-137`, the documented
canonical example):

```javascript
function check(ctx, scriptConfig) {
  if (ctx.mediaFilesCount < 3) {
    return { result: false, message: "Release has fewer than three media files." };
  }
  return true;
}
```

Boa-compatible equivalent — **identical source text**. The contract is
engine-neutral modern ECMAScript; no rewrite is required for this class of
script:

```javascript
function check(ctx, scriptConfig) {
  if (ctx.mediaFilesCount < 3) {
    return { result: false, message: "Release has fewer than three media files." };
  }
  return true;
}
```

Bare-expression form also translates unchanged and must keep working through
the executor's auto-wrap:

```javascript
ctx.mediaFilesCount >= 3 && !ctx.hasTrackNumberGaps
```

Differences the operator may observe despite identical source:

- `scriptConfig.maxStatements` is accepted but not enforced (no Boa fuel);
  a pathological loop is stopped by the wall-clock timeout instead of a
  statement counter, and repeated trips disable the script via the circuit
  breaker rather than failing each invocation.
- Timeout budget defaults differ (legacy 50 ms Jint constraint vs ADR-0135
  default 250 ms wall clock); both are per-script configurable.
- Failure audit moves from log lines to queryable `script_runs` rows.

## 4. Configuration migration

| Legacy key | Rust disposition |
|---|---|
| `script.{eventName}` Settings JSON | Migrated into `script_registry` (ADR-0134) per event: body, enabled, timeout, onDeny. `engine` and `default.enabled` fields are dropped (both were stored-but-ignored in legacy — inventory §4.4). `overrides` are dropped (dead in legacy — inventory §4.3). `version` is superseded by the registry schema. |
| `script.dryRun.enabled` | **Not migrated (WP-11 decision).** In legacy it guarded only the safe-delete service, which is unreachable (delete is rewritten to skip); with quarantine moves deferred there is nothing for it to gate. If physical deny actions land later, this setting returns with them. |
| `script.quarantine.path` | **Deferred with quarantine moves (WP-11).** No physical quarantine exists in this slice, so no quarantine root is configured; when quarantine lands it must keep the traversal-containment requirement (writable root, not inside inbound). |
| `scripting.enabled`, `scripting.preDiscoveryScript`, `scripting.postDiscoveryScript` | Not migrated (shell subsystem, §2b). Import tooling should warn when these are set in a legacy export, pointing at this guide. |
| `library.inboundPath.<libraryId>` (deletion containment) | Not migrated; spec-only in legacy and tied to the non-existent delete path. |

## 5. Honest uncertainty register

1. ~~`hasTrackNumberGaps` definition~~ — **resolved (WP-11):** the shipped
   legacy definition (any start, adjacent-diff-exactly-1, duplicates are
   gaps) is implemented and documented in the hook contract; the stricter
   spec-doc definition was never shipped and is not adopted.
2. `userLoginStart` payload timing (pre-auth nulls vs post-credential real
   values).
3. `userProfileUpdateStart`: keep shipped `profileChangesCount` or adopt the
   spec'd `changedFields` list.
4. Whether server-side hooks apply on the OpenSubsonic/Jellyfin
   compatibility surfaces (they never did in legacy because legacy hooks
   were Blazor-only; Rust enforcement at the service boundary changes that
   by default).
5. `podcastChannelAddStart` derived URL fields (spec'd, never shipped).
6. No real-world operator script corpus exists to validate Boa/Jint
   equivalence against; validation is contract- and test-corpus-based only.
7. `script.dryRun.enabled` disposition (§4).
8. A post-processing notification hook (`PostDiscoveryScript` successor) is
   out of WP-11 scope pending product decision.
