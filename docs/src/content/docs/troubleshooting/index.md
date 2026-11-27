---
title: Troubleshooting
description: Diagnosing and resolving common Melodee problems.
sidebar:
  order: 1
---

This section collects diagnostic steps for common operational problems. Each entry links back to
the relevant feature or configuration documentation.

## Application logs

- Melodee always writes server logs to the console. Set `MELODEE__LOGGING__LEVEL` to `trace`,
  `debug`, `info`, `warn`, or `error` to choose the verbosity.
- Set `MELODEE__LOGGING__FORMAT=json` to emit one structured JSON object per event for a log
  shipper or an automation agent. The default is human-readable text.
- Set `MELODEE__LOGGING__DIRECTORY` to additionally append the same events to
  `<directory>/YYYY-MM-DD.log`. The date is UTC; the server creates the directory,
  reports the current file at startup, and switches files after UTC midnight. Older files are
  retained, so configure host or container retention according to available space. Limit file
  access because logs can contain media paths and account identifiers. See
  [Configuration](../installation/configuration/#core-runtime).
- A failed Release keeps its exact error code and message in PostgreSQL. Open **Processing →
  Failed**, read the **Last error** column, select the row for its inspector/history, or run
  `melodee-cli work show <id>`. Worker warning events also include the durable error code and
  message, so the reason does not disappear when a retry is scheduled.

## Runtime diagnostics API

When you have an administrator token, `GET /api/admin/v1/diagnostics/*` answers most "is this
deployment healthy, and what does this setting actually do?" questions without shell access:

| Route | Use it for |
| --- | --- |
| `GET /api/admin/v1/diagnostics/selftest` | One pass/warn/fail verdict per subsystem (database, migrations, settings, storage/media tools, search, streaming, deny lists, notifications) with an actionable hint. |
| `GET /api/admin/v1/diagnostics/settings` | Every effective setting, its winning layer (`environment`, `config-file`, `database`, `default`), its registered kind, and its runtime consumer or the reason it is inert. Use this when a configured value appears to have no effect. |
| `GET /api/admin/v1/diagnostics/events?limit=100` | The most recent 200 `WARN`/`ERROR` events with correlation ids, without reading container logs. |
| `GET /api/admin/v1/diagnostics/bundle` | A single redacted support report (version, uptime, selftest, settings summary, streaming admission, recent events) to attach to an issue. |

See the [Admin API](../api/admin-api/#runtime-diagnostics) for the exact response shapes. Every
route requires an administrator token (`401` anonymous, `403` non-administrator).

## Requests that return `429`

- `429` from a native, OpenSubsonic, or Jellyfin stream means the per-user concurrent stream cap
  (`streaming.maxConcurrentStreams.perUser`) is configured and this user exceeded it. Stop the
  extra stream and retry; the response body is the legacy `"Too many concurrent streams"` shape.
- `429` from authentication means the authentication rate window was exceeded. Respect
  `Retry-After`.
- The global direct-play cap queues requests with backpressure rather than rejecting; check
  `GET /api/admin/v1/diagnostics/bundle` → `streaming` for admitted/available capacity.

## Requests that return `403 BLACKLISTED`

An operator deny list matched: `security.blacklistedEmails` or `security.blacklistedIPs`.
The settings are cached for 10 seconds, so a change takes effect without a restart. Check
`GET /api/admin/v1/diagnostics/selftest` → `blacklist` for loaded counts, and
`GET /api/admin/v1/diagnostics/settings` for the winning layer of each list. A blocked request
logs `WARN` with the rule name only.

## Server and queue checks

- Request `GET /healthz` to confirm that the HTTP service is running. Because the listener starts
  after PostgreSQL connection and migration, a connection failure during startup prevents this
  route from becoming available.
- Request `GET /readyz` to run configured dependency checks. HTTP 503 means PostgreSQL did not
  answer in time or, when search is enabled, no healthy active generation is available.
- Use the [Admin diagnostics API](../api/admin-api/) to inspect queue counts, bounded work-item
  listings, one item's last error and history, and active lease evidence.
- Run `melodee-cli work list` or `melodee-cli work show <id>` for the same durable work state from the CLI.
  These commands require a working `MELODEE__DATABASE__URL` connection.
- Treat an active lease as database evidence, not worker liveness. An expired lease is recovered by
  the durable work engine; an unexpired lease does not prove its owner process is alive.

The readiness route is deliberately narrow. It does not report disk space, media tools, or
compatibility adapters. See
[System and Diagnostics](../admin/system-diagnostics/) for the exact implemented boundary.

## Migration checksum mismatch

The server validates SQLx migration history before opening its listener. An error such as
`VersionMismatch(24)` means the checksum stored when that migration ran differs from the migration
embedded in the current binary.

Current Melodee builds accept the repository's exact known comment-only variants of migrations
24–29 without rerunning SQL or rewriting `_sqlx_migrations`. If a current build still reports
`VersionMismatch(<number>)`, treat it as unknown migration drift:

1. Stop the candidate server and preserve the database unchanged.
2. Confirm that `MELODEE__DATABASE__URL` names the intended database and record the exact binary or
   image revision.
3. Back up PostgreSQL and validate that backup before further diagnosis.
4. Compare the database's `_sqlx_migrations` row with the migration set from the exact release that
   originally applied it. Do not edit the checksum row or edit an already-applied migration.

For a disposable local-development database only, `make postgres-reset` can create a fresh
database after you have explicitly accepted losing all data in the development volume. Never use
that command as an upgrade or production recovery procedure. See [Upgrading](../installation/upgrading/)
for backup, maintenance-window, and rollback requirements.

## Catalog import checks

- Run `melodee-cli catalog status` to confirm whether the catalog worker is enabled and compare
  Release/import counts. `pending_outbox` represents work not yet acknowledged by the search
  projection; inspect search status if it continues growing.
- Run `melodee-cli catalog inspect <work-item-id>` to compare the work item's durable state with its
  immutable import receipt. `Published` is awaiting a claim, `Cataloging` is leased, waiting for its
  retry time, or reclaimable, and `Cataloged` should have a receipt. Failed attempts include the
  next retry time and last durable error.
- Run `melodee-cli catalog import` to execute one bounded pass while diagnosing the background worker.
  This command uses PostgreSQL evidence only and does not require access to inbound or canonical
  media roots.
- Check `MELODEE__CATALOG__ENABLED`, the catalog concurrency/batch/poll settings, and the shared work
  lease/retry settings in [Configuration](../installation/configuration/). Invalid zero or negative
  bounds fail configuration rather than creating an unbounded worker.

## Media normalization checks

Implemented normalization checks:

- Run `melodee-cli doctor` and confirm `ffmpeg`, `ffprobe`, and `fpcalc` report `ok`.
- Verify the configured executable paths and timeouts under
  `MELODEE__NORMALIZATION__*`. FFmpeg must include the `libmp3lame` encoder.
- Inspect the work item's durable error code and manifest findings. Tool failures, timeouts,
  fingerprint failures, source changes, and output-verification failures are retryable and do not
  publish partial normalization results.
- Ensure `MELODEE__NORMALIZATION__WORK_AREA` is writable and has enough temporary space. Attempt
  directories are deliberately retained for diagnosis; they are not canonical library storage.
- Set `MELODEE__STORAGE__ROOT` to a dedicated canonical-library root before running validation or
  publication. It must not overlap an inbound root or the normalization work area.
- Use `melodee ingest validate` and `melodee ingest publish` to run one bounded batch while
  diagnosing a worker. A target conflict is never overwritten; inspect the durable work-item and
  publication journal evidence before correcting the external filesystem state.

## Interrupted pre-catalog purge

A purge interrupted after its guard was recorded can leave the affected inbound occurrence
blocked. The guard does not establish which filesystem operations completed. Preserve the source,
staging, work, and canonical media and do not repeat a purge to clear it.

1. Enter a maintenance window. Stop and fence **all** Melodee servers and workers, including
   processes on other hosts; verify they cannot resume during recovery. An expired database lease
   alone does not prove an old process has stopped.
2. Back up the database and affected media, then inspect the source, staging, work, and canonical
   locations and the work-item history. If the result is ambiguous, keep the guard and investigate.
3. Run `melodee-cli ingest purge-guards`. The output includes work item IDs, guard owners, fencing
   tokens, source identities, and intent times. Continue with `--after <last-work-item-id>` when
   the page contains 100 entries. Protect this output because it contains media paths.
4. After reviewing the exact guard and deciding that releasing it is safe, run:

   ```text
   melodee-cli ingest recover-purge-guard --work-item-id <id> --fence-token <token> --guard-owner <owner> --actor <operator> --reason '<inspection and decision>' --stopped-all-writers --inspected-media --backup-verified --confirm
   ```

The command requires every attestation and rejects a changed guard or a live processing lease. It
records the supplied operator and reason in the database audit and releases only the matching
guard; it does not delete media or work-item data or repeat the purge. The operator name is an
assertion by the local administrator, not a verified login. The command cannot prove remote
processes have stopped; maintain the fence until the evidence is reviewed and recovery is complete.

The current pre-catalog purge removes an inbound source occurrence without a staging handoff only
when the pinned occurrence is an empty directory, through a descriptor-relative
pin/move/verify/unlink protocol that preserves all bytes if any race is observed. A non-empty
inbound source still refuses with `source_custody_required` before deleting any storage; this
restriction remains until occurrence-bound removal of retained source bytes is available.
Preserve the source and do not bypass the restriction by deleting it manually.

## Retained inbound sources awaiting verification

Run `melodee-cli ingest retained-obligations` from an authorized administrative
shell to list active retained-source obligations. The command is read-only and
returns at most 100 entries by default; `--limit` accepts 1–128. For another
page, pass the previous response's `nextAfter` value as `--after <work-item-id>`.
When the last page has been read, start again from the beginning if a complete
current view is needed: new entries can arrive before a previously read cursor.

Each entry identifies the work item, source library when present, exact stored
source path and path format, verification state, retry or unreadable reason and
count, any unsupported-directory reason and retry time, next due time, and
lease state. The JSON output may contain sensitive media paths;
restrict access to it. Non-Unicode names are represented by their reversible
`//melodee-unix-hex:` or `//melodee-windows-u16:` encoding. Preserve the exact
JSON value rather than decoding it through a lossy display. Historical format-0
paths containing the replacement character are ambiguous and require review;
do not guess the original bytes.

`baseline_change_pending` or an `awaiting_intake` reason means the changed
source remains held for further safe intake. Listing it does not create a new
work item, verify the source, or authorize deletion or movement. Preserve the
source and its evidence; safe copy-only intake for this case is still under
development. A lease or due time is database evidence and does not prove an
old process is running or stopped.

## Policy evaluation checks

- Run `melodee-cli policy config` and `melodee-cli policy rules` to confirm the configured immutable
  rule-set version and safe disposition preferences. If a preference changed, increment
  `MELODEE__POLICY__RULE_SET_VERSION`; a same-version payload conflict deliberately fails closed.
  Version 1 is obsolete; set existing local `.env` files to version `2` or a later
  operator-authored version. Built-in v2 accepts an ordinary Unknown classification unless an
  explicit Unknown review rule is configured.
- Run `melodee-cli policy test --limit 100 --actor <administrator-id>` to compare proposed dispositions, matched rules, existing
  decisions/states, and destructive impact without writing proposed decisions.
- Run `melodee-cli policy inspect <release-id>` to inspect the current decision and append-only audit.
  A `Review` or `Delete` decision should remain pending in `NeedsReview`.
- Run `melodee-cli policy evaluate <release-id>` when a cataloged Release has no durable decision or
  needs a read-only comparison. Policy evaluation uses PostgreSQL facts and the sealed-manifest
  fingerprint; it does not need source or canonical media access.
- Check the `MELODEE__POLICY__*` settings in
  [Configuration](../installation/configuration/#policy-evaluation). Invalid zero/out-of-range
  bounds, unsafe dispositions, and immutable rule-set conflicts stop startup rather than silently
  changing policy.

## Search index checks

- Run `melodee-cli search status`. No active generation, a schema/open error, permanently failed rows,
  or an old growing backlog is actionable; retryable rows remain scheduled by `available_at` and
  become permanent evidence after the configured maximum attempt count. Pending and leased counts
  are disjoint, so add them when assessing all unfinished, non-failed work.
- Verify `MELODEE__SEARCH__ROOT` is absolute, dedicated, writable, and separate from inbound,
  normalization, and canonical storage roots. Do not replace its ownership marker or insert
  symlinks beneath it.
- Run `melodee-cli search rebuild --actor <administrator-id>` to recover a missing/corrupt index. The
  command reads catalog rows from PostgreSQL in bounded pages and does not reopen media.
- A crash after an index commit but before PostgreSQL acknowledgement is safe: the row is reclaimed
  and revision fencing makes replay idempotent. A stale owner cannot acknowledge after lease loss.
- If `/healthz` is healthy but `/readyz` reports not ready, compare PostgreSQL connectivity with
  `melodee-cli search status`; rebuild if the configured active generation is missing or incompatible.

For container-specific checks, image tags, attestation verification, and persistent paths, see
[Container deployment](../installation/docker/). For legacy C# migration issues — unsupported
schema fingerprints, durable issue codes, resume after interruption, and media adoption
validation — see [Legacy C# migration tooling](../installation/legacy-migration/); it is not
part of a Rust container upgrade.

## Migration and rollback diagnostics

- `melodee-cli legacy status --run <uuid> --json` reports durable state, checkpoints, lease
  ownership, and issue totals. A live lease is database evidence, not proof that the owner process
  is healthy; wait for its bounded lease or investigate before starting another owner.
- `melodee-cli legacy validate --run <uuid> --canonical-root <absolute-root> --json` is the safe
  first response to missing, moved, unsafe, or size-mismatched adopted media. Review its
  path-digest plan before any `--fix --confirm`; repair changes only target byte-size metadata.
- If verification remains in `verifying`, inspect every durable issue and rerun the same run after
  correcting the approved source snapshot or path mapping. Do not create a new run to conceal a
  mismatch.
- `melodee-cli rollback status --database-url "$MELODEE__DATABASE__URL"` distinguishes database
  reachability, current migrations, freeze evidence, and a cutover-ready run. The generated
  rollback runbook is an auditable procedure, not an automatic traffic switch.
- After an approved external traffic return to C#, run `melodee-cli rollback record ... --confirm`
  against the still `cutover-ready` run. Retain the Rust target and backups; never mark rollback
complete by deleting forensic evidence.

## Release-candidate and migration blockers

If release evidence names a different commit, image digest, architecture, or checksum than the
candidate under review, stop and invalidate the evidence. A clean local smoke test does not prove
arm64 execution, a current vulnerability disposition, provenance, production migration, physical
client acceptance, or cutover. Escalate those gaps to the release owner and keep source media,
backups, and failed targets until the documented acceptance window and cleanup approval are
complete.
