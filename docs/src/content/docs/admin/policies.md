---
title: Policies
description: Classification and policy rules governing inbound Releases.
sidebar:
  order: 3
---

Policies classify cataloged Releases, assess Track novelty, and choose one of five dispositions:
`Accept`, `Review`, `Ignore`, `Quarantine`, or `Delete`. Every durable decision records the exact
immutable rule-set version, matched rules, evidence summary, actor, and sealed-manifest fingerprint.

The server evaluates only authoritative PostgreSQL catalog and manifest evidence. It does not open
source or canonical media. Classification considers structured and embedded evidence, existing
independently asserted catalog traits, and weak title/directory observations. Novelty distinguishes
exact-byte matches, known AudioIdentities, confirmed alternate recordings, audited no-catalog-match
findings, and uncertain material. Unconfirmed identities fail closed as uncertain. A name or
directory match by itself never authorizes deletion.

## Greatest Hits defaults

The built-in rule set defaults to:

- `Ignore` for a Greatest Hits Release whose Tracks are all redundant;
- `Review` when any Track is new or a confirmed alternate recording; and
- `Review` when novelty is uncertain.

The safe preference ranges and exact settings are listed in
[Configuration](../installation/configuration/#policy-evaluation). A `Delete` outcome is never an
instruction to remove media. It creates a pending decision and routes the Release to
`NeedsReview`. Phase 5 has no CLI action that completes deletion.

Rule sets are immutable by name and version. If any configured preference changes, increment
`MELODEE__POLICY__RULE_SET_VERSION`. Reusing a stored version with different rules fails startup
instead of changing the meaning of historical decisions.

## Test and inspect policy

These commands require `MELODEE__DATABASE__URL`:

```bash
melodee policy config
melodee policy rules
melodee policy test --limit 100 --actor <administrator-id>
melodee policy evaluate <release-id>
melodee policy inspect <release-id>
```

`policy test` requires a nonblank administrator identity, evaluates a bounded catalog sample, and stores an audit summary containing proposed
dispositions, matched-rule counts, existing-disposition counts, destructive impact, and up to 20
sample Release IDs. It does not persist the proposed decisions. `policy evaluate` reports one
Release's proposed decision without persisting that decision. Both commands provision the exact
configured rule set idempotently if it is not present. `policy inspect` prints the current durable
decision and its append-only audit history.

## Administrator decisions

A pending `NeedsReview` decision can be overridden explicitly:

```bash
melodee policy decide <release-id> <accept|review|ignore|quarantine|delete> \
  --actor <administrator-id> \
  --reason "<operator-safe reason>"
```

The current sealed-manifest fingerprint is checked in the same transaction as the audited update.
The actor and non-empty reason are mandatory. `Accept`, `Ignore`, and `Quarantine` complete the
non-destructive decision and route the work item accordingly. `Review` and `Delete` remain pending
in `NeedsReview`; the command does not access, move, or delete media.

:::caution[No graphical policy editor yet]
These PostgreSQL, worker, and CLI surfaces are implemented. A graphical rule editor and Release
review screen remain planned.
:::
