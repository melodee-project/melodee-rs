# Phase 3 Report — Media inspection, normalization, and canonical publication

**Status:** Final — Phase 3 complete (2026-08-10)
**Date:** 2026-08-10
**Authority:** PRD-INGEST-005 through PRD-INGEST-010, PRD-MEDIA-001 through PRD-MEDIA-006,
PRD-FP-001 through PRD-FP-005, PRD-STORAGE-001/002/004, SPEC sections 11–16,
ADR-0105/0106/0108/0110/0115/0116, and `design/agents/DEFINITION-OF-DONE.md`.

## Executive summary

Phase 3 implements the complete durable media pipeline from a settled Release candidate through
read-only inspection, normalization/fingerprinting, structural validation, and convergent canonical
publication:

```text
Ready -> Inspecting -> Normalizing -> Validating -> Staged -> Publishing -> Published
                                      \-> NeedsReview
```

The result is a complete canonical MP3/artwork bundle plus Release manifest v3. Every Published item
has one immutable UUIDv7 Release ID, a durable structural-publication authorization, an immutable
publication plan, a portable Release-relative target/manifest path, append-only journal evidence,
one completion receipt, and source disposition `retained`. ADR-0145 later supersedes only the
historical inbound-retention interpretation: staged source is retained, while acquired inbound
candidates are drained. Artist/catalog identity, classification,
novelty policy, source deletion, and catalog insertion remain explicitly outside Phase 3.

## Sub-slice decision

