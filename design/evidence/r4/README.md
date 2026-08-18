# R4 — Production rehearsal, dual run, cutover, and rollback

Status: In progress. The isolated rehearsal and local rollback legs are
complete; production snapshot, physical-client, and deployment-provider legs
remain operator-owned acceptance work.

Date: 2026-08-18

## Scope and authority

R4 is the rehearsal/cutover portion of official Phase 11. The accepted
migration contract is ADR-0148. ADR-0149 defines the proposed rehearsal,
freeze, and cutover amendment and remains pending release-owner acceptance;
ADR-0144 (container distribution) and the R4 work packages in
`IMPLMENTATION_OUTSTANDING.md` §9 also apply.

The rehearsal harness never targets a live database or canonical library. It
creates disposable PostgreSQL databases from the frozen legacy DDL, imports
into a fresh target, restores a target backup into an isolated database, and
keeps the failed target available for rollback inspection until the harness
cleanup step.

## Reproducible local evidence

Prerequisites: PostgreSQL on host port 5433, the pinned Rust toolchain, and a
built `melodee-cli` binary.

```text
python3 scripts/legacy_rehearsal.py clean
python3 scripts/legacy_rehearsal.py drill --songs 12000
```

The latest production-shaped run is recorded in
`rehearsal-drill-12000.json`. It imported 500 Artists, 1,000 Releases, 12,000
Tracks, 50 users, 2,400 playlist items, 6,000 user-track states, and 12,000
play-history rows. The source schema fingerprint was
`20260714155936_AddDeleteSourceResidueAfterIngestSetting`.

The run proved:

- baseline capture is source-identity and marker based, with a key-only
  configuration fingerprint;
- one planned run is imported and verified without a second implicit run;
- import completed in 177.24 seconds with batch size 2,500 and a measured
  31,588,352-byte peak RSS for the CLI process;
- verification found zero mismatch gaps and zero duplicate public IDs;
- a fresh Tantivy generation was rebuilt and activated (7,178,032 bytes) in
  5.68 seconds with a measured 160,940,032-byte peak RSS;
- five bounded exact/broad/Artist/Release/Track/year-filtered search queries
  returned within 150 ms p50 and 350 ms p95, with the default 100-hit limit;
- the target database measured 98,932,415 bytes after import, and a
  custom-format PostgreSQL backup (7,364,899 bytes) restored with 12,000
  Tracks present;
- a write arriving before the freeze marker was imported by the final delta;
- the acknowledged freeze reached `cutover-ready` in a 3,553 ms freeze window;
- the legacy source remained authoritative and the failed target was retained
  for forensics during the rollback drill; and
- the pre-existing canonical media tree was not modified by migration.

The importer preserves the native authorization vocabulary from the legacy
role flags (`HasStreamRole`, `HasDownloadRole`, and so on), including
`Administrator` and `HasJukeboxRole`. This keeps migrated users authorized to
receive native stream URLs; the regression is covered by the full migration
integration suite.

Search validation was repeated independently against a migrated target with
`melodee-cli search rebuild --actor r4-rehearsal`: the active generation
contained 10 Artists, 10 Releases, and 19 Tracks, and an exact
`artist:"Aurora Fields"` query returned the expected Artist, Release, and
three Track public IDs.

The production-shaped drill now also records process peak RSS and executes a
bounded query suite directly against the rebuilt generation. These are local
synthetic measurements, not a production performance SLA: queue age, disk
headroom, failure/retry counts, repeated clean-target runs, and the real
approximately-5 TB topology still require the external R4 rehearsal.

The workspace search integration suite separately proves interruption,
restart, corruption recovery, lease fencing, and atomic generation activation
(`crates/melodee-search/tests/rebuild_integration.rs` and
`crates/melodee-persistence/tests/search_phase6.rs`). Those tests are included
in the full repository gate; the drill's measurements above are the
production-shaped migration/search evidence, not a replacement for that
fault suite.

The native replay was rerun after the migration authorization, audio-metadata,
catalog ordering, user-state, login-version, cookie-cache, and comparator fixes
and remains recorded in `dual-run-report.json`: all 12 native groups executed,
with 258 classified differences and 204 unclassified comparator differences.
The replay also closed the previously observed
`native-user-extra/search-suggest`, `native-discovery/login`, and
`auth-cookie-sign-in`, and missing-chart error-message mismatches:
normalized contains matching now returns the two expected Artists and three
expected Songs in legacy order. Legacy bitrate values
now preserve the captured kbps/bps units and BPM is retained in native audio
features; random-song selection and replay-created URL variables are compared
using field-specific volatility rules. The remaining differences include
intentional current-vs-legacy behavior, migration metadata boundaries, and
unresolved catalog ordering/state gaps. They must receive external
compatibility classification or implementation fixes before release.

