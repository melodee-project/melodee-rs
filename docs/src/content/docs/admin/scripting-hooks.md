---
title: Scripting Hooks
description: Operator-installed JavaScript hooks that gate inbound directory processing.
sidebar:
  order: 7
---

Melodee supports operator-installed JavaScript hooks that evaluate a decision for specific
lifecycle events. Scripts are installed by the server administrator only — there is no
user-submitted script surface anywhere in the product.

## Trust boundary and sandbox

Scripts run in a capability-less JavaScript sandbox (the [Boa](https://boajs.dev/) engine):

- **No filesystem, network, process, or module access exists in the sandbox.** Scripts receive
  exactly two pieces of plain data (`ctx` and `scriptConfig`) and return a decision; there is
  nothing to escape to.
- Every execution runs on a dedicated worker thread with a wall-clock timeout (default 250 ms,
  configurable per script up to 10 seconds). A script that overruns is abandoned and counted.
- Payloads and return values are size-capped (256 KiB in, 16 KiB out).
- A **circuit breaker** disables a script automatically after 3 consecutive failed runs
  (errors or timeouts) and records a `tripped` audit row. A tripped script stays off until you
  explicitly re-enable it.

Scripts are administrator trust-boundary content: they are not a security boundary against the
operator, but the sandbox means a buggy or copy-pasted script cannot read your files, call the
network, or spawn processes.

## The hook contract

A script is either a bare expression or a full `check` function. A bare expression is wrapped
automatically as `function check(ctx, scriptConfig) { return (<your expression>); }`.

```javascript
// Bare expression form
ctx.mediaFilesCount >= 3 && !ctx.hasTrackNumberGaps
```

```javascript
// Function form
function check(ctx, scriptConfig) {
  if (ctx.mediaFilesCount < 3) {
    return { result: false, message: "Release has fewer than three media files." };
  }
  return true;
}
```

Accepted return values:

- `true` — allow;
- `false` — deny;
- `{ result: true|false, message: "optional explanation" }` — verdict plus an operator-visible
  message.

Anything else — a number, a string, an exception, a timeout — is recorded as an `error` (or
`timeout`) audit row and the operation **proceeds as if the script had allowed it**
(fail-degraded semantics). A broken script can never block your library.

`scriptConfig` is plain data: `{ eventName, timeoutMs, onDeny }`. Legacy `maxStatements` has no
equivalent in the Rust engine; runaway CPU is bounded by the wall-clock timeout and the circuit
breaker instead.

## The master switch

Scripting is disabled by default, exactly like the legacy server: the registry key
`scripting.enabled` (environment `MELODEE__SCRIPTING__ENABLED`) defaults to `false`. While it is
false, installed scripts are **not evaluated** by the ingestion gate and no `script_runs` audit
rows are written. Admin test-runs still work so you can validate a script before enabling it.
The switch is resolved through the layered settings on each candidate, so a change takes effect
without a restart. The Admin UI Scripts tab and `melodee-cli scripts` show the same registry.

## The wired hook: `directoryProcessingStart`

This is the one hook with live server-side enforcement. It fires once per stable inbound
candidate directory, immediately before media inspection. The `ctx` payload:

| Field | Meaning |
|---|---|
| `path` | Absolute directory path |
| `directoryName` | Directory name |
| `totalFilesCount` | Top-level file count |
| `totalSizeMegabytes` | Top-level size in MiB (2 decimals) |
| `mostRecentModified` | Newest top-level modification time, ISO-8601 UTC |
| `mediaFilesCount` | Audio files whose tags were readable |
| `totalDurationMinutes` | Sum of media durations (2 decimals) |
| `trackNumbers` | Sorted track numbers from tags |
| `hasTrackNumberGaps` | True when sorted track numbers are non-consecutive (the sequence may start at any number; duplicates count as gaps) |

On deny, the effective action is **`skip`**: the directory is left unprocessed for that pass and
is re-evaluated on a later pass (the script runs again). Nothing is moved or deleted.

:::caution[Planned documentation]
The legacy `onDeny: quarantine` action (physically moving a denied directory) is planned but not
implemented: physical moves require Melodee's durable decision/audit/authorization pipeline.
Deny currently always behaves as `skip`. The legacy `delete` action was never honored — the
legacy server itself rewrote it to `skip` — and this is preserved deliberately.
:::

The six legacy UI-gate events (`userRegistrationStart`, `userLoginStart`, `userProfileUpdateStart`,
`playlistCreateStart`, `podcastChannelAddStart`, `requestCreateStart`) are accepted in the registry
for forward compatibility but are **inert** in this build: they fired as cosmetic form-disabling
in the legacy web UI with placeholder values. Their server-side enforcement points are planned
with real payloads; the admin API and CLI mark which events are currently wired.

The legacy shell-script hooks (`scripting.preDiscoveryScript` /
`scripting.postDiscoveryScript`) are **not** carried over: arbitrary shell execution is
incompatible with the sandbox. Re-express pure directory predicates as `directoryProcessingStart`
scripts, or run external tooling (cron, systemd, watch-folder scripts) outside Melodee. Note the
semantic difference: a failing legacy pre-discovery script aborted the whole processing run,
while a failing sandbox script fails open to allow.

## Audit

Every script evaluation is recorded in an append-only audit (`script_runs`): outcome
(`allow`, `deny`, `error`, `timeout`, `tripped`), duration, and a bounded error message. Rows
survive script deletion and are purged after 90 days.

## Managing scripts

In the admin UI, open **System → Settings → Scripts** (`/admin/system/settings`, Scripts tab) to
list scripts with their wired/inert events, enable or disable them, edit source (a plain
textarea — no code editor is claimed), and test-run a script against a `ctx` mock.

Admin API (administrator JWT required):

- `GET /api/admin/v1/scripts` — list (each row is flagged `wired`/`inert` via `wiredEvents`);
- `GET /api/admin/v1/scripts/{event}/{name}` / `PUT` (create or replace) ;
- `GET /api/admin/v1/scripts/id/{id}` / `DELETE`;
- `POST /api/admin/v1/scripts/id/{id}/enable` / `.../disable`;
- `GET /api/admin/v1/scripts/runs` — execution audit (bounded, newest first, `scriptId` filter);
- `POST /api/admin/v1/scripts/test-run` — evaluate a stored script (`scriptId`) or ad-hoc
  `source` against a JSON `ctx` mock and record the honest outcome.

CLI equivalents:

```bash
melodee-cli scripts list
melodee-cli scripts test --event directoryProcessingStart --ctx ctx.json
melodee-cli scripts test --event directoryProcessingStart --ctx ctx.json --source my-script.js
```

`--source` evaluates an ad-hoc file; without it, every enabled script for the event runs in
registry order against the mock. Every test run is recorded in the audit; CLI test-runs never
trip the circuit breaker.

## Migrating legacy scripts

Scripts written for the legacy server's Jint runtime use the same contract and typically run
unchanged (modern ECMAScript, camelCase `ctx`, same auto-wrap rule). Known differences:

- the timeout budget defaults to 250 ms wall clock (legacy: 50 ms);
- `maxStatements` is accepted in stored configs but not enforced (no instruction counter);
- failures are queryable audit rows instead of log lines;
- the legacy editor's mock ctx advertised `libraryId`/`relativePath` fields that production
  never supplied — test against the real field list above.

Per-hook dispositions and shell-hook migration paths are recorded in the repository's
engineering documentation (`design/adr/ADR-0135` and the scripting module documentation).
