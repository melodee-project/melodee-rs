# Canonical Phase 11 Report

Status: **Not release-accepted — operator and external blockers remain** — safe local
implementation, synthetic rehearsal tooling, documentation, and fast local validation are
complete. Current-candidate security, exact multi-architecture packaging, production/private-data
execution, traffic operations, release publication, and rollback-asset cleanup remain open.

Date: 2026-08-25

## Phase objective and exit decision

The objective was to prove a repeatable, non-destructive migration from a supported C# Melodee
installation to the Rust server, preserving public identity, media references, configuration,
users, and user state, with verified backup, restore, rollback, packaging, and release operations.

The safe local portion is complete. This is not a claim of production or release completion. The
operator acceptance record is
[OPERATOR-ACCEPTANCE-REPORT.md](OPERATOR-ACCEPTANCE-REPORT.md); its approval-gated legs must be
executed by the named operator/approver before a production release can be accepted.

## Baselines and worktree state

| Repository | HEAD | Branch | Worktree at entry | Handling |
|---|---|---|---|---|
| Rust rewrite | 35272b1d344cb0c821b171c3f215243af8706c24 | main | clean before closeout edits | implementation target; final candidate recapture remains required |
| C# oracle | 4028b58dbd3302ed431e2c5b32d9dde5634db218 | sph.2026-07-11.01 | clean | read-only; not built or modified |
| MeloAmp | 096c76d810af91205aeee54da58131766b0f076b | sph.2026-07-14.01 | clean | read-only; not built or modified |

The C# and MeloAmp worktrees remain clean and untouched. Any final candidate must be captured only
after the Rust documentation closeout is complete and the Rust worktree is clean; evidence from a
dirty or subsequently changed tree is invalid.

## Entry validation and historical evidence

Passed at entry:

- python3 scripts/validate_repository.py
- python3 scripts/extract_requirements.py --check
- python3 scripts/audit_requirement_overrides.py
- python3 scripts/validate_client_acceptance.py
- make check-fast
- python3 -m unittest tests.python.test_legacy_rehearsal tests.python.test_release_evidence tests.python.test_container_smoke -v
- cargo test -p melodee-cli --test legacy --test legacy_faults --test legacy_freeze

The strict `python3 scripts/audit_requirement_overrides.py --require-terminal` check correctly
fails closed on 13 non-terminal requirements. The requested
`scripts/validate_r4_client_acceptance.py` is present as a compatibility wrapper for the
historical R4 client acceptance matrix, and it validates the committed artifact structurally
without creating physical-client evidence.

R3 is synthetic/production-shaped local importer evidence. R4 is isolated synthetic/fixture
rehearsal evidence, including the earlier 12,000-Track recovery drill. R5 is historical local
packaging/hardening evidence and is not current candidate-SHA scan or publication evidence.
Historical reports are not treated as production access, private-data rehearsal, live
provider/OIDC validation, physical-client acceptance, traffic cutover, or production rollback.

## Migration command and state-machine closure

The current melodee legacy family covers inventory, plan, baseline, import, verify, delta, freeze,
status, and the new validate command. The rollback family provides a topology-bound runbook and
confirmation-gated record audit command. The migration boundary:

- opens the C# source through a transaction-read-only session and fails closed on an unsupported
  schema fingerprint;
- records source identity, snapshot marker, counts, and read-only facts before import;
- uses durable target runs, checkpoints, leases, fencing, monotonic transitions, and idempotent
  entity mappings;
- preserves failed target data and durable issues for forensic inspection;
- redacts credentials, hashes, secrets, session keys, and private paths from output; and
- refuses a rollback audit record unless the exact run is cutover-ready, while leaving target data
  intact.

