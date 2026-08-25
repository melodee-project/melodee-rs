# Cutover and Rollback Runbook

This is the operator procedure for moving an existing C# Melodee installation to the Rust
server and returning traffic to C# if acceptance fails. It explicitly separates migration
proof from infrastructure actions that require an operator and approver.

The migration source and its media are read-only. The Rust target is retained for forensic
inspection after a failed cutover. No command here deletes a source, target, backup, snapshot,
or rollback asset.

References: [ADR-0148](../adr/ADR-0148-legacy-migration-contract.md),
[ADR-0149](../adr/ADR-0149-phase-11-rehearsal-freeze-cutover-and-rollback-procedures.md), and the
[public migration guide](../../docs/src/content/docs/installation/legacy-migration.md).

## Roles and approval gates

Name these roles in the change record:

- **Migration operator** — runs inventory, import, verification, backup, and diagnostics.
- **Data owner** — confirms the source snapshot, path mappings, domains, and issue dispositions.
- **Release owner** — confirms the Rust image, Cargo/CLI version, digest, and release evidence.
- **Client/compatibility owner** — runs native-v1, OpenSubsonic, Jellyfin, and MeloAmp checks.
- **Approver** — authorizes production access, write freeze, final delta, traffic change,
  rollback activation, and rollback-asset cleanup.

Production database/media access, production-sized rehearsal, live provider/OIDC/registry use,
C# write freeze, final production delta, traffic switching, production rollback, release/GHCR
publication, and deletion of temporary or rollback assets each require a separate approval.

## Variables and safety rules

Set these only in the protected operator environment, never in committed evidence or shell
history:

```text
LEGACY_SOURCE_DATABASE_URL=<approved read-only snapshot URL>
RUST_DATABASE_URL=<approved Rust target URL>
CANONICAL_ROOT=<approved absolute mounted media root>
PATH_MAP=/legacy/library=root-relative-prefix
RUN_ID=<migration run UUID>
RUST_TARGET=<exact Rust service/container identifier>
LEGACY_TARGET=<exact C# service/container identifier>
TRAFFIC_TARGET=<exact proxy/load-balancer/DNS control identifier>
BACKUP_DIR=<approved backup directory>
MIGRATION_OPERATOR=<stable operator identity>
```

Before every externally visible or destructive step, verify the exact target, expected impact,
approval record, and topology. Stop if the target, database identity, snapshot marker, canonical
root, or backup differs from the change record. Redact URLs, credentials, private paths, hashes,
tokens, and session keys from saved output.

## 1. Preflight and source baseline

Against the approved isolated snapshot and Rust target, run:

```bash
melodee migrate
melodee doctor --deep
melodee legacy inventory --source-db "$LEGACY_SOURCE_DATABASE_URL" --json > inventory.json
melodee legacy plan --source-db "$LEGACY_SOURCE_DATABASE_URL" \
  --library-path-map "$PATH_MAP" --json > plan.json
melodee legacy baseline --source-db "$LEGACY_SOURCE_DATABASE_URL" --json > baseline.json
```

Confirm the schema fingerprint, source identity, snapshot marker, all persistent domains, path
mappings, and issue dispositions with the data owner. An unexpected fingerprint or identity fails
closed. Back up and validate the source snapshot before creating or changing a Rust target:

```bash
mkdir -p -- "$BACKUP_DIR"
pg_dump --dbname="$LEGACY_SOURCE_DATABASE_URL" --format=custom \
  --file="$BACKUP_DIR/legacy-source.dump"
pg_restore --list "$BACKUP_DIR/legacy-source.dump" > "$BACKUP_DIR/legacy-source.list"
```

A failed or incomplete backup blocks cutover. The source remains read-only.

## 2. Initial import and verification

Create the run from the plan, or resume the named run after an interruption:

```bash
melodee legacy import --source-db "$LEGACY_SOURCE_DATABASE_URL" \
  --library-path-map "$PATH_MAP" --canonical-root "$CANONICAL_ROOT" \
  --batch-size 500
melodee legacy status --run "$RUN_ID" --json
melodee legacy verify --run "$RUN_ID" --source-db "$LEGACY_SOURCE_DATABASE_URL" \
  --canonical-root "$CANONICAL_ROOT" --json > verify-initial.json
```

If the process stops, inspect the lease/checkpoint state and resume with
`legacy import --run "$RUN_ID"` using the same source identity, path map, and canonical root.
Never start a second run to hide an unresolved issue. Review every durable issue, including
unsupported credentials, secret reseal failures, media mismatches, exclusions, and
unrepresentable fields.

The media validator is read-only by default and emits path digests, not private paths:

```bash
melodee legacy validate --run "$RUN_ID" --canonical-root "$CANONICAL_ROOT" --json \
  > media-validation.json
```

Only a `delta-ready` run may receive the narrow derived-metadata repair, after reviewing the
deterministic plan:

```bash
melodee legacy validate --run "$RUN_ID" --canonical-root "$CANONICAL_ROOT" \
  --fix --confirm --actor "$MIGRATION_OPERATOR" --json > media-repair.json
```

This changes target `media_assets.byte_size` only when a no-follow regular-file observation still
matches the plan. It never changes media bytes, paths, hashes, identities, relationships, or
source state and never deletes uncertain media.

Rebuild search from PostgreSQL, then verify again:

```bash
melodee search rebuild
melodee legacy verify --run "$RUN_ID" --source-db "$LEGACY_SOURCE_DATABASE_URL" \
  --canonical-root "$CANONICAL_ROOT" --json > verify-search.json
```

## 3. Rust backup and isolated restore