An OpenSubsonic replay also executed 11 groups through migrated snapshots. Its
negative-route matrix matched, but the run stopped closed on the captured
legacy range/playlist status quirks and recorded 389 unclassified differences,
primarily missing migrated manifest metadata (genre and bit depth),
legacy-derived play fields, and canonical path presentation. BPM and bitrate
differences are no longer present after the audio-metadata fix. The remaining
metadata boundary is recorded in proposed ADR-0150 and the importer's durable
`metadata.reference_only` issues. This is
additional R4 blocker evidence, not an adapter-contract change; the committed
report is left as the native replay report so the accepted native evidence is
stable, while the disposable OpenSubsonic run remains under
`/tmp/melodee-r4-dual-opensubsonic` for investigation.

The smaller `rehearsal-drill-1200.json` run is retained as a quick local
iteration sample. It is not a substitute for the 12,000-Track evidence.

The same 12,000-Track drill was then repeated from a newly seeded source and
fresh target; the separate result is recorded in
`rehearsal-drill-12000-repeat.json`. Both runs reported zero verification
mismatch gaps and zero duplicate public IDs, identical 500/1,000/12,000/50
Artist/Release/Track/user counts, 12,000 restored Tracks, a successful
cutover-ready freeze with the pre-marker write imported, a retained failed
target, and an unchanged 16-file/16,000,000-byte canonical tree. Import peak
RSS was 31,588,352 and 31,539,200 bytes; search rebuild peak RSS was
160,940,032 and 161,017,856 bytes. These repeat results establish local
reproducibility, but do not substitute for the production-sized snapshot,
queue/headroom, and deployment-topology measurements required by R4.2.

## Implemented corrective work

The rehearsal script now uses the frozen legacy DDL's required columns, seeds
valid unique playlist ordering, stops on the first seed SQL error, imports into
the run recorded by `plan`, and validates JSON command output without human
status-line contamination. The CLI sends JSON-mode verification status to
stderr and suppresses internal delta chatter while `freeze --json` is
assembling its document.

The native-v1 search-suggest correction is covered by
`native_search_suggest_preserves_legacy_contains_matching_and_order` in
`apps/melodee-server/tests/native_catalog_api.rs` and by the real migrated
dual-run replay: normalized substring matching now returns the captured Artist
and Song sets in legacy order, while playlist suggestions remain empty as in
the frozen `SearchService`.

Focused proof:

```text
cargo test -p melodee-cli --test legacy --test legacy_freeze
14 passed; 0 failed
```

## Requirements and ADR disposition

R3's migration requirements remain evidenced by the existing report. R4
extends that evidence for `PRD-MIG-007` rollback behavior and the rehearsal
legs of `PRD-TEST-005`, `PRD-TEST-006`, `PRD-TEST-007`, `PRD-PERF-004`,
`PRD-SCALE-001`, and `PRD-SCALE-002`. The requirement registry is not marked
terminal until the external R4 legs below are completed.

No compatibility DTO, status, identifier, or authentication contract was
changed. The search-suggest implementation was corrected against the frozen
legacy behavior without changing its wire shape. Migration remains
source-read-only,
non-destructive, and media adopting rather than relocating or re-encoding.

## Remaining R4 acceptance

These require deployment credentials, production-shaped snapshots, or client
hardware and cannot be honestly generated by this isolated local harness:

- R4.2 repeat on the real approximately-5 TB production-shaped snapshot;
- R4.4 dual-run replay against equivalent C# and Rust snapshots. The local
  executable native replay is recorded in `dual-run-report.json`: all 12
  native groups ran from isolated per-file migrated snapshots, and stream
  HEAD/full/range requests passed. The current comparator report records 258
  classified and 204 unclassified differences; the unclassified set is not
  silently waived and still requires compatibility classification or fixes.
  The earlier OpenSubsonic replay recorded 389 differences, including
  migration metadata omissions, and requires a separate correction and
  compatibility review.
- R4.5 physical Android, Android Auto/DHU, MeloAmp, OpenSubsonic, and Jellyfin
  client workflows;
- R4.6 production Google OIDC, Last.fm, SMTP, real podcast, proxy/TLS, and
  storage-topology integrations; and
- R4.7/R4.8 operator-authorized production freeze, traffic switch, and rollback.

The executable entry points are `scripts/run_r4_dual_run.py` and
`scripts/legacy_rehearsal.py`; their reports must be attached here after those
environment-owned runs. R4 is therefore intentionally not promoted to
Complete by this local evidence alone.

## Completion protocol

Every production rehearsal report must record the baseline and ending commit,
source/target schema and configuration fingerprints with secrets redacted,
counts/checksums, client/provider versions, exact commands, direct gate
results, rollback decision owners, and the documentation, compatibility,
data/migration, security, and operational impacts. A passing aggregate gate
does not replace the physical-client or provider evidence.

Documentation impact:

- Updated — this internal R4 evidence report and the migration rehearsal
  script. Public migration documentation remains truthful about the
  rehearsal-gated production cutover and therefore needs no status change yet.
