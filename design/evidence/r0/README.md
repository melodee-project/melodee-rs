# R0 — Corrective baseline and durable staging closure

**Status:** Complete (2026-08-16)
**Baseline commit:** `a5ba0e88fdb7cb3a383d6a992f5b3738753440f7` (release-closure plan baseline)
**Authority:** ADR-0145 (durable staging), ADR-0115, ADR-0116 (inbound-disposition clause,
partially superseded), ADR-0105, ADR-0106; `IMPLMENTATION_OUTSTANDING.md` section 5.
**Requirements:** PRD-INGEST-005, PRD-INGEST-009 (R2 owns the remaining bounds), PRD-STORAGE-004
(R0 portion), PRD-TEST-005/006/007 (R0 fault-suite portion).

This is the release-closure evidence for the owner-approved lifecycle:

```text
Inbound -> durable Staging -> attempt-isolated Work -> Review/Quarantine -> Canonical Storage
```

R0 is a closure phase, not an official numbered implementation phase; the historical Phase 0–10
reports remain untouched except for explicit ADR-0145 supersession notes.

---

## 1. Work package summary

| Work package | Result | Evidence |
|---|---|---|
| R0.1 Correct stale inspection tests | Complete | `crates/melodee-ingestion/tests/inspection.rs`; §3 |
| R0.2 Audit the handoff state machine | Complete | §4 audit matrix |
| R0.3 Same-filesystem handoff acceptance | Complete | `two_hundred_release_acquisition_drains_inbound_and_stays_drained`; §5 |
| R0.4 Cross-filesystem and crash convergence | Complete | test-only hook + 8-boundary crash matrix; §6 |
| R0.5 Adversarial safety tests | Complete | §7 |
| R0.6 Configuration, containers, diagnostics | Complete | §8 |
| R0.7 Requirement and evidence correction | Complete | §9 and the file list in §11 |

## 2. Environment

- Host: Linux x86_64 development machine; dev PostgreSQL 18 container on host port 5433
  (`compose.dev.yaml`).
- Toolchain: Rust 1.97.1 (pinned in `rust-toolchain.toml`); FFmpeg/FFprobe 8.1.2; fpcalc present.
- All tests run with `cargo test --workspace --all-features` semantics; the ingestion
  `test-support` feature carries the fault-injection surface and exists in no default build.

## 3. R0.1 — stale inspection tests

The two stale pre-staging assumptions were corrected (landed in `a5ba0e8`, verified again here):

- `script_gate_deny_retains_staged_candidate_for_retry`
  (`crates/melodee-ingestion/tests/inspection.rs:164`): a `directoryProcessingStart` denial after
  durable acquisition records a retryable `Failed` work item, publishes no manifest, retains the
  staged source directory, and proves the acquired candidate no longer exists in inbound.
- `source_change_during_inspection_discards_manifest`
  (`crates/melodee-ingestion/tests/inspection.rs:216`): the fixture mutates the *owned staged
  source* located through the deterministic staging resolver, and asserts the manifest is
  discarded with the item failed out of the pass. Mutation during cross-filesystem acquisition
  itself is covered by the handoff fault suite (§6/§7).

Command and result:

```text
cargo test -p melodee-ingestion --test inspection
test result: ok. 5 passed; 0 failed; 0 ignored — finished in 71.11s
```

## 4. R0.2 — handoff state machine audit

Audit of `crates/melodee-ingestion/src/acquisition.rs`,
`crates/melodee-persistence/src/work_items.rs`, and migration
`0031_durable_inbound_staging_handoff.sql` against every R0.2 bullet:

