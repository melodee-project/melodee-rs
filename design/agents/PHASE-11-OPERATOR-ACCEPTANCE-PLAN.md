# Phase 11 Operator Acceptance and Release-Security Plan

Status: **Open — operator and external gates remain unexecuted**  
Plan date: 2026-08-25  
Scope: canonical Phase 11 operator acceptance, security disposition, production rehearsal,
cutover, rollback, and release publication.

This plan is the execution contract for the remaining Phase 11 work. Historical R3/R4/R5
artifacts are inputs and regression evidence only; they are not evidence for a new candidate
unless the exact candidate commit, image digest, runner architecture, and tool versions are
recorded by the same run.

## Authority and safety boundary

The governing documents are `PRD.md`, `SPEC.md`, `IMPLEMENTATION.md`, the Definition of Done,
ADR-0144, ADR-0148, ADR-0149, ADR-0150, ADR-0151, ADR-0161, and
`design/runbooks/CUTOVER-AND-ROLLBACK.md`. C# and MeloAmp are behavior/client oracles and remain
read-only. No production or private data is accessed by preparation work.

The following actions require a separate approval at execution time: production snapshot or
media access; live providers/OIDC/registry credentials; C# write freeze; final delta; traffic
switch; production rollback; GHCR/GitHub Release publication; and deletion of backups, snapshots,
temporary imports, failed targets, or rollback assets.

## Entry recapture

Captured before edits on 2026-08-25 in `America/Chicago`:

| Repository | HEAD | Branch | Worktree | Handling |
|---|---|---|---|---|
| Rust rewrite | `35272b1d344cb0c821b171c3f215243af8706c24` | `main` | clean | candidate source, subject to final candidate recapture |
| C# oracle | `4028b58dbd3302ed431e2c5b32d9dde5634db218` | `sph.2026-07-11.01` | clean | read-only; not modified or built |
| MeloAmp | `096c76d810af91205aeee54da58131766b0f076b` | `sph.2026-07-14.01` | clean | read-only; not modified or built |

Cargo metadata reports `melodee-server=0.1.0` and `melodee-cli=0.1.0`. The proposed stable
release tag is therefore `v0.1.0`; a prerelease tag would require matching package-version
changes and a new candidate.

## Candidate protocol

1. Finish this documentation and any approved remediation before candidate selection.
2. Require a clean tree and record `git rev-parse HEAD`, branch, and all three repository states.
3. Use one immutable candidate commit and one exact revision label in every image and artifact.
4. Build each architecture once, record its image digest, and scan that exact image. Never rebuild
   after scanning; the publication job must assemble the archived scanned images.
5. Require `linux/amd64` and `linux/arm64` execution evidence, not static workflow inspection.
6. Require version, revision, OCI labels, SBOM, Trivy reports, scan status, image archive,
   `release-evidence.json`, and `SHA256SUMS` to agree. A dirty tree, floating tag, rebuilt image,
   missing report, checksum mismatch, or candidate-SHA mismatch fails closed.
7. Candidate evidence is invalidated by any later code or documentation change. The final
   operator report must state the exact candidate SHA and image digests captured after the last
   change.

## Workstream execution ledger

| Workstream | Planned command/evidence | Current state | Completion condition |
|---|---|---|---|
| Entry/candidate | `git` state; Cargo metadata; tool versions; clean candidate record | Preparation only; current clean HEAD recaptured | Immutable candidate SHA and version agreement |
| A Security | Fresh Trivy DB; exact amd64/arm64 scans; JSON/table reports; package/layer/reachability/CVE decisions; remediation rescans | Blocked: no current candidate images or exact scan reports | Every critical/high finding remediated or time-bounded release-owner decision |
| B Packaging | Exact per-architecture build, smoke, SBOM, scan, archive, manifest, checksums | Historical images exist but are stale and not candidate evidence | Both exact images execute and bundles validate |
| C CI/provenance | Full validation and release workflows at candidate SHA; retained artifacts; OIDC attestation verification | Not run for current candidate; GitHub configuration/credentials not approved | Successful required jobs and verifiable attestation |
| D Private rehearsal | Approved snapshot/media package; inventory/import/verify/search/backup/restore/freeze simulation | Package can be prepared; no private data accessed | Approved rehearsal passes with redacted aggregate evidence |
| E Differential | Dual-server native-v1, OpenSubsonic, Jellyfin migrated-target replay | Not run against migrated production-shaped target | Zero unexplained differences |
| F Clients/providers | MeloAmp, Android/Auto, OpenSubsonic, Jellyfin; approved OIDC/providers/MPD/MPV | Current artifact is five pending-external rows; no physical pass claimed | Required journeys pass or have explicit release-owner dispositions |
| G Readiness | Signed-off packet with backups, monitoring, roles, rollback window, candidate and security decision | Not eligible while A–F are open | Explicit write-freeze approval |
| H Cutover | Runbook-only freeze, final delta, exact image, probes, traffic switch | Not executed | Approved cutover and acceptance window complete |
| I Rollback | Controlled production rollback or accepted isolated alternative | Synthetic rollback only | Production proof or explicit release-owner acceptance |
| J Publication | Release event, GHCR manifest, SBOM, provenance, checksums, notes, immutable post-publish smoke | Not executed | All gates pass and release owner approves publication |
| K Cleanup | Named-object, topology-checked, auditable deletion after acceptance window | Not executed | Separate cleanup approval |

