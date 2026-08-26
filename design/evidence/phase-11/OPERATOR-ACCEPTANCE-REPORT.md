# Phase 11 Operator Acceptance Report

Status: **Not release-accepted — explicit operator blockers remain**  
Report date: 2026-08-25  
Candidate: **not yet established after the final documentation closeout**

This report is deliberately conservative. It separates executed local checks from planned,
approval-gated, and unavailable work. Historical R3/R4/R5 evidence is linked for context but is
not reclassified as proof for the current candidate SHA.

## Recaptured repositories

| Repository | HEAD | Branch | Worktree at recapture | Handling |
|---|---|---|---|---|
| Rust rewrite | `35272b1d344cb0c821b171c3f215243af8706c24` | `main` | clean before this closeout | implementation target |
| C# oracle | `4028b58dbd3302ed431e2c5b32d9dde5634db218` | `sph.2026-07-11.01` | clean | read-only; untouched |
| MeloAmp | `096c76d810af91205aeee54da58131766b0f076b` | `sph.2026-07-14.01` | clean | read-only; untouched |

Cargo metadata agrees on `melodee-server=0.1.0` and `melodee-cli=0.1.0`. The proposed release
tag is `v0.1.0`; no GitHub Release or registry tag exists as current publication evidence.

## Executed local evidence

| Check | Result | Boundary |
|---|---|---|
| `python3 scripts/validate_repository.py` | Pass | Repository/doc structure only |
| `python3 scripts/extract_requirements.py --check` | Pass | Requirement index integrity only |
| `make check-fast` | Pass | Fast local gate; not PostgreSQL/full release/remote evidence |
| `python3 scripts/validate_r4_client_acceptance.py` | Pass structurally: 0 pass, 0 fail, 5 pending-external | Historical wrapper alias for the R4 client matrix; does not create physical-client evidence |
| `python3 scripts/validate_client_acceptance.py` | Pass structurally: 0 pass, 0 fail, 5 pending-external | Does not create physical-client evidence |
| `python3 scripts/audit_requirement_overrides.py --require-terminal` | Fail closed on 13 non-terminal requirements | Correctly blocks release acceptance |
| `cargo fmt --all`, Rust unit/library tests | Included in fast gate and pass | Does not prove production migration |
| `scripts/validate_r4_client_acceptance.py` | Present | Historical wrapper alias validates the committed matrix structurally only |

No C# or MeloAmp command was run. No production endpoint, registry, external provider, private
database, private media root, physical device, traffic controller, or GitHub configuration was
contacted.

## Candidate and packaging disposition

Candidate establishment is intentionally deferred until this documentation and the final local
state are closed. The existing local image is not candidate evidence:

| Item | Observation | Disposition |
|---|---|---|
| Existing local image | amd64, ID `sha256:1ba0b659c8c80dcb2e6bc579cda08ba7459e1a4795ae48ea733133062afd18f6`, label revision `phase11-local` | Stale/non-candidate; created before the current HEAD recapture |
| Existing arm64 image | Historical R5 image only | Stale/non-candidate; no current-SHA execution |
| Runtime principal | Existing image reports `melodee:melodee`; Dockerfile defines UID/GID 10001 | Local design evidence only |
| Candidate build | Not run for this closeout | Requires exact candidate selection and image build |
| SBOM/scan | No current-candidate SBOM or Trivy report | Host Syft/Trivy binaries are absent; cached tool images are not a substitute for an exact candidate run |
| Provenance | None | Workflow configuration is not an attestation |
| Publication | None | Release owner approval and passing workflow required |

Tool versions observed locally: Rust/Cargo `1.97.1`; Docker Engine `29.7.2`; BuildKit `0.32.2`
through Docker Buildx `0.36.1`; Syft container `1.51.0`; Trivy container `0.72.0`; FFmpeg,
ffprobe, and fpcalc are installed. The exact Trivy database timestamp/digest for a current
candidate is not available because that scan was not run.

