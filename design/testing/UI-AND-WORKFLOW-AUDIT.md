# UI and Workflow Audit

Status: **active** (2026-11-12). Companion to the gates in `AGENTS.md`; this document explains the
seams the standard gates do **not** cover and the audits that close them.

## Why this exists

Four defects reached an operator while every committed gate was green:

| Defect | Why the gate missed it |
|---|---|
| Releases parked in `indexing` forever — never terminal, invisible in History, counted as active processing | No test asserted that a lifecycle **completes**. API tests assert transitions, not that the last one exists. |
| Artist `Verified` unreachable — import writes `provisional` and nothing promoted | The domain enum and the UI chip both existed; only an end-to-end path assertion would notice nothing produces the state. |
| Release edit offered `Country`/`Release Type`, which the wire contract had removed — saves silently discarded input | `admin-ui/e2e/*` mocks the API with `page.route()`, so the mock accepted the fields the real server drops. |
| Artist `Type` was free text over a closed dictionary; "Appearances 1" opened an empty tab | Same mock seam, plus no assertion that a rendered statistic has drill-down rows. |

Root cause is structural, not carelessness:

- **The Admin UI e2e suite is entirely mocked** (18 spec files, zero live requests). It proves the
  UI behaves against fixtures, never that the UI and the API agree.
- **Rust tests prove the API**, not that the UI calls it with the right fields.
- **No test asserted workflow completion or statistic provenance.**

## The four audit layers

### 1. Runtime invariant audit — `make audit`

`scripts/audit_runtime_invariants.py`: read-only assertions over a running instance's database. It
refuses to execute anything that is not a `SELECT`/`WITH`, so it is safe to run at any time,
including against a production-shaped deployment.

| Invariant | Catches |
|---|---|
| `workflow.no_stuck_non_terminal` | A Release parked in a non-terminal state past the threshold (the `indexing` dead-end) |
| `workflow.client_visible_matches_status` | `indexing`/`indexed` without `releases.status = 'active'` — the partial commit that would make "Active" a lie |
| `workflow.indexed_requires_durable_projection` | `indexed` claimed while the search projection still has pending rows |
| `catalog.playable_asset_resolves` | A Track whose playable media asset does not resolve (silent playback failure) |
| `catalog.release_aggregate_matches_tracks` | A Release aggregate the UI renders disagreeing with the Track rows |
| `catalog.artist_credits_backed_by_rows` | A statistic with no drill-down rows ("Appearances 1", empty tab) |
| `catalog.artist_type_in_dictionary`, `catalog.artist_status_in_dictionary`, `catalog.release_status_in_dictionary`, `workflow.state_in_dictionary` | Any value outside the closed dictionary that reached the database — the failure mode of free-text enum fields |
| `catalog.import_bindings_resolve` | Orphaned import bindings (the queue's Release/Artist columns and completion logic both join on them) |
| `identity.provider_candidates_resolve` | Provider candidates attached to a missing Artist (the Identity tab renders them) |
| `projection.no_stale_outbox_when_idle` | A stalled projection worker: nothing in flight, rows pending for >10 minutes |
| `queue.counts_match_rows` | The queue's rendered per-state counts disagreeing with the rows |

Exit code is non-zero on any violation; `--json` is available for wiring into monitoring.
`--stuck-minutes` tunes the completion threshold.

### 2. Live Admin UI audit — `make audit-live`

`admin-ui/e2e/live/live-admin-audit.spec.ts` with `playwright.live.config.ts` drives a **real**
instance and a real administrator session (no `page.route()`), and asserts the seam the mocked suite
cannot:

1. **Every canonical screen loads** with no console error and no failed `/api/` request.
2. **Edits round-trip**: change a value, save, reload, and read it back — a silently dropped field,
   a wrong field name, or a rejected value fails here.
3. **Closed dictionaries are selects** whose options are exactly the domain values (Artist type).
4. **No form offers a field the API rejects**, and validated fields (dates) are validated.
5. **Statistics drill down**: a stat card opens the rows it counts.

Run:

```bash
Live credentials are read from the environment; nothing is stored:
MELODEE_LIVE_URL=http://127.0.0.1:8080/admin \
MELODEE_LIVE_USERNAME=<admin> MELODEE_LIVE_PASSWORD=<password> \
  make audit-live
```

Without credentials the tests **skip** (never silently pass); the default `make check` /
`do-prechecks.py` gate is unaffected.

### 3. Workflow completion drill — manual, documented

The invariant audit proves completion *state*; a drill proves completion *transition* on real media:

1. `make audit` (baseline must be green).
2. Copy one already-processed Release from the retained staged source
   (`<staging-root>/releases/…`) into a fresh directory under the inbound root.
3. Watch `release_work_items` until the new item reaches a terminal state; it must reach `indexed`,
   with `releases.status = 'active'` and no pending projection rows.
4. Re-run `make audit`: it must still be green, and the catalog counts must have increased by
   exactly one Release (History shows the new Release; the queue no longer lists it).

A batch drill (25 Releases) exercises the same path at scale and is the shape of the evidence in
`design/evidence/`.

### 4. C# parity ledger — the completeness claim

Runtime coverage claims come from `design/evidence/feature-parity/CURRENT-CSHARP-RECONCILIATION.md`
and the MEL-1 epics, not from this audit. This audit's job is to keep the **runtime** honest; when it
finds a gap, the correct action is to record it in the parity ledger (Hamstik MEL ticket) so the
"implemented" claim is corrected rather than quietly repaired.

## What this audit cannot tell you

- **That feature X matches the C# product.** Only the parity ledger and the acceptance epics
  (MEL-2..19) speak to that.
- **That a workflow behaves correctly under load or failure.** That is the target-scale and
  fault-injection acceptance work (MEL-16), not a single-instance audit.
- **That provider/social/streaming integrations work against real services.** Those need credentials
  and are tracked as `UNVERIFIED` (MEL-9..11, MEL-19).

## Recommended cadence

| When | Command |
|---|---|
| After any change to the Admin UI | `make audit-live` against a local instance |
| After any change to the ingestion lifecycle, projection, or completion logic | `make audit` |
| Before claiming a workflow is done | `make audit` + the completion drill in §3 |
| In CI | `tests/python/test_audit_runtime_invariants.py` + the existing mocked e2e suite (a live instance is not available there) |

## Extending it

Add a check to `CHECKS` in `scripts/audit_runtime_invariants.py` whenever a new UI statistic,
status, or lifecycle state is introduced: the rule is that every rendered count has a query that
proves it can be drilled down, and every status the UI can display must be produced by the code
path that the audit names.
