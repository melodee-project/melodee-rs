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

## Policy evaluation checks

- Run `melodee-cli policy config` and `melodee-cli policy rules` to confirm the configured immutable
  rule-set version and safe disposition preferences. If a preference changed, increment
  `MELODEE__POLICY__RULE_SET_VERSION`; a same-version payload conflict deliberately fails closed.
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
