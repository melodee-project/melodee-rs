# ADR-0149 decision package — Phase 11 rehearsal, freeze, cutover, and rollback

Status: Decision required. ADR-0149 remains **Proposed** and has not been
accepted by this work.

Date: 2026-08-19  
Decision owner: Release owner  
Evidence: `rehearsal-drill-12000.json`,
`rehearsal-drill-12000-repeat.json`, and the focused migration/freeze tests.

## Decision requested

The release owner must accept, reject, or revise ADR-0149 before its freeze,
cutover, and rollback procedure is treated as release authority. The decision
must confirm:

1. the two-command `legacy baseline` / `legacy freeze` extension;
2. the operator-enforced source write-freeze boundary and marker proof;
3. the `cutover-ready` acknowledgement rule and required operator statement;
4. the non-destructive rollback procedure, objective trigger, decision window,
   and forensic-target retention; and
5. the production proxy/container traffic-switch mechanism and its authorized
   operator.

No production procedure, target transition, or ADR status was changed by this
package.

> **Disposition (2026-08-21):** ADR-0149 was **Accepted** by the release owner in
> commit `c8cfbd0` (all five confirmation points: the two-command `legacy baseline` /
> `legacy freeze` extension, the operator-enforced source write-freeze boundary and marker
> proof, the `cutover-ready` acknowledgement rule, the non-destructive rollback procedure with
> its objective trigger, decision window, and forensic-target retention, and the production
> proxy/container traffic-switch mechanism). ADR-0149 is now release authority for the R4.7/R4.8
> procedures. The text below is the historical decision record and is preserved as written.

## Implemented evidence

The local implementation provides:

- a stable `melodee.legacy-baseline` artifact with schema fingerprint, source
  identity digest, `legacy-marker-v2`, read-only facts, counts, media totals,
  and redacted configuration fingerprints;
- `legacy baseline` and `legacy freeze` CLI help and JSON paths;
- baseline drift rejection, source-write abort, acknowledgement gating, and
  lease-fencing tests in `apps/melodee-cli/tests/legacy_freeze.rs`;
- final-delta convergence, pre-marker write inclusion, freeze timing, backup
  restore-content validation, failed-target retention, and non-destructive
  rollback evidence in the two rehearsal artifacts; and
- exact documented command verification recorded in `design/evidence/r4/README.md`.

## Current isolated rehearsal measurements

The latest 12,000-Track rehearsal used disposable PostgreSQL databases and a
synthetic canonical tree. It is not production evidence.

| Measure | Result |
|---|---:|
| Source / target catalog | 500 Artists, 1,000 Releases, 12,000 Tracks, 50 users |
| Source media | 12,000 files / 12,000,000,000 bytes |
| Import wall time / peak RSS | 178.34 s / 31,182,848 bytes |
| Verification | 0 mismatch gaps / 0 duplicate public IDs |
| Search rebuild wall time / peak RSS | 5.83 s / 161,964,032 bytes |
| Target database after import | 98,973,375 bytes |
| Backup / restored content | 7,365,393-byte dump / 12,000 Tracks restored |
| Freeze window | 3,466 ms |
| Pre-marker write | Imported by final delta |
| Rollback decision window | 300 seconds |
| Failed target | Preserved for forensic analysis |
| Canonical media | 16 files / 16,000,000 bytes unchanged |

The repeat artifact reports matching catalog counts, restored content, freeze
convergence, retained failed target, and unchanged canonical media. Both runs
are synthetic and therefore cannot establish the required production-sized
approximately-5-TB capacity or deployment-topology behavior.

## Required external evidence before R4.7/R4.8 completion

- an approved isolated production snapshot, backup location, and canonical-root
  mapping;
- the authorized source write-freeze operator and exact enforcement command;
- the approved proxy/container traffic switch and rollback owner;
- production-topology restore and rollback execution with the failed target
  retained; and
- release-owner disposition of ADR-0149, plus ADR-0150 for dependent metadata
  semantics.

Until those inputs and the decision are recorded, R4.7 and R4.8 remain Partial
and `MELODEE 1.0.0: INCOMPLETE` remains the truthful release status.