legacy validate is read-only by default. It emits a deterministic sorted plan containing counts,
asset IDs, actions, observed sizes, and SHA-256 path digests only. --fix requires --confirm and a
delta-ready run, claims the run lease, rechecks the fence for every mutation, updates only target
media_assets.byte_size when the planned identity/path/old-size predicate still matches, and
records applied/conflicted repairs. It never changes source bytes, paths, hashes, identities,
relationships, or deletes media. A repeated repair is idempotent.

## Legacy domains and field dispositions

The frozen source inventory reports 52 tables and is validated before import. The importer and
ADR-0148 per-table disposition cover the following domain groups without silent domain skipping:

| Disposition | Domains |
|---|---|
| Imported with public identity preserved | Artists and aliases; Releases/editions and dates; Tracks; MediaAssets and audio facts; contributions, memberships, and credits; external IDs/provider evidence; libraries and path-mapped media; users, supported credentials, roles, groups, and ACLs; social identities; Last.fm links; playlists and smart playlists; favorites, ratings, hates, pins, bookmarks, and history; podcasts, episodes, downloads, progress, and bookmarks; shares, requests, comments, activities, radio, and user state; playback/device settings and EQ presets where representable |
| Transformed under accepted rules | C# Album/Song storage becomes Rust Release/Track; bitrate/BPM/audio fields are represented in the Rust model; unsupported metadata is retained as reference_only; legacy paths become validated canonical-relative paths; supported legacy secrets are decrypted only in memory and re-sealed by the Rust contract |
| Regenerated from authoritative target state | Tantivy search generations and other derived projections; operational summaries and ephemeral lease/session state |
| Accepted exclusion or deferred representation | Refresh tokens and ephemeral sessions; retired Party/Jukebox behavior; unsupported/non-Last.fm provider runtime behavior under accepted ADRs; telemetry and legacy-only hooks that are regenerated or explicitly inert; unsupported password hashes become reset-required records |
| Durable issue requiring review | Malformed or orphaned references, duplicate/malformed public IDs, unsupported credentials, missing/moved/inaccessible or wrong-size media, unsafe paths, secret re-seal failures, and every other source row that cannot be represented safely |

The source inventory, import modules, legacy_migration.issues, and metadata.reference_only
records are the field-level audit path. No normalized name or filesystem path is used as semantic
identity. Accepted exclusions remain governed by ADR-0133/0140/0141, ADR-0136, ADR-0148, and
ADR-0150; no retired Party/Jukebox behavior was reintroduced.

## Identity, credentials, secrets, and media results

- Legacy ApiKey GUIDs, ShareUniqueId values, and mapped public IDs are preserved in durable
  mapping tables. Duplicate target public IDs fail as issues rather than overwriting authority.
  Repeat import reasserts the mapping and delta updates the mapped target.
- Cross-domain relationships are checked by importer verification, including Artist/Release/Track
  links, contributions, playlist ordering, and user-state ownership.
- Supported bcrypt credentials remain usable without a forced reset; the integration test verifies
  a password against the imported hash. Unsupported hashes receive an explicit reset-required
  placeholder and issue. No hash value is emitted in evidence.
- Last.fm and legacy-protected OpenSubsonic secrets are re-sealed only with the operator-supplied
  legacy key. Missing/wrong keys record an issue and create no usable secret row. Legacy encrypted
  blobs are never copied as Rust ciphertext. Refresh tokens and ephemeral sessions remain
  intentionally excluded and require reauthentication.
- Media is adopted in place. Absolute/relative safety, containment, and no-follow checks reject
  traversal, symlink components, and special files. Missing or wrong-size files become issues
  without deleting or rewriting source media. Valid hashes are retained; the ADR-0148 sentinel is
  non-authoritative for identity, deduplication, replacement, and deletion.
- The 12,000-Track drill checked 12,000 mapped media rows: 16 deterministic sparse fixture files
  were valid and 11,984 expected synthetic files were reported missing. The canonical tree
  before/after fingerprint matched and the rehearsal recorded source_mutated_by_harness=false.

## Current synthetic rehearsal evidence

Artifact: [rehearsal-drill-12000.json](rehearsal-drill-12000.json)