## Security investigation and disposition procedure

The current supplied baseline says the current amd64 image has 7 critical and 153 high findings,
with no fixed versions reported. That count is not a CVE-level current-candidate report. The
historical R5 scan and any existing local image are explicitly excluded from candidate evidence.

For each finding in the fresh exact-architecture reports, record:

| Required field | Decision record content |
|---|---|
| Identity | CVE, package, source package, installed version, severity, layer, architecture |
| Runtime purpose | Required/optional, binary or library, build-only or runtime, supported operation |
| Reachability | Remote/local-only, reachable endpoint or media path, exploit prerequisites, exposure |
| Remediation | Debian Trixie fixed-version check, accepted base digest comparison, package removal/update result, media-tool verification |
| Disposition | Remediated, not affected after proof, or time-bounded release-owner acceptance with owner, rationale, expiry, and recheck date |
| Integrity | Scan DB timestamp/digest, image digest, report checksum, and exact command |

No finding may be suppressed, downgraded, or treated as waived because it has no fixed version.
Changing away from Debian Trixie or changing the severity policy requires a superseding ADR that
remains Proposed until explicitly accepted.

## Bounded production-rehearsal approval package

The operator request must fill every value below before any private-data command is run:

| Field | Required value |
|---|---|
| Source | Exact database/snapshot identity, schema fingerprint, snapshot marker, read-only proof |
| Media | Exact absolute root and access mode; no-follow/read-only proof; path map |
| Targets | Named target database, isolated restore database, target storage/search roots |
| Capacity | Free-space budget, expected temporary bytes, expected runtime, concurrency limits |
| Secrets | Secret names only; injected through protected environment/secret store; never shell history or evidence |
| Commands | Exact `legacy inventory/plan/baseline/import/verify/search/delta/freeze`, backup/restore, and rollback commands |
| Outputs | Redacted counts, checksums, timings, issue totals, search status, backup/restore proof, client results |
| Exclusions | Private paths, names, media metadata, credentials, tokens, and catalog rows are not committed |
| Cleanup | Exact target list, retention period, confirmation, audit event, topology-drift failure behavior |
| Stop conditions | Source mutation, marker drift, unexpected schema, unresolved critical issue, backup/verify mismatch, media mutation, or differential regression |
| People | Named migration operator, data owner, release owner, client owner, and approver |

## Approval matrix

| Action | Exact target | Impact | Approver | Rollback path | Result |
|---|---|---|---|---|---|
| Fresh candidate build/scan | Disposable builder and exact image tags | CPU, disk, package DB access | Release owner | Delete only named disposable objects after evidence retention | Not executed for current candidate |
| Private rehearsal | Approved snapshot/replica and mapped media root | Read-only source; writes isolated target | Data owner + release owner | Retain target; restore isolated backup; no source mutation | Approval not granted/recorded |
| Live provider/OIDC/client acceptance | Named credentials, services, and devices | External calls, quota, account/session effects | Service/client owners | Revoke test sessions; retain redacted logs | Not executed |
| Write freeze/final delta | Exact C# deployment and source DB | Stops legacy writes; migration state changes | Approver + data owner | Abort before freeze or use approved rollback | Not executed |
| Traffic switch | Exact proxy/load balancer/DNS control | Changes production authority | Approver + on-call | Run generated rollback procedure | Not executed |
| Production rollback | Exact Rust target and C# target | Restores legacy authority | Approver | Retain failed Rust target and verify C# | Not executed |
| GHCR/GitHub publication | Exact release tag and registry package | Public artifact and release metadata | Release owner | Do not publish; no destructive registry cleanup | Not executed |
| Cleanup | Named backups/snapshots/targets/rollback assets | Irreversible deletion | Data owner + release owner | No cleanup until separate approval | Not executed |

## Local validation baseline

Executed at the recaptured clean HEAD:

- `python3 scripts/validate_repository.py` — pass.
- `python3 scripts/extract_requirements.py --check` — pass.
- `python3 scripts/validate_client_acceptance.py` — pass as a structural check: 0 pass, 0 fail,
  5 pending-external.
- `make check-fast` — pass.
- `python3 scripts/audit_requirement_overrides.py --require-terminal` — fail closed on the 13
  non-terminal requirements; this is an expected release blocker, not a waiver.

The requested `scripts/validate_r4_client_acceptance.py` is absent from this checkout. No
substitute validator is represented as that script.

## Exit decision

The phase remains **not release-accepted**. Safe local implementation and synthetic rehearsal are
complete, but current-candidate security, exact multi-architecture artifacts, remote CI/provenance,
private-data rehearsal, migrated-target differential validation, physical/external acceptance,
production cutover, production rollback, publication, and cleanup are not executed. The report may
be changed to Accepted only after the required evidence and approvals are present.

Documentation impact:
- Updated - this plan, the Phase 11 operator report, Phase 11/R5 evidence, runbook, roadmaps,
  parity/traceability records, and public migration/deployment/troubleshooting guidance.