Before a final delta or traffic decision, back up the exact Rust target and record protected
configuration/secret-store version identifiers:

```bash
pg_dump --dbname="$RUST_DATABASE_URL" --format=custom \
  --file="$BACKUP_DIR/rust-target-${RUN_ID}.dump"
pg_restore --list "$BACKUP_DIR/rust-target-${RUN_ID}.dump" \
  > "$BACKUP_DIR/rust-target-${RUN_ID}.list"
```

Restore into a separately named isolated database and verify catalog counts, users, settings,
public IDs, media references, `/healthz`, `/readyz`, search rebuild, compatibility probes, and a
representative client stream. Do not point production traffic at the restore database.

Tantivy is not backup authority. Restore PostgreSQL first and rebuild Tantivy from restored
PostgreSQL state. A failed backup, restore, search rebuild, or compatibility probe blocks cutover.

## 4. Delta and freeze gate

Run the ordinary delta until the run is clean and `delta-ready`:

```bash
melodee legacy delta --run "$RUN_ID" --source-db "$LEGACY_SOURCE_DATABASE_URL" \
  --library-path-map "$PATH_MAP" --canonical-root "$CANONICAL_ROOT"
melodee legacy verify --run "$RUN_ID" --source-db "$LEGACY_SOURCE_DATABASE_URL" \
  --canonical-root "$CANONICAL_ROOT" --json
```

The approver now authorizes the C# write freeze and records the exact C# target, freeze start,
operator, and expected impact. After that external freeze is confirmed, run:

```bash
melodee legacy freeze --run "$RUN_ID" --source-db "$LEGACY_SOURCE_DATABASE_URL" \
  --library-path-map "$PATH_MAP" --canonical-root "$CANONICAL_ROOT" \
  --json --acknowledge-cutover \
  --note "approved freeze change <ID>; C# writes frozen by <operator>"
```

`freeze` performs the final delta, verifies marker stillness and parity, records freeze evidence,
and enters `cutover-ready` only with `--acknowledge-cutover` and `--note`. It does not freeze C#
writes and does not switch traffic. A changed source marker, failed verification, missing backup,
or unresolved release blocker leaves the run non-ready.

## 5. Cutover acceptance and traffic switch

The release and compatibility owners verify the exact image digest, health/readiness, diagnostics,
search status, native-v1/OpenSubsonic/Jellyfin differential results, and available MeloAmp/client
journeys. Record approved differences and unexpected differences separately.

The approver explicitly authorizes the traffic change. The operator changes only the named
`TRAFFIC_TARGET` to the named `RUST_TARGET`, records the control-plane audit event, and verifies:

```bash
melodee doctor --deep
melodee rollback status --database-url "$RUST_DATABASE_URL"
# Probe /healthz, /readyz, native-v1, OpenSubsonic, Jellyfin, ranges, artwork,
# search, and the approved client journeys at the exact Rust endpoint.
```

Monitor errors, latency, database growth, migration issues, search status, job/provider failures,
and client reports throughout the rollback decision window. Do not mark the run complete or delete
source, backups, restored databases, or rollback assets before acceptance signoff.

## 6. Rollback decision and execution

Rollback criteria include authentication or public-ID loss, catalog/relationship corruption,
unsafe media behavior, failed readiness, sustained compatibility/client failure, data loss, or an
unresolved security/operational fault inside the decision window. The approver authorizes rollback
and records the objective trigger.

Generate the topology-bound procedure; generation changes no state:

```bash
melodee rollback status --database-url "$RUST_DATABASE_URL"
melodee rollback runbook --database-url "$RUST_DATABASE_URL" \
  --rust-target "$RUST_TARGET" --legacy-target "$LEGACY_TARGET" \
  --traffic-target "$TRAFFIC_TARGET" --confirm > rollback-runbook.json
```

Execute the generated steps through the reviewed deployment control plane:

1. Confirm and stop the exact Rust target gracefully; retain it for forensics.
2. Change the exact `TRAFFIC_TARGET` back to the exact `LEGACY_TARGET`; record the audit event.
3. Verify C# health, native-v1, OpenSubsonic, Jellyfin, public-ID lookup, and a known client
   journey before accepting writes.
4. After traffic is confirmed on C#, append the durable rollback record to the still-
   `cutover-ready` Rust run:

   ```bash
   melodee rollback record --database-url "$RUST_DATABASE_URL" --run "$RUN_ID" \
     --legacy-target "$LEGACY_TARGET" --by "$MIGRATION_OPERATOR" \
     --note "<approved objective rollback trigger>" --confirm
   ```

`rollback record` requires `cutover-ready`, records the exact target/operator/reason as a durable
redacted issue and run note, and never changes migration state or deletes target data. It must not
be used to claim production rollback when the external traffic switch did not occur.

## 7. Acceptance, retention, and cleanup

The data, release, and compatibility owners plus the approver sign the acceptance record with the
run ID, source marker, target/image digest, backup validation, verification, client results,
monitoring window, and approved differences. Retain the C# source/snapshot, Rust target, backups,
persistent-volume backups, restored test database, search evidence, and rollback runbook until
signoff plus the documented retention period.

Cleanup is a separate approved destructive action. Before cleanup, verify signed acceptance,
retention expiry, backup readability, and that no incident or rollback decision remains. Name each
exact object to remove; topology drift fails closed.

## What this runbook does not claim

Synthetic and isolated rehearsal tooling and local release validation do not prove production-sized
migration, physical-client acceptance, live provider behavior, traffic cutover, production
rollback, or release publication. Any unexecuted external leg remains a release blocker or an
explicitly approved release disposition.