## Security disposition

The assignment baseline reports 7 critical and 153 high findings for the current amd64 image and
states that no fixed versions were reported. This report does not convert that statement into a
pass: no exact current-candidate amd64/arm64 JSON reports were generated, so no per-CVE decision
record can truthfully claim installed package, layer, reachability, Debian fixed-version, or
recheck evidence.

The historical R5 reports contain different counts and older image/revision identities. They are
retained as historical findings only. No finding was suppressed, filtered, downgraded, or waived.
The critical/high policy therefore remains blocking, and no release-owner security acceptance has
been recorded.

## Migration and private-data rehearsal

The synthetic 12,000-Track drill remains valid as isolated tooling evidence: it exercised source
read-only checks, public-ID mapping, durable issues, search rebuild, backup/restore, freeze state,
and retained-target rollback. It does not prove a production-sized rehearsal.

The bounded approval package is prepared in
`PRD.md`. It requires exact source/snapshot identity,
read-only proof, media access mode, target locations, capacity, protected secret injection,
commands, redacted outputs, cleanup, stop conditions, and named operator/approver. No production
or private snapshot/media access occurred, so source inventory, initial import, delta, freeze,
isolated restore, and before/after mutation checks remain unexecuted.

## Differential and external acceptance

No dual-server run was executed against a migrated production-shaped target. Native-v1,
OpenSubsonic, and Jellyfin must be re-run with matching migrated identities and every difference
classified; an unexplained difference remains a blocker. Existing fixture evidence and historical
reports do not satisfy this requirement.

The committed client artifact remains `pending-external` for all five rows. No physical Android
Auto, live Google OIDC, live provider quota, production replica warmup, MPD daemon, MPV device, or
Windows named-pipe result is claimed. MeloAmp and the C# oracle were not modified.

## Cutover, rollback, publication, and cleanup

| Gate | Result |
|---|---|
| Readiness packet | Not signable while security, candidate packaging, rehearsal, differential, and client gates are open |
| C# write freeze/final delta | Not executed; approval required |
| Traffic switch | Not executed; approval required |
| Production rollback | Not executed; isolated synthetic rollback is not promoted to production proof |
| GHCR/GitHub Release | Not executed; no provenance or public artifact exists |
| Post-publication smoke | Not applicable |
| Cleanup | Not executed; backups, snapshots, targets, and rollback assets require named approval |

The authoritative commands and fail-closed boundaries remain in
`design/runbooks/CUTOVER-AND-ROLLBACK.md`. No production command was improvised.

## Requirement and ADR disposition

- `MIGRATE-001`: safe local tooling and synthetic evidence exist; production/private-data
  rehearsal and migrated-target acceptance remain open.
- `MIGRATE-002`: isolated backup/restore/search/freeze/rollback evidence exists; production
  cutover and rollback proof remain open.
- `PACKAGING-001`: Dockerfile/workflow and historical local smoke evidence exist; exact current
  candidate multi-architecture execution, SBOM, scan policy, provenance, and publication remain open.
- Applicable `OPS-001`: local diagnostics/runbook evidence exists; production topology and
  external client/provider monitoring remain open.
- ADR-0144, ADR-0148, ADR-0149, ADR-0150, ADR-0151, and ADR-0161 remain the applicable decisions.
  No new security waiver ADR was created because no release-owner decision was supplied.

## Truthful exit decision

Phase 11 is **not release-accepted**. The safe implementation portion is complete, but the exit
criteria requiring current-candidate security disposition, exact amd64/arm64 evidence, provenance,
private-data rehearsal, zero-unexplained migrated differential results, required real-client
acceptance, production cutover or approved blocker disposition, production rollback proof or
approved alternative, publication, and post-acceptance cleanup are not satisfied.

- Updated - operator plan/report, Phase 11/R5 evidence, runbook, roadmap/parity/traceability
  records, and public migration/deployment/troubleshooting guidance.