| Sub-slice | Result | Evidence |
| --- | --- | --- |
| 3.1 inspection and manifest v1 | Complete | [Media inspection and manifest](media-inspection-and-manifest/README.md) |
| 3.2 normalization and manifest v2 | Complete | [Media normalization](media-normalization/README.md) |
| 3.3 structural validation and canonical publication v3 | Complete | [Canonical publication](canonical-publication/README.md) |
| 3.4 representative/pathology/fault/200-Release integration gate | Complete | [Canonical publication acceptance evidence](canonical-publication/README.md#phase-34-acceptance-evidence) |

## Exit criteria

### Representative media and pathologies

The real-tool vertical test carries MP3, FLAC, OGG/Vorbis, and WAV candidates through Published. It
also covers Unicode and leading-dash names, external artwork normalization, CUE/NFO/M3U/LRC/SFV
sidecars, and an outbound symlink that is not followed. The earlier Phase 3.2 matrix additionally
covers AAC/M4A, retained VBR/CBR MP3, multichannel downmix, corrupt audio, timeout, cancellation,
malformed child output, and source changes.

Status: **satisfied.**

### Manifest handoff without audio reread

Manifest v3 retains v1 inspection facts and v2 normalization/fingerprint/artwork/sidecar facts, then
adds structural validation, stable Release identity, durable authorization, retained source
disposition, layout/plan identity, and the exact sorted source-to-normalized-to-canonical asset map.
Phase 4 can insert the catalog from this contract without reopening audio merely to rediscover tags,
technical properties, hashes, fingerprints, or normalized asset facts. Phase 4 still owns semantic
Artist/Release/Track identity resolution and is required to consume the reserved Release ID.

Status: **satisfied for the Phase 3 handoff.** The broader PRD-INGEST-007 remains `in-progress`
until the Phase 4 consumer proves the end-to-end catalog contract.

### Crash/retry and source safety

Canonical final directories are absent or complete. All files and manifest v3 are copied, streamed
and hash/size verified, sealed, fsynced, and then exposed by a no-replace directory rename. A retry
resumes a matching hidden incoming directory or adopts an exact final; it never overwrites a
different final. A crash after rename but before PostgreSQL completion converges to exactly one
receipt and Published transition. Lease fencing prevents stale completion. Source snapshots remain
identical across success, validation failure, tamper, cancellation, conflict, lease loss, recovery,
pathology, and the 200-Release scenario.

Deterministic injection covers asset fsync, manifest fsync, sealed-tree fsync, final rename, and
post-rename parent fsync. Pre-rename failures expose no final and succeed on retry. The post-rename
parent-fsync failure leaves a complete final, which retry verifies and adopts. Combined with the
database-completion crash test, this exercises both sides of the filesystem/PostgreSQL convergence
boundary.

Status: **satisfied.**

## 200-Release closure

The normal, non-ignored PostgreSQL/real-media vertical test asserts:

- 200 normalized, structurally validated, and Published work items;
- 200 durable authorizations, immutable plans, distinct Release IDs, receipts, Published
  transitions, final journal outcomes, and canonical manifests;
- zero expected/unexpected failure, lost item, duplicate authoritative result, partial incoming or
  `.copying` file, and source mutation;
- normalization concurrent execution reaches but never exceeds its configured bound; and
- validation and publication use independent bounded claim/execution pools, each configured to 8.

Focused vertical tests also pass exact-final adoption after rename-before-database-completion,
conflicting-final no-clobber/direct reclaim, durable `NeedsReview` routing before canonical I/O, and
the real MP3/FLAC/OGG/WAV + Unicode/artwork/sidecar matrix. A feature-gated deterministic lease-theft
test additionally proves stale-copy cancellation and direct retry convergence.

Exact observed output:

```text
phase3.4-acceptance={"total":200,"normalized":200,"validated":200,"published":200,"expected_failure":0,"unexpected_failure":0,"lost":0,"duplicate_authoritative_result":0,"source_mutations":0,"publication_receipts":200,"configured_normalization_concurrency":8,"measured_normalization_concurrency":8,"configured_validation_concurrency":8,"configured_publication_concurrency":8,"elapsed_ms":23177}
```

The complete 11-test normalization binary took 78.16 seconds including database lifecycle and all
earlier pipeline stages. No performance threshold is inferred from this development-host
observation.

## Architecture and durable data

- ADR-0115 keeps third-party media tools behind Melodee-owned observation/manifest contracts and
  defines semantic versus byte hashing.
- ADR-0116 fixes immutable Release IDs, the lowercase ID/hash-derived layout, non-destructive
  structural authorization, publication-plan hashing, no-clobber atomic finalization, and retry
  adoption.
- PostgreSQL remains authority for workflow state and intent; the filesystem is authority for the
  finalized media bytes. The journal/receipt protocol reconciles their intentionally non-atomic
  boundary.
- Additive migration `0006_canonical_publication.sql` creates authorization, plan, journal, and
  receipt tables, their constraints/indexes, and publication-state lease invariants. It does not
  alter or delete canonical/source media.
- Operational rollback retains the additive evidence tables. Do not drop publication evidence to
  downgrade; restore a verified database backup if a full schema rollback is required.

## Dependency and security decision

ADR-0116 records `rustix` 1.1.4 as the narrow FOSS dependency needed for Unix no-follow opens and
kernel-enforced no-replace directory rename on Rust 1.97. It is pinned centrally, represented in
`Cargo.lock`, uses safe APIs, and avoids local `unsafe` and an unsafe check-then-rename race.

Root resolution requires pairwise-disjoint inbound, work, and storage roots. Traversal,
non-portable paths, symlink components, non-regular files, cross-attempt provenance, hash/size
mismatch, malformed v3/plan facts, and conflicting finals fail closed. Operator-safe errors and
durable journal facts do not contain database URLs or secret values.

## Operations and observability

The server starts validation/publication loops only when ingestion is enabled and a canonical
storage root is configured. Both loops are bounded, leased, heartbeat-aware, independently
cancellable, and joined during graceful shutdown. CLI commands run one bounded validation or
publication batch and inspect manifest/publication evidence. The Rust-native `/readyz` endpoint is
an additive bounded PostgreSQL readiness check; `/healthz` remains dependency-free liveness.

PRD-INGEST-010 is supported through durable work/event/publication evidence, CLI diagnostics, and
the Admin diagnostics API. PRD-INGEST-009 truthfully remains `in-progress`: Phase 3 bounds
inspection, normalization, validation, and publication, but the full requirement calls for more
independent resource classes (including later enrichment, catalog writes, and search indexing).

## Compatibility impact

No native Melodee API v1, OpenSubsonic, or Jellyfin route, request/response DTO, public identifier,
status, error envelope, authentication flow, pagination, range behavior, or intentional negative
behavior changed. `/readyz` and the publication CLI are additive Rust-native operational surfaces,
not compatibility adapters. No legacy C# service/class boundary was copied.

## Requirements disposition

| Requirement | Closure status | Reason |
| --- | --- | --- |
| PRD-INGEST-005 | superseded in part | ADR-0145 preserves byte immutability before acquisition and replaces inbound retention with durable staging handoff/drain. |
| PRD-INGEST-006 | tested | Copy/verify/fsync/no-replace finalize, exact-final adoption, conflict protection, and idempotent durable completion are implemented and tested. |
| PRD-INGEST-007 | in-progress | Manifest v3 satisfies the Phase 3 handoff, but the Phase 4 catalog consumer has not yet proved full end-to-end completeness. |
| PRD-INGEST-008 | tested | Structural/media validity is independent of external enrichment and unresolved catalog identity. |
| PRD-INGEST-009 | in-progress | Phase 3 pools are bounded; all later resource classes named by the broad requirement do not yet exist. |
| PRD-INGEST-010 | tested | Work state/history/worker/error and publication decision evidence are visible through Admin API/CLI; the UI arrives later. |
| PRD-STORAGE-001 | tested | Canonical layout contains no Artist-derived component; Artist rename/merge/split cannot determine media paths. |
| PRD-STORAGE-002 | tested | ADR-0116 accepts and tests the exact immutable-ID-centric layout. |
| PRD-STORAGE-004 | in-progress | Existing MP3 can be retained during new ingestion, but Phase 11 registration of an existing canonical library without movement/re-encoding remains unimplemented. |

## Validation record

Documentation-only validation performed while drafting this report:

```text
python3 scripts/validate_repository.py
Repository validation passed (17 ADRs, 8 mockups, 232 requirements).

python3 scripts/extract_requirements.py --check
Requirements index is current (232 requirements).
```

Final implementation gates:

```text
cargo test -p melodee-ingestion --lib
24 passed; 0 failed

cargo test -p melodee-ingestion --test normalization -- --nocapture
11 passed; 0 failed; finished in 78.16s

publication_lease_theft_cancels_stale_copy_and_direct_retry_converges (feature-gated focused run)
PASS — finished in 2.53s

cargo fmt --all && ./scripts/check.sh
PASS (exit 0) — 17 ADRs, 8 mockups, and 232 requirements validated; requirements index current;
111 Python tests passed; rustfmt and workspace all-target/all-feature Clippy passed; workspace
all-feature tests passed, including ingestion normalization 12/12 (controlled lease theft and 200
Published) and publication 12/12; cargo-deny advisories, bans, licenses, and sources all passed.
Only pre-existing duplicate-version and license-not-encountered warnings were emitted.

cd docs && pnpm check
PASS — Astro checked 4 files with 0 errors, warnings, or hints

cd docs && pnpm check:links
PASS — all relative content links and assets are valid

cd docs && pnpm build
PASS — 35 pages built and all internal links validated (exit 0)
```

## Remaining later-phase work

- Phase 4 catalog/identity/contribution schemas and set-based manifest consumer.
- Phase 5 general policy, novelty, quarantine/ignore/delete decisions, and deletion audit.
- Separate resource pools for later enrichment, catalog writes, and search indexing.
- Phase 11 existing-library registration/migration without mandatory relocation or re-encoding.
- CUE-driven audio splitting, broader legacy sidecar parity, optional global physical deduplication,
  and conservative stale work-attempt cleanup.

Documentation impact:
- Updated — `docs/src/content/docs/getting-started/concepts.md`,
  `docs/src/content/docs/installation/configuration.md`,
  `docs/src/content/docs/installation/storage.md`,
  `docs/src/content/docs/library-management/inbound-processing.md`,
  `docs/src/content/docs/admin/system-diagnostics.md`,
  `docs/src/content/docs/api/admin-api.md`, and
  `docs/src/content/docs/troubleshooting/index.md`; internal architecture, runbook, phase evidence,
  status, changelog, and traceability are updated in the same change.