| Requirement | Verdict | Where |
|---|---|---|
| Durable persistence distinguishes intent, physical staging finalization, durable receipt, inbound removal, and completed bookkeeping | Yes — monotonic `intent -> staged -> inbound_removed` status plus `intended_at`/`staged_at`/`inbound_removed_at` and a receipt CHECK constraint (`tree_hash`/`method` required once `staged`) | migration 0031; `begin_staging_handoff`, `record_staging_receipt`, `record_inbound_removed`, `record_inbound_replaced` |
| Every mutation fenced by current work-item ownership and lease generation | Yes — every handoff mutation runs in one transaction with `SELECT ... FOR UPDATE` + `ensure_active_lease` | `work_items.rs` handoff methods; proven hostile in `stale_worker_losing_lease_during_copy_cannot_commit` |
| Deterministic final path relative to configured staging root, derived from Melodee-owned work-item identity | Yes — `releases/<work-item-id>/source` under `MELODEE__INGEST__STAGING_ROOT` | `acquire_owned_source`; asserted in `adversarial_candidate_names_acquire_cleanly` |
| Source library ID, name, source-root identity, and original relative path retained as provenance without configured absolute roots in semantic identity | Yes — work item keeps `source_library_id`/`source_path`; the handoff row stores only the relative staging path and occurrence generation | `release_work_items`, `release_work_item_staging_handoffs` |
| A completed occurrence releases its uniqueness reservation on the historic inbound path | Yes — `inbound_active = false` at removal/replacement; reconcile suppression (`inbound_occurrence_is_acquired`) is generation-scoped | `finish_inbound_claim`; `delayed_cleanup_after_path_reuse_preserves_new_occurrence` |
| A second occurrence at the same relative path always creates a distinct acquisition | Yes — proven end to end | `delayed_cleanup_after_path_reuse_preserves_new_occurrence` (two work items, two handoffs, same path) |
| One authoritative resolver rule: inbound before acquisition, staging after durable acquisition; no scattered conditional path construction | Yes — `acquire_owned_source` (pre-acquisition) and `owned_source_root` (post-acquisition only; refuses intent/unverified/absent receipts) are the only resolvers. Validation/publication consume manifest/work-area paths; the Admin review preview reads staging-only through the manifest with containment revalidation (no inbound fallback anywhere) | `acquisition.rs`, `inspect.rs`, `normalize.rs`, `admin_review.rs` |
| `NeedsReview`, `Quarantined`, `Failed`, and `Published` work retains staged source | Yes | `script_gate_deny_retains_staged_candidate_for_retry` (Failed), `needs_review_and_quarantined_retain_staged_source_across_restart`, `two_hundred_releases_reach_published_once_with_zero_loss_and_bounded_concurrency` (Published; staged hashes unchanged after publish) |
| No uncertain path calls staging cleanup; `Deleted` semantics not broadened | Yes — the only production recursive removals are `cleanup_inbound` (contained direct child of an inbound root, generation + tree-hash fenced) and `remove_partial` (contained hidden attempt scratch whose deterministic name embeds this work item's id). Nothing deletes staged source trees; nothing new enters `Deleted` | `acquisition.rs:638`, `acquisition.rs:794` |

No code change resulted from the audit; the state machine already implements the ADR-0145
contract. The audit is repeated as executable assertions in §5–§7.

## 5. R0.3 — same-filesystem handoff acceptance

Test: `two_hundred_release_acquisition_drains_inbound_and_stays_drained`
(`crates/melodee-ingestion/tests/acquisition.rs`). 200 Releases under one inbound root on the
same filesystem as staging; nested `disc 01/` structure per Release so the byte-for-byte proof is
recursive.

Assertions:

- Intent persisted before filesystem change (`begin_staging_handoff` precedes all I/O — crash
  matrix §6 proves the ordering by boundary).
- No-clobber rename primitive (`secure_fs::rename_noreplace`, fd-walked, kernel-enforced
  no-replace); receipt method recorded per item.
- Each final staging directory equals its source fixture recursively, byte-for-byte
  (path→BLAKE3 maps compared per Release).
- All 200 inbound candidate directories are gone after durable acquisition (`read_dir` count 0).
- 200 work items, 200 handoff rows, 200 distinct work-item ids, 200 distinct staging paths, 200
  `inbound_removed` receipts with tree hashes, 200 with method `rename`.
- Second reconciliation instrumented: `candidates_seen = 0`, `new_work_items = 0`,
  `missing_candidates = 0`.
- One new Release afterward: exactly one new work item, one new handoff (201 total), inbound
  empty again.

Recorded evidence (development machine; not a production SLA):

```text
r0-same-fs-acceptance={"releases":200,"files":667,"bytes":4145,"handoff_method":"rename",
"wall_ms":12370,"second_scan_candidates":0,"new_arrivals_acquired":1}
```

The 200-Release full-pipeline vertical
(`two_hundred_releases_reach_published_once_with_zero_loss_and_bounded_concurrency`,
`tests/normalization.rs`) additionally now asserts the drained inbound root, 200 complete
`inbound_removed` receipts after inspection, and an empty post-publication reconciliation
(`candidates_seen = 0`, `new_work_items = 0`) — proving acquisition, drain, downstream
processing, and rescan in one scenario (94.2s wall on the evidence host).

## 6. R0.4 — cross-filesystem and crash convergence

CI hosts have one filesystem, so the copy path is exercised through a test-only injection: the
`test-support`-only `AcquisitionTestHook::forcing_copy()` makes `stage_source` take the exact
EXDEV branch without any production switch (the field is absent from `IngestionConfig` unless the
feature is enabled). The same hook injects one-shot faults at durable boundaries and provides a
copy-window barrier.

Verified behavior of the forced copy path (`forced_cross_filesystem_acquisition_verifies_and_drains`
plus the crash matrix): intent persisted first; copy into hidden `.<work-item-id>.acquiring`
directory; streamed BLAKE3 tree hash before and after copy with 1 MiB bounded buffers; source
unchanged across the copy or fail-closed; file fsync plus directory syncs; atomic no-replace
finalize; durable receipt before inbound removal; inbound removal revalidates containment, lease,
occurrence identity, and receipt hash; removal recorded durably (`inbound_removed`,
`inbound_active = false`).

Crash matrix (`crash_boundaries_retry_and_converge`): each boundary crashed once, then retried.

| # | Crash boundary | State after crash (asserted) | Retry convergence (asserted) |
|---|---|---|---|
| 1 | before intent | no handoff row; no staging tree; inbound intact | clean acquire |
| 2 | after intent | `intent`, no receipt; no final tree; inbound intact | acquire completes |
| 3 | during copy | `intent`; no final tree; inbound intact | stale hidden partial removed; recopy |
| 4 | after copy, before verification | `intent`; no final tree | recopy through removed partial |
| 5 | after verification, before finalization | `intent`; no final tree | recopy and finalize |
| 6 | after finalization, before receipt | `intent`; complete final tree present | adopt verified final; no overwrite |
| 7 | after receipt, before inbound removal | `staged` with receipt; inbound present | receipt verifies; drain completes |
| 8 | after removal, before bookkeeping | `staged`; inbound physically gone; `inbound_active` still true | `AlreadyAbsent` cleanup records `inbound_removed` idempotently |

Every retry asserted: no loss (tree equals original), no overwrite (final adopted, never
replaced), no duplicate acquisition (exactly one handoff row per work item), no partial
authoritative final tree, no hidden attempt scratch left, inbound drained.

```text
r0-crash-matrix={"boundaries":8,"retried":8,"duplicates":0}
```

## 7. R0.5 — adversarial safety

All tests in `crates/melodee-ingestion/tests/acquisition.rs` unless noted.

| Scenario | Test | Result |
|---|---|---|
| Matching existing final staging tree | `final_staging_tree_adoption_and_conflict_fail_closed` | adopted, receipt recorded, inbound drained; no overwrite |
| Mismatching final staging tree | same test, second half | `IncompleteReceipt` conflict; foreign tree byte-identical afterward; source retained; no receipt |
| Source mutation during copy (add / byte-change / remove) | `source_mutation_during_copy_fails_closed_and_recovers` | `SourceChanged`; partial removed; inbound untouched; no receipt; retry against the settled tree converges and drains |
| Names: quotes, apostrophes, semicolons, ampersands, parentheses, leading dash, Unicode | `adversarial_candidate_names_acquire_cleanly` | all four fixtures acquire byte-for-byte; inbound empty |
| Traversal input (`../escape`, `/etc/absolute`, `nested/child`) | `traversal_source_paths_are_rejected_before_io` | `InvalidSourcePath` before any I/O; no intent row; no staging writes |
| Symlink escape | `tree_hash_and_copy_tree_skip_symlinks_and_special_files` (unit), `symlink_candidates_are_safe` (reconcile) | internal + dangling escape symlinks never followed, hashed, or copied |
| Staging collision (non-directory at deterministic path) | `staging_collision_with_non_directory_fails_closed` | fails closed; collision file and source untouched |
| Configured-root symlink aliases | `resolve_configured_root` rejects symlink components (config.rs unit tests incl. storage root; `publication_roots_reject_symlink_component`) | rejected at configuration |
| Restart after acquisition with inbound absent | `restart_with_inbound_absent_continues_from_staging` | fresh pool/worker reclaims the expired lease, adopts the durable receipt, inspection completes and publishes exactly one manifest |
| NeedsReview / Quarantined retention across restart | `needs_review_and_quarantined_retain_staged_source_across_restart` | staged bytes intact and resolvable in both states after reconnect |
| Stale worker losing lease during copy | `stale_worker_losing_lease_during_copy_cannot_commit` | cannot record receipt or remove source; on-disk finalize is adopted by the current owner only after byte verification; one acquisition total |
| Delayed cleanup after path reuse (A replaced by B) | `delayed_cleanup_after_path_reuse_preserves_new_occurrence` + unit `delayed_cleanup_never_removes_a_reused_inbound_path` | A records `staged`/replaced, never deletes B; B becomes a distinct acquisition and drains |
| Concurrent claims for one occurrence | `concurrent_acquisition_of_one_occurrence_is_single_authoritative` + persistence claiming tests | lease fence rejects the rival before intent; exactly one handoff/receipt/drain |

### Generation-fence correction (behavioral finding)

The adversarial suite exposed one real defect in the first R0 implementation: `source_generation`
mixed directory ctime into occurrence identity, so an in-place content mutation during acquisition
looked like a path replacement at cleanup time — the retried (fully verified) acquisition recorded
`Replaced` and left the same occurrence in inbound, which reconciliation would have re-acquired as
a duplicate. Occurrence identity is now the directory device+inode on Unix (a replaced path always
changes inode); content evidence remains the complete tree hash. The two fences are independent
and both must pass before removal. Unit and integration path-reuse tests pass unchanged.

## 8. R0.6 — configuration, containers, and diagnostics

- `MELODEE__INGEST__STAGING_ROOT` is required, absolute, and validated whenever inbound ingestion
  has sources; startup refuses missing staging configuration with a role-specific error
  (`IngestionConfig::from_env`, `storage_roots()`, surfaced fatally in `main.rs`).
- Overlap rejection covers inbound/staging, inbound/work, inbound/canonical, staging/work,
  staging/canonical, and work/canonical using canonical path-component containment
  (`storage_roots()`), plus search-index and podcast roots at server startup (`main.rs`).
- Inbound, staging, work, canonical, search-index, and podcast roots remain independently
  configurable; compose.yml mounts inbound writable (`read_only: false`, required for candidate
  rename/removal) and gives staging/work/library/search distinct named volumes.
- Onboarding/diagnostics now probe and display all four roles — inbound source(s), staging root,
  normalization work area (writable-or-creatable, matching the search-root convention), and
  canonical root — in the existing `storage_paths` preflight check surfaced by the Admin UI
  storage screen. No secrets are emitted; no dynamic settings system was added.
- Same-filesystem rename vs cross-filesystem verified copy-finalize-remove is documented in
  `docs/installation/storage.md` and `docs/library-management/inbound-processing.md`.

## 9. R0.7 — requirement and evidence correction

- PRD Gate B now distinguishes inbound candidate removal after a verified staging receipt from
  separately authorized staged/canonical deletion; the data-loss risk row and PRD-INGEST-006's
  work-area location were corrected to match ADR-0145; PRD-DEPLOY-003 lists the four roles.
- ADR-0116 keeps its historical text with its existing partial-supersession marker untouched.
- Phase 2/Phase 3 reports already carried ADR-0145 notes; Phase 5 report now records the
  corrective follow-up and links here; the Phase 3 canonical-publication sub-evidence gained an
  explicit supersession note.
- The strongest 200-Release vertical now proves staging acquisition, inbound drain, complete
  downstream processing, and an empty second reconciliation (§5).
- Stale public/operator claims corrected: quick-start (writable inbound mount), concepts
  (acquire-then-drain lifecycle), admin-api preview (staging root), configuration safety rules,
  storage architecture doc, local-development runbook, release-distribution container evidence
  (historical note), and the Unreleased changelog entry.
- Requirement artifacts regenerated (`python3 scripts/extract_requirements.py`); PRD-INGEST-005
  is terminal (`tested`) with this report and the named suites as evidence. PRD-INGEST-009 stays
  open for R2 resource bounds; PRD-STORAGE-004 stays open for R3 migration adoption.

## 10. Gate results

Focused suites (this commit):

```text
cargo test -p melodee-ingestion --all-features --test acquisition   -> 13 passed, 0 failed
cargo test -p melodee-ingestion --all-features --test inspection    -> 5 passed, 0 failed
cargo test -p melodee-ingestion --all-features --test normalization -> 12 passed, 0 failed
cargo test -p melodee-ingestion --all-features --test reconciliation-> 12 passed, 0 failed
cargo test -p melodee-ingestion --all-features --test publication   -> 15 passed, 0 failed
cargo test -p melodee-server --all-features --test admin_onboarding_api
   --test admin_diagnostics_api --test admin_system_api             -> 28 passed, 0 failed
cargo clippy -p melodee-ingestion --all-targets --all-features -- -D warnings -> clean
```

Full gate: `./scripts/check.sh` — see the tail of this section for the recorded run.

- Repository/document validation: pass
- Requirements index drift check: pass
- Python capture tests: pass (140)
- Rust formatting + workspace Clippy (`-D warnings`): pass
- `cargo test --workspace --all-features`: pass (933 passed, 0 failed, 4 ignored benchmarks)
- Admin UI gate (pnpm typecheck/lint/i18n/unit/build): pass
- `cargo deny check`: pass

## 11. Files changed in R0 closure

- `crates/melodee-ingestion/src/acquisition.rs` — test-support fault hook/barrier, forced-copy
  injection, stale-partial reclamation before rename retry, occurrence-identity generation fence
  (dev+inode), unit tests.
- `crates/melodee-ingestion/src/config.rs` — `acquisition_test_hook` (test-support only).
- `crates/melodee-ingestion/src/lib.rs` — hidden test-support exports.
- `crates/melodee-ingestion/tests/acquisition.rs` — new 13-test R0.3/R0.4/R0.5 suite.
- `crates/melodee-ingestion/tests/normalization.rs` — 200-Release vertical drain/rescan proofs.
- `apps/melodee-server/src/onboarding.rs` — four-role storage diagnostics incl. work area.
- `PRD.md`, `CHANGELOG.md`, `design/architecture/STORAGE-AND-MANIFESTS.md`,
  `design/runbooks/LOCAL-DEVELOPMENT.md`,
  `design/evidence/phase-3/canonical-publication/README.md`,
  `design/evidence/phase-5/PHASE-5-REPORT.md`,
  `design/evidence/release-distribution/container-validation.md`,
  `docs/src/content/docs/getting-started/quick-start.md`,
  `docs/src/content/docs/getting-started/concepts.md`,
  `docs/src/content/docs/api/admin-api.md`,
  `docs/src/content/docs/installation/configuration.md` — ADR-0145 language corrections.
- `design/requirements/status-overrides.json` + regenerated requirement artifacts.
- `IMPLMENTATION_OUTSTANDING.md` — phase map/status truthfulness.

## 12. Impact statements

- **Compatibility:** none. No external route, DTO, status code, auth flow, pagination, range, or
  intentional 410/501 behavior changed (native v1, OpenSubsonic, Jellyfin untouched).
- **Data/migration:** none to existing PostgreSQL schemas (migration 0031 predates this closure
  and is unchanged). The generation-fence format is in-memory evidence, not persisted data.
- **Security/data-loss:** the generation-fence defect (§7) was found and fixed by the adversarial
  suite before release; all destructive operations remain fenced by containment, occurrence
  identity, verified receipt, and active lease.
- **Performance/operational:** acquisition remains a single rename on one filesystem; the copy
  path reads source bytes a bounded number of times with 1 MiB buffers. No new production
  dependency; the test hook compiles out of default builds.

## 13. Known limitations (owned by later closure phases)

- The cross-filesystem branch is exercised through the forced-copy hook; a two-device physical
  rehearsal belongs to R4 storage-topology validation.
- `/tmp/melodee-work` remains the development default work area (disposable by contract);
  production guidance is the container/compose configuration.
- Podcast-storage and queue-saturation health depth, Last.fm configured-state evidence, and live
  SMTP are R2 scope; production-scale rehearsal is R4 scope.
