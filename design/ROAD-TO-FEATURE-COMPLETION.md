# Road to feature completion

## Current verdict

The Rust rewrite is functionally feature complete relative to the current C# Melodee product,
subject to the approved differences in `design/ROAD-TO-FEATURE-PARITY.md`. No outstanding
requirement represents a missing user-facing application workflow.

MeloAmp owns listener-facing web workflows. Melodee owns the server APIs, ingestion and catalog
runtime, operational tooling, and the full administrative UI.

## Completion summary

| Measure | Result |
|---|---:|
| Runtime feature completion | 100% |
| Terminal PRD requirements | 226 of 239 |
| Overall requirement completion | 94.6% |
| Non-terminal requirements | 13 |
| Missing user-facing workflows | 0 |

The distinction is intentional: functional feature completion is achieved, while production release
qualification and operator acceptance remain open.

## Outstanding release-readiness requirements

| Requirement | Qualification still required | Runtime feature missing |
|---|---|---:|
| `PRD-API-M1-004` | Verify preservation of client-visible public IDs against an authorized migrated C# dataset. | No |
| `PRD-DEPLOY-001` | Produce and verify immutable Linux `amd64` and `arm64` OCI artifacts from the exact release candidate. | No |
| `PRD-MIG-007` | Execute and evidence the authorized rollback path to the C# service and pre-migration data. | No |
| `PRD-PERF-003` | Record final release-candidate performance and PostgreSQL query measurements on representative workloads. | No |
| `PRD-PERF-004` | Accept release-candidate evidence for 200 or more concurrently available Releases with repeated termination and restart. | No |
| `PRD-SCALE-001` | Demonstrate bounded-memory catalog operation against the authorized production-scale library. | No |
| `PRD-SCALE-002` | Demonstrate indexed, bounded API and Admin UI behavior against the authorized production-scale library. | No |
| `PRD-STORAGE-004` | Verify that the authorized media library migrates without re-encoding, unnecessary movement, or byte changes. | No |
| `PRD-TEST-001` | Run relevant scenarios against the current C# service and the exact Rust candidate using migrated data. | No |
| `PRD-TEST-002` | Complete the side-by-side differential API run against that migrated dataset. | No |
| `PRD-TEST-005` | Complete remaining real-library, physical-client, and production-scale media-corpus acceptance. | No |
| `PRD-TEST-006` | Complete release and production fault-recovery evidence around migration, restart, cutover, and rollback. | No |
| `PRD-TEST-007` | Demonstrate that every durable work item completes, retries safely, or remains visibly actionable after failure. | No |

## Dependency-ordered release work

### 1. Establish the immutable candidate

- Record a clean commit and proposed version.
- Build both required architectures.
- Attach digests, SBOMs, provenance, and vulnerability results to that exact candidate.
- Resolve or explicitly waive release-blocking security findings through the accepted decision process.

### 2. Qualify migration against authorized data

- Back up the C# database and media roots.
- Import without changing the source checkout, source database, or source media.
- Verify public IDs, identities, credentials, secrets, playlists, user state, and byte preservation.
- Rebuild search and compare catalog totals and sampled semantic identities.

### 3. Run differential and client acceptance

- Run native, OpenSubsonic, and Jellyfin comparisons against the same migrated dataset.
- Record every difference as expected, approved, corrected, or blocking.
- Complete MeloAmp and supported physical-client journeys.

### 4. Qualify scale and recovery

- Capture the required performance, query, memory, and bounded-pagination evidence.
- Exercise termination, restart, retry, and durable-work visibility under representative load.
- Confirm backup restore and rollback before traffic switching is authorized.

### 5. Operator acceptance

- Obtain explicit authorization for write freeze, final delta import, traffic cutover, rollback drills,
  publication, and cleanup.
- Record exact commands, actors, timestamps, candidate digests, results, and acceptance decisions.

## Release acceptance rule

Do not describe the release as production accepted until all 13 requirements are terminal and all
operator-controlled gates have recorded evidence. This limitation does not negate functional feature
parity; it limits claims about production readiness, migration safety, and release acceptance.

## Historical records

Numbered implementation reports and accepted ADRs remain under `design/evidence/` and `design/adr/`
as immutable historical records. They are evidence, not the active project-management model.

## Documentation impact

- Updated - active planning is capability-oriented and separates runtime feature completion from
  release qualification.