Command:

    python3 scripts/legacy_rehearsal.py clean
    python3 scripts/legacy_rehearsal.py drill --songs 12000 --output design/evidence/phase-11/rehearsal-drill-12000.json

Results:

| Measure | Result |
|---|---:|
| Source tables | 52 |
| Artists / Releases / Tracks / users | 500 / 1,000 / 12,000 / 50 |
| Target media references / metadata bytes | 12,000 / 12,000,000,000 |
| Total migration issues | 12,001: 12,000 expected missing-media warnings, 0 errors, and one informational run record |
| Initial import wall time / throughput | 161.79 s / approximately 74.2 Tracks/s |
| Complete measured step wall time | 181.91 s |
| Peak observed RSS | 148,865,024 bytes (search rebuild) |
| Target database size | 97,719,999 bytes |
| Search index size | 6,160,176 bytes |
| Search state | active generation: 500 Artists, 1,000 Releases, 12,000 Tracks; pending=0, failed=0 |
| Search query validation | six exact/normalized/Unicode/punctuation/broad cases; p50 100 ms, p95 250 ms |
| Backup | 7,239,515 bytes, SHA-256 recorded, 1,017 TOC entries |
| Isolated restore | 500 / 1,000 / 12,000 / 50 counts matched; media metadata matched |
| Freeze | cutover-ready, acknowledgement recorded, 3,990 ms stillness window |
| Rollback rehearsal | failed target retained, source/media unchanged by harness, durable synthetic audit recorded |

The fault-oriented migration suites also pass: 13 legacy tests, 7 legacy_faults tests, and 4
legacy_freeze tests. They cover schema rejection, source read-only enforcement, public-ID
conflicts, orphan issues, stale-lease recovery, fencing, restart convergence, freeze drift,
media traversal, and repair confirmation/idempotency.

## Backup, restore, search, and rollback

The rehearsal validates a PostgreSQL custom dump, checksum and TOC inspection, isolated restore,
catalog/media count comparison, search rebuild from PostgreSQL, and search query probes. Tantivy
files are never treated as migration authority. The rollback runbook names the Rust target, legacy
target, traffic target, expected impact, confirmation, health probes, retained forensic target,
and durable audit command. It never runs a proxy, process-name search, traffic switch, or
production rollback itself.

The disposable rollback path passed with a retained failed Rust target and an audit record. No
production database, snapshot, media root, write freeze, final delta, traffic switch, rollback, or
rollback-asset deletion was executed.

## Differential and real-client acceptance

The accepted Phase 7 native-v1, Phase 8 OpenSubsonic/Jellyfin, MeloAmp, Android, and Feishin
evidence remains valid for its frozen fixtures and stated environments. This Phase 11 run did not
build or modify the C# or MeloAmp repositories and did not run a new dual-server migrated-data
differential capture, physical-device journey, live OIDC/provider test, or production-sized
client acceptance. Those remain release blockers; existing approved differences were not changed
to force migration fixtures to pass.

## Packaging and release readiness

No current-candidate image was built in this closeout. The existing local amd64 image has label
revision `phase11-local` and ID
`sha256:1ba0b659c8c80dcb2e6bc579cda08ba7459e1a4795ae48ea733133062afd18f6`; it predates the
current HEAD recapture and is stale/non-candidate evidence. Historical arm64 images are likewise
not current-candidate evidence. Host Syft and Trivy binaries are absent; cached tool containers
were version-observed but no exact candidate scan was run. Therefore this report records no
current SBOM, Trivy database timestamp/digest, image digest pair, scan-policy pass, provenance,
GHCR publication, GitHub Release, or post-publication smoke result.

The release workflow remains release-published-only and defines the required amd64/arm64, SPDX,
Trivy, exact-scanned-image, checksum, and provenance gates. No production Compose deployment was
started.

## Approval-gated actions and blockers

The following were not executed and remain blockers until separately approved at execution time:

1. Access to a production C# database, snapshot, replica, or media library.
2. Production-sized private-data rehearsal and isolated production restore.
3. C# write freeze, final production delta, and traffic switch.
4. Production rollback and post-rollback legacy-authority verification.
5. Live provider, Google OIDC, registry, deployment, MPD/MPV, physical Android Auto, and other
   external credentials/services/hardware.
6. Current release-candidate scan remediation/acceptance, arm64 CI execution, provenance
   attestation, GHCR/GitHub publication, and release-owner signoff.
7. Deletion of temporary imports, backups, snapshots, or rollback assets.

## Requirements, ADRs, and compatibility impact

- MIGRATE-001 and MIGRATE-002: safe local implementation and synthetic recovery are current;
  production rehearsal/cutover/rollback remain in progress.
- PACKAGING-001: local amd64 build/smoke and workflow validation are current; current scan
  policy, arm64, provenance, publication, and owner acceptance remain open.
- Applicable OPS-001: local diagnostics, correlation, durable issues, readiness distinction,
  search status, rollback audit, and runbook evidence are current; deployment and external-client
  legs remain open.
- PRD-MIG-001..006, applicable deployment/observability/security/test requirements, and the
  release workflow's local obligations are recorded in status-overrides.json. PRD-MIG-007 remains
  in progress because production rollback is not proven.
- Applied accepted decisions: ADR-0144, ADR-0148, ADR-0149, ADR-0150, ADR-0151, ADR-0133/0140/0141,
  ADR-0136, and ADR-0161. No new ADR or public compatibility-contract change was needed.

No native-v1, OpenSubsonic, or Jellyfin wire DTOs/routes were changed by this phase. No source
media was moved, renamed, rewritten, transcoded, deleted, or used as semantic identity. No schema
change was introduced; the validation repair only updates target media-size metadata under the
existing migration run contract.

## Validation commands

Current closeout results:

    python3 scripts/validate_repository.py                         # pass
    python3 scripts/extract_requirements.py --check                 # pass
    python3 scripts/validate_r4_client_acceptance.py                # pass structurally; 5 pending-external
    python3 scripts/validate_client_acceptance.py                   # pass structurally; 5 pending-external
    make check-fast                                                 # pass
    make test-integration                                             # pass; workspace PostgreSQL/integration matrix
    cargo deny check                                                  # pass; policy checks with non-failing warnings
    python3 scripts/audit_requirement_overrides.py --require-terminal # fail closed on 13 non-terminal requirements

The full `make check`, PostgreSQL production rehearsal, exact candidate image build, exact
amd64/arm64 scan, remote workflows, and docs-site release gate were not rerun after the final
candidate documentation changes. Historical reports may contain successful older runs, but those
are not current-candidate evidence.

## Readiness for comprehensive parity audit

The repository is ready for the final comprehensive parity audit of the safe local migration and
release surface. The audit must carry forward the explicit production/private-data, current scan
policy, arm64/provenance/publication, differential/client, traffic, rollback, and cleanup blockers;
none may be silently converted into completed evidence.

Documentation impact:
- Updated - design/ROAD-TO-FEATURE-COMPLETION.md; design/ROAD-TO-FEATURE-COMPLETION.md; design/evidence/phase-11/PHASE-11-REPORT.md; design/evidence/phase-11/OPERATOR-ACCEPTANCE-REPORT.md; design/ROAD-TO-FEATURE-COMPLETION.md; design/ROAD-TO-FEATURE-PARITY.md; design/requirements/FEATURE-PARITY-MATRIX.md; design/requirements/status-overrides.json; design/runbooks/CUTOVER-AND-ROLLBACK.md; docs/src/content/docs/installation/legacy-migration.md; docs/src/content/docs/installation/database.md; docs/src/content/docs/installation/upgrading.md; docs/src/content/docs/installation/docker.md; docs/src/content/docs/troubleshooting/index.md.
