# R4 — Production rehearsal, dual run, cutover, and rollback

Status: In progress. The isolated rehearsal and local rollback legs are
complete; production snapshot, physical-client, and deployment-provider legs
remain operator-owned acceptance work.

Date: 2026-08-19

## Scope and authority

R4 is the rehearsal/cutover portion of official Phase 11. The accepted
migration contract is ADR-0148. ADR-0149 defines the proposed rehearsal,
freeze, and cutover amendment and remains pending release-owner acceptance;
ADR-0144 (container distribution) and the R4 work packages in
`IMPLMENTATION_OUTSTANDING.md` §9 also apply.

The default rehearsal harness never targets a live database or canonical
library: it creates disposable PostgreSQL databases from the frozen legacy
DDL, imports into a fresh target, restores a target backup into an isolated
database, and keeps the failed target available for rollback inspection until
the harness cleanup step. An explicit `--source-db` mode now accepts an
operator-provided existing isolated snapshot and `--canonical-root`; it never
seeds, writes, or drops that source. Large canonical roots use a metadata
fingerprint unless `--hash-media` is explicitly supplied, so the resulting
artifact cannot claim byte-level identity without measuring it.

## Reproducible local evidence

Prerequisites: PostgreSQL on host port 5433, the pinned Rust toolchain, and a
built `melodee-cli` binary.

The documented operator entry points were checked against the current binary
on 2026-08-20 with `python3 scripts/check_command_help.py`. Each command
exited successfully and printed its Clap usage: `migrate --help`, `legacy
--help`, `legacy inventory --help`, `legacy plan --help`, `legacy baseline
--help`, `legacy import --help`, `legacy verify --help`, `legacy delta
--help`, `legacy status --help`, `legacy freeze --help`, and `search rebuild
--help`. The same audit checked `python3 scripts/legacy_rehearsal.py drill
--help`. The environment was reduced to `PATH` only, so the audit cannot
connect to a source snapshot or change a target.

```text
python3 scripts/legacy_rehearsal.py clean
python3 scripts/legacy_rehearsal.py drill --songs 12000
python3 scripts/legacy_rehearsal.py drill --help
# Optional fail-closed capacity check for an operator-sized rehearsal:
python3 scripts/legacy_rehearsal.py drill --source-db "$SNAPSHOT_URL" \
  --library-path-map /srv/music=library --canonical-root /srv/music-copy \
  --source-backup /srv/backups/legacy-snapshot.dump \
  --min-free-bytes 1099511627776 --min-headroom-ratio 0.15
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
- import completed in 178.34 seconds with batch size 2,500 and a measured
  31,182,848-byte peak RSS for the CLI process in the current run;
- verification found zero mismatch gaps and zero duplicate public IDs;
- a fresh Tantivy generation was rebuilt and activated in 5.83 seconds with a
  measured 161,964,032-byte peak RSS in the current run;
- five bounded exact/broad/Artist/Release/Track/year-filtered search queries
-  returned within 150 ms p50 and 300 ms p95, with the default 100-hit limit;
- the target database measured 98,973,375 bytes after import, and a
  custom-format PostgreSQL backup (7,365,393 bytes in the current run) restored with 12,000
  Tracks present;
- a write arriving before the freeze marker was imported by the final delta;
- the acknowledged freeze reached `cutover-ready` in a 3,466 ms freeze window;
- the legacy source remained authoritative and the failed target was retained
  for forensics during the rollback drill; and
- the pre-existing canonical media tree was not modified by migration; its
  16-file/16,000,000-byte before/after evidence used explicit byte hashing.

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

The rehearsal also supports explicit `--min-free-bytes` and
`--min-headroom-ratio` preflight thresholds. The check runs against the
disposable target/search work directory before target creation, records the
observed disk metrics and thresholds in the machine-readable `preflight`
object, and fails closed when either threshold is not met. This makes an
insufficient-disk rehearsal reproducible without allowing a partially created
target to be treated as evidence; it does not predict the final production
capacity requirement.

The documented operator command audit is now reproducible with
`python3 scripts/check_command_help.py`. It invokes the eleven `melodee-cli`
paths and `legacy_rehearsal.py drill --help` with only `PATH` in the child
environment. The audit passed on 2026-08-20; its two unit tests also pin the
matrix and environment boundary. This checks command availability and usage
text only; it does not connect to a source snapshot or alter a target.

The existing-snapshot path is documented for that external rehearsal in the
public migration guide. It records source counts through a read-only `psql`
session, leaves cutover in `delta-ready` unless an operator supplies the
explicit acknowledgement, validates a required operator-supplied custom-format
legacy backup with `pg_restore --list` before target creation, and keeps target
cleanup separate from the supplied source. No production snapshot, backup, or
canonical-root URL has been used in this repository evidence.

The workspace search integration suite separately proves interruption,
restart, corruption recovery, lease fencing, and atomic generation activation
(`crates/melodee-search/tests/rebuild_integration.rs` and
`crates/melodee-persistence/tests/search_phase6.rs`). Those tests are included
in the full repository gate; the drill's measurements above are the
production-shaped migration/search evidence, not a replacement for that
fault suite.

The native replay was rerun after the migration authorization, audio-metadata,
catalog ordering, user-state, login-version, cookie-cache, comparator, and
migrated share-order fixes and is recorded in
[`dual-run-report-native.json`](dual-run-report-native.json): all 12 native
groups executed, with 267 classified differences and 181 unclassified
comparator differences. Six replay-created account lifecycle timestamps are
now classified by exact scenario-and-field rules; other user, playlist, state,
ordering, metadata, and provider fields remain differential assertions.
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

The current 181-entry unclassified set has a concrete residual shape rather
than one undifferentiated parity claim. The largest clusters are 117 catalog
list projection/order fields (63 `albums-list`, 32 `artists-list-page2`, and
22 `artists-list`), 20 genre/reference-only fields and route results, 10 live
podcast discovery/OPML fields, 3 disabled playback-backend response fields,
and 1 artist-lookup provider-candidate field. The remaining entries are
scattered user-state/play-count, migrated timestamp/biography, playlist/share
payload, search-similarity, and related catalog fields. The catalog clusters
need an explicit ordering/data-model decision; the genre cluster depends on
the `metadata.reference_only` boundary recorded in proposed ADR-0150; live
podcast values need provider/volatility evidence; the disabled backend result
must be reviewed under PRD-PARTY-003 and ADR-0123 rather than waived as Party
Mode absence; and the artist-lookup result needs a decision reconciling the
captured provider-backed response with ADR-0136's provider deferral. No item
in this residual set is silently reclassified by this report.

The 117-entry catalog ordering/projection cluster is documented in the
[catalog ordering decision package](catalog-ordering-decision-package.md).
It presents the captured source sequence, current deterministic ordering, and
three alternatives without changing runtime behavior or accepting an ADR.

### R4.4 decision-package and assumptions preparation — 2026-08-20

- Exact files changed: `design/evidence/r4/behavioral-assumptions.md`,
  `design/evidence/r4/catalog-ordering-decision-package.md`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.1/R4.4 evidence preparation is strengthened with one auditable record of
  compatibility, migration, security, storage, data-loss, and release boundaries plus a decision
  package for the 117-entry native catalog ordering cluster. No requirement was marked terminal.
- ADRs: none accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed; the ordering
  package explicitly declines to implement any option before owner disposition.
- Tests: `git diff --check`, `python3 scripts/validate_repository.py`,
  `python3 scripts/extract_requirements.py --check`, and the focused release-evidence/script suite
  passed; the latest `./scripts/check-smoke.sh` also passed.
- Compatibility impact: none; no route, DTO, identifier, status, auth, pagination, range, or
  runtime ordering behavior changed.
- Data/migration impact: none; the package describes existing migration mappings and alternatives
  without changing PostgreSQL schema, source data, canonical media, or search authority.
- Security/data-loss impact: positive clarification; the document makes no-follow, read-only,
  secret-redaction, non-destructive uncertainty, backup, and rollback boundaries explicit.
- Performance/operational impact: no runtime change; the selected ordering option will require
  explicit query/performance evidence before implementation.
- Documentation impact: Updated — this R4 evidence report, the new decision/assumption artifacts,
  and `IMPLMENTATION_OUTSTANDING.md`; no public behavior documentation changed.
- Remaining risks and owner: release/compatibility owner must decide the ordering strategy and all
  other listed external gates; R4.4 remains Partial.
- External input still required: owner disposition of the ordering options and ADR-0149/0150/0151,
  plus the production snapshot, client, provider, and cutover inputs listed in the assumptions file.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

The cross-package behavioral assumptions and authorization boundaries used by
the local rehearsal are recorded in
[`behavioral-assumptions.md`](behavioral-assumptions.md). They are explicitly
working assumptions, not accepted ADRs or production authorization.

An OpenSubsonic replay also executed 11 groups through migrated snapshots. Its
negative-route matrix matched, but the run stopped closed on the captured
legacy range/playlist status quirks and recorded 333 unclassified differences
in [`dual-run-report-opensubsonic.json`](dual-run-report-opensubsonic.json).
The adapter correction now projects canonical `Artist/Release/file` paths as
the captured legacy-relative `Release/file` paths, removing 49 path
differences; the migration authority and media manifest joins remain
unchanged. The remaining differences are primarily missing migrated manifest
metadata (genre and bit depth), legacy-derived play fields, and playlist/share
projection. BPM and bitrate differences are no longer present after the
audio-metadata fix. The remaining metadata boundary is recorded in proposed
ADR-0150 and the importer's durable `metadata.reference_only` issues. This is
additional R4 blocker evidence, not an adapter-contract change. The
surface-scoped report is committed so the result is reproducible and
reviewable.

A Jellyfin replay executed eight migrated groups and is recorded in
[`dual-run-report-jellyfin.json`](dual-run-report-jellyfin.json): 149 classified
and 17 unclassified differences. The migrated media-container mapping and
Jellyfin primary-release artist-count correction removed the WAV/codec and
artist-count residuals under focused tests. The remaining residuals are
concentrated in genre projection, instant-mix cardinality, and playlist
metadata; no broad comparator exclusion was added.

The native, OpenSubsonic, and Jellyfin migrated replays were rerun from the current working tree
on 2026-08-19 against disposable `r4_dual_source`/`r4_dual_target` databases. That historical
run completed all 12 native groups with 267 classified and 185 unclassified differences, all 11
OpenSubsonic groups with 315 classified and 336 unclassified differences, and eight Jellyfin
groups with 149 classified and 17 unclassified differences. The 2026-08-20 disabled-podcast
phase correction reran OpenSubsonic and superseded its report with 318 classified and 333
unclassified differences; the later surface-specific fixture isolation rerun supersedes that
current OpenSubsonic count with 330, so the current committed reports at that point were 192/330/17. The commands
remain fail-closed because residuals remain; this does not make R4.4 complete.

The smaller `rehearsal-drill-1200.json` run is retained as a quick local
iteration sample. It is not a substitute for the 12,000-Track evidence.

The same 12,000-Track drill was then repeated from a newly seeded source and
fresh target; the separate result is recorded in
`rehearsal-drill-12000-repeat.json`. Both runs reported zero verification
mismatch gaps and zero duplicate public IDs, identical 500/1,000/12,000/50
Artist/Release/Track/user counts, 12,000 restored Tracks, a successful
cutover-ready freeze with the pre-marker write imported, a retained failed
target, and an unchanged 16-file/16,000,000-byte canonical tree. Import peak
RSS was 31,793,152 and 32,153,600 bytes; search rebuild peak RSS was
170,680,320 and 138,530,816 bytes. These repeat results establish local
reproducibility, but do not substitute for the production-sized snapshot,
queue/headroom, and deployment-topology measurements required by R4.2.

The current marker implementation is `legacy-marker-v2`: it hashes row counts
for all 52 frozen source data tables and includes the maximum
`LastUpdatedAt` for timestamped tables, including tables excluded from import
by accepted decisions. The 1,200-Track refresh and the current 12,000-Track
refresh record this marker, disk headroom, queue age, issue/error counts, and
an explicit retry measurement. No retries are inferred from checkpoint totals;
the drill injects none and records `retries.observed: 0` with that scope.

The read-only production-integration probe is
`scripts/r4_integration_probe.py`. It has no credential arguments and performs
only explicitly requested bounded checks for health/readiness, range
responses, OIDC discovery metadata plus bounded JWKS shape validation, podcast XML, Last.fm reachability without
credentials, proxy/TLS endpoint headers, SMTP STARTTLS handshake, filesystem topology, and custom-format backup integrity. Endpoint hosts and
configuration paths are fingerprinted in JSON output. Unit and loopback-fixture coverage is in
`tests/python/test_r4_integration_probe.py`; the probe does not substitute for
the operator's real credentials or external integration acceptance.

The optional `--proxy-url` check performs a bounded GET through the intended
proxy/TLS endpoint, can require HTTPS, and compares explicitly requested
response headers without recording their values. This validates deployment
header policy without accepting cookies, credentials, or mutating requests.

Local graceful-shutdown behavior is also exercised by the bounded
`scripts/container_smoke.sh` path, which stops the disposable server with a
15-second grace period and requires a zero exit status. This is local image
 evidence only; deployment proxy/TLS acceptance, provider delivery, and shutdown
acceptance on the production topology remain external R4.6 conditions.

The R4.5 current-client evidence matrix is validated by
`scripts/validate_r4_client_acceptance.py` and its tests in
`tests/python/test_r4_client_acceptance.py`. Schema version 2 keeps each
physical-client row `pending-external` until the migrated target, exact client
and platform versions, server commit, configuration fingerprint, command,
result, and sanitized repository-relative evidence are recorded. The validator
rejects obvious credentials and cannot promote a partial matrix to `pass`.
The Feishin launcher now has an existing-target mode using
`--existing-db-url-env` and `--existing-storage-root`; its safety checks and
no-seed/no-drop behavior are covered by
`tests/python/test_phase8_client_smoke.py`. The mode was not run against a
real migrated target because no approved target URL or isolated canonical-root
mapping was supplied.

The ADR-0149 decision package at
[`adr-0149-decision-package.md`](adr-0149-decision-package.md) records the
current baseline, freeze, backup/restore, and rollback measurements and names
the exact release-owner decisions and production-topology evidence still
required. ADR-0149 remains Proposed.

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

The same replay and focused native integration coverage also corrected eight
small captured-contract implementation mismatches: native login now reports
the captured `2.2.0.0` server-version value while `/system/info` remains
`2.2.0`; successful browser sign-in now carries the captured no-cache headers;
missing chart detail/playlist requests use the captured `Chart not found`
message; missing theme assets use the captured problem-details envelope and
cache header; empty playlist import/image uploads use the captured validation
problem-details envelope; missing playback-backend heartbeats use the captured
not-found message; unknown stream users use the captured authorization
message; and native image responses no longer add an uncaptured cache-control
header. These are implementation corrections to frozen behavior,
not new compatibility contracts.

Focused proof:

```text
cargo test -p melodee-cli --test legacy --test legacy_freeze
15 passed; 0 failed
cargo test -p melodee-server --test native_discovery_api \
  native_discovery_routes_use_durable_bounded_facts_and_preserve_wire_shapes -- --exact
1 passed; 0 failed
python3 -m unittest tests/python/test_legacy_rehearsal.py \
  tests/python/test_container_smoke.py
10 passed; 0 failed
cargo test -p melodee-server --test opensubsonic_annotation_api \
  --test opensubsonic_lists_api
24 passed; 0 failed
cargo test -p melodee-server --test jellyfin_media_api
18 passed; 0 failed
python3 scripts/run_r4_dual_run.py --only native
replay completed; report recorded 192 unexpected differences and exited 1
python3 scripts/run_r4_dual_run.py --only jellyfin --skip-build --keep
replay completed; report recorded 17 unexpected differences and exited 1
python3 scripts/run_r4_dual_run.py --only opensubsonic --skip-build
replay completed; report recorded 340 unexpected differences and exited 1
```

The rehearsal harness safety tests cover malformed URLs, bounded synthetic
shape, backup validation, required snapshot mappings, non-symlink canonical
roots, required source-backup validation, metadata-only media observations, and
the container smoke script. The updated `drill --help` was executed successfully; the synthetic drill above
also exercised the normal command path. The existing-snapshot path was not
run against a production resource because no approved snapshot URL, backup, or
canonical-root mapping was supplied.

The current-client evidence validator and migrated-target launcher were
included in the final local gate on 2026-08-19: `./scripts/check-smoke.sh` and
`./scripts/check.sh` passed; the full gate ran 174 Python tests, the Admin UI
static/type/lint/i18n/unit/build checks (44 files/248 tests), workspace
Clippy, all-feature Rust and PostgreSQL tests, and cargo-deny. This validates
the local acceptance tooling only. The five R4.5 matrix rows remain
`pending-external` until current Android, Android Auto/DHU, MeloAmp,
OpenSubsonic, and Jellyfin client runs are captured against a migrated target.

## Requirements and ADR disposition

## R4.4 package report — 2026-08-19 differential replay correction

- Exact files changed: `scripts/run_phase8_differential.py`,
  `tests/python/test_scripts.py`, `design/evidence/r4/dual-run-report-native.json`,
  and this report.
- Requirements: R4.4 differential classification was narrowed for six replay-created
  account lifecycle timestamp fields. At this package's replay point, R4.4 remained
  Partial because 192 native, 340 OpenSubsonic, and 17 Jellyfin differences remained
  unclassified and no
  decision-dependent metadata was implemented.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain
  Proposed. The timestamp rules are comparator behavior for replay volatility,
  not a metadata or compatibility decision.
- Tests: `python3 -m unittest tests/python/test_scripts.py -v` passed 31/31;
  `python3 scripts/run_r4_dual_run.py --only native` reran all 12 native groups,
  wrote the committed report, and exited 1 only because the remaining 192
  differences are still unclassified; `python3 scripts/validate_repository.py`,
  `python3 scripts/extract_requirements.py --check`, and `git diff --check` passed.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO,
  property, identifier, status, auth, pagination, or range contract changed.
  The rules match only six named native replay fields; usernames, emails,
  playlist fields, and other timestamps remain strict comparisons.
- Data/migration impact: none. The rules account for timestamps generated by
  capture/replay user creation or login and do not alter migrated source values.
- Security/data-loss impact: no source, target, media, or credential behavior
  changed; broad field masking was not added.
- Performance/operational impact: no runtime path changed. The native replay
  remains reproducible and fails closed while unexplained differences remain.
- Documentation impact: Updated — this report and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: release owner/compatibility decision owner must
  resolve catalog ordering, metadata, user-state, provider, playlist/share, and
  other residual semantics before R4.4 can complete.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.3 search diagnostic and query-matrix rehearsal — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, this report, the R5 report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the isolated drill now validates `search status` after rebuild, requiring an
  activated generation whose Artist/Release/Track counts and schema agree with migrated PostgreSQL
  authority. It records index bytes and exercises prefix autocomplete plus exact, normalized,
  Unicode, punctuation, broad, and low-selectivity query classes with bounded result limits and
  p50/p95 timings. The synthetic source includes explicit CJK and punctuation Artist values so
  those checks cannot pass vacuously. R4.3 remains Partial because production-catalog sizing,
  headroom, and operator evidence are absent.
- ADRs: accepted ADR-0148 remains applicable; no ADR was accepted or applied. ADR-0149,
  ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v` passed 19/19. The isolated
  `python3 scripts/legacy_rehearsal.py drill --songs 5 --output /tmp/melodee-r4-search-matrix.json`
  passed with zero migration mismatch gaps, an activated 50/100/5 search generation at schema 3,
  a 135,246-byte index, a 20-value bounded prefix result containing seeded values, all six query classes, and 100 ms p50/p95
  query timing. The JSON was written to `/tmp` and is not production evidence. The subsequent
  exact local `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 209
  Python tests, Admin UI 44 files/248 unit tests, the complete workspace/PostgreSQL Rust and
  doctest matrix, formatting, workspace Clippy with `-D warnings`, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior remain unchanged and separate.
- Data/migration impact: only the disposable synthetic source fixture gained deterministic Unicode
  and punctuation names for search coverage; PostgreSQL remains the search authority and no
  production source or canonical media was accessed.
- Security/data-loss impact: positive bounded validation; an apparently successful rebuild cannot be
  recorded as healthy when its active generation is missing, stale, mismatched, or over the query
  limit. No destructive behavior was added.
- Performance/operational impact: one bounded status read, one bounded autocomplete call, six bounded
  queries, index-size measurement, and process timing per drill; no production runtime path changed.
- Documentation impact: Updated — this report, the R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal
  rehearsal validation.
- Remaining risks and owner: release owner/operator still owns the real migrated catalog,
  production-sized Tantivy measurements, disk headroom, and operator-triggered diagnostic/retry
  evidence.
- External input still required: approved isolated production snapshot/canonical mapping and
  authorization for the production-sized rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.3 migrated-catalog search probes fail closed and redact snapshot values — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, this report, the R5 report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: snapshot-mode search validation now derives Artist representatives from the
  migrated PostgreSQL authority instead of using synthetic names or accepting zero-result probes.
  It requires exact, normalized, Unicode, punctuation, broad, and repeated-token
  low-selectivity classes, bounds every result set, and redacts snapshot-derived prefixes,
  expressions, expected values, and returned names from evidence. No requirement became
  terminal; R4.3 remains Partial pending the real production-sized target and operator evidence.
- ADRs: accepted ADR-0148 remains the migration/search authority; no ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v` passed 21/21;
  `python3 -m py_compile scripts/legacy_rehearsal.py` passed; the isolated
  `python3 scripts/legacy_rehearsal.py drill --songs 5 --output /tmp/melodee-r4-search-probe-v2.json`
  passed migration verification, backup/restore comparison, freeze, rollback, all six query
  classes, a 20-value prefix limit, and a low-selectivity minimum of 10 hits. The JSON remains
  temporary synthetic evidence and is not production evidence.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, identifiers,
  statuses, auth, pagination, and range behavior remain unchanged and separate.
- Data/migration impact: only the disposable synthetic rehearsal databases and temporary roots
  were used; the new snapshot path is read-only and queries PostgreSQL authority after import.
- Security/data-loss impact: positive fail-closed correction; missing required search classes now
  stop the rehearsal, and catalog values are not copied into snapshot evidence. No destructive
  behavior was added.
- Performance/operational impact: one bounded authority-name query, one bounded autocomplete,
  six bounded searches, and the existing index/timing measurements per drill; no runtime server
  path changed.
- Documentation impact: Updated — this report, the R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal
  rehearsal tooling.
- Remaining risks and owner: release owner/operator still owns production-sized Tantivy sizing,
  headroom, real-catalog representative availability, and operator diagnostic/retry evidence.
- External input still required: approved isolated production snapshot, canonical mapping, backup,
  and authorization for the production-sized rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.1/R5.1 baseline and path-map fail-closed validation — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, this report, the R5 report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the rehearsal now validates `--library-path-map` syntax with the same accepted
  rules as the Rust CLI before creating a target database. It also validates the CLI's versioned
  `melodee.legacy-baseline` JSON shape, frozen 52-table coverage, source identity/schema linkage,
  v2 snapshot marker, transaction read-only facts, media totals, and redacted/hashed configuration
  fingerprint. No requirement was marked terminal; R4.1 and R5.1 remain Partial pending real
  snapshot and topology evidence.
- ADRs: accepted ADR-0148 remains the migration authority; no ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v` passed 18/18. The isolated
  `python3 scripts/legacy_rehearsal.py drill --songs 5` passed against the actual CLI baseline
  output with zero verification mismatch gaps, full restore-content equality, freeze, rollback
  retention, and unchanged canonical media. Evidence was written to a temporary path and is not
  production evidence. The subsequent exact local `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh`
  exited 0 with 208 Python tests, Admin UI 44 files/248 unit tests, the complete
  workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with `-D warnings`,
  and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin contracts are unchanged.
- Data/migration impact: malformed operator mappings now fail before target creation; synthetic
  source/target databases remain isolated and no production source or canonical media was used.
- Security/data-loss impact: positive fail-closed correction; invalid mapping configuration cannot
  create partial target state, and baseline evidence cannot claim read-only or redaction guarantees
  from malformed JSON.
- Performance/operational impact: bounded string validation and in-memory JSON validation before
  target creation; no migration or production runtime path changed.
- Documentation impact: Updated — this report, the R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal
  rehearsal validation.
- Remaining risks and owner: release owner/operator still owns the approved snapshot, backup,
  canonical mapping, production-sized run, topology, restore target, and cutover evidence.
- External input still required: approved isolated production snapshot/backup/canonical mapping and
  authorization for the real rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.8 synthetic restore-content and fixture closure — 2026-08-20

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, this report, the R5 report,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the isolated synthetic R4.8 drill now validates the generated backup before
  restore and compares restored Artist, Release, Track, and user counts plus media file count and
  byte totals with the migrated target. Synthetic playlist and user-state fixtures now generate
  unique source identity pairs within their bounded capacity, so verifier failures represent
  migration defects rather than duplicate fixture rows. R4.8 and R5.1 remain Partial because
  production-equivalent backup, restore, traffic switch, and rollback evidence is still absent.
- ADRs: accepted ADR-0148 remains the migration authority; no ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v` passed 16/16; the
  isolated `python3 scripts/legacy_rehearsal.py drill --songs 5` passed end to end, including
  zero verification mismatch gaps, backup validation, full restore-content equality, freeze,
  failed-target retention, and canonical-media immutability. The post-change exact
  `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 206 Python tests,
  Admin UI 44 files/248 unit tests, the complete workspace/PostgreSQL Rust and doctest matrix,
  formatting, workspace Clippy with `-D warnings`, and cargo-deny. The generated drill evidence
  was written to a temporary path and is not authoritative production evidence.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior are unchanged.
- Data/migration impact: synthetic databases were isolated and source/media behavior remained
  bounded to the rehearsal. No production source, target, backup, or canonical media was used.
- Security/data-loss impact: positive; restore verification now checks complete measured authority
  and media content shape, and synthetic source identities no longer manufacture duplicate-state
  discrepancies. No destructive production behavior was added.
- Performance/operational impact: two bounded aggregate queries for restored media and catalog
  content; no production workload or migration algorithm changed.
- Documentation impact: Updated — this report, the R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because this is internal
  rehearsal tooling.
- Remaining risks and owner: release owner/operator must supply and authorize the approved
  approximately-5-TB snapshot, backup, canonical mapping, production-equivalent restore target,
  traffic switch, and rollback drill.
- External input still required: isolated production snapshot/backup/canonical-root mapping and
  explicit operator authorization for the real restore and cutover rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6 descriptor-bound topology race correction — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, this report,
  `design/evidence/r5/README.md`, `design/evidence/r5/release-gate-report.md`,
  and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the read-only integration probe now derives storage topology
  entirely from the validated descriptor facts. It no longer reopens the
  canonical pathname after validation, so a path replacement cannot change
  the reported same/cross-filesystem result. R4.6 and R5.1 remain Partial
  because production endpoints, backup/restore targets, and topology evidence
  are still unavailable.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151
  remain Proposed.
- Tests: `python3 -m unittest tests/python/test_r4_integration_probe.py -v`
  passed 10/10, including the held-descriptor topology regression. The new
  test fails if `Path.stat()` is used for the topology result; the source and
  backup probes are isolated in the test and no external endpoint is contacted.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes,
  DTOs, properties, identifiers, statuses, auth, pagination, and range
  behavior are unchanged.
- Data/migration impact: none; the probe remains read-only and does not alter
  source databases, target databases, canonical media, or backup contents.
- Security/data-loss impact: positive; the topology artifact cannot pair a
  held backup descriptor with a newly resolved canonical pathname after a
  replacement race.
- Performance/operational impact: removes one pathname `stat` from the
  topology check and reuses the bounded descriptor facts already collected.
- Documentation impact: Updated — this R4 report, the R5 report, the release
  gate report, and `IMPLMENTATION_OUTSTANDING.md`; no public documentation
  changed because user-visible behavior is unchanged.
- Remaining risks and owner: release owner/operator must provide and review
  the actual production storage and restore topology and authorize the
  external rehearsal.
- External input still required: approved isolated production snapshot,
  backup, canonical-root mapping, and explicit production-rehearsal
  authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 current all-surface differential refresh — 2026-08-20

- Exact files changed: `design/evidence/r4/dual-run-report-native.json`,
  `design/evidence/r4/dual-run-report-opensubsonic.json`,
  `design/evidence/r4/dual-run-report-jellyfin.json`, this report,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: all three migrated surface replays were rerun from the
  current tree. Native completed 12 groups with 267 classified and 185
  unclassified differences; OpenSubsonic completed 11 with 318 classified and
  330 unclassified; Jellyfin completed eight with 149 classified and 17
  unclassified. R4.4 remains Partial and no residual was waived.
- ADRs: no ADR was accepted or applied. ADR-0148 remains accepted migration
  authority; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 scripts/run_r4_dual_run.py --only native --skip-build`,
  `--only opensubsonic --skip-build`, and `--only jellyfin --skip-build`
  each completed its isolated replay and exited 1 fail-closed because
  unexplained differences remain. The three committed reports were parsed
  after the runs and contain exactly 185/330/17 unexpected entries.
- Compatibility impact: no route, wire DTO, property, identifier, status,
  auth, pagination, range, or intentional 410/501 behavior changed. Native v1,
  OpenSubsonic, and Jellyfin remain independently reported products.
- Data/migration impact: only disposable local source/target databases and
  temporary fixture roots were used; no production database or media root was
  accessed.
- Security/data-loss impact: differential comparison remained fail-closed;
  no broad comparator mask, synthetic aggregate play event, or disputed
  metadata mapping was introduced.
- Performance/operational impact: three full isolated replay passes were
  executed; no production runtime path changed.
- Documentation impact: Updated — this R4 report, the R5 release-gate report,
  and `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because
  these are internal release artifacts.
- Remaining risks and owner: release/compatibility owner must resolve all
  residual semantics and provide accepted ADR-0150/product decisions plus
  production-sized replay evidence.
- External input still required: accepted ADR-0150 disposition, approved
  migrated snapshot/topology, and authorization for final differential runs.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.1 complete documented CLI command audit — 2026-08-20

- Exact files changed: `scripts/check_command_help.py`,
  `tests/python/test_command_help.py`, this report,
  `design/evidence/r5/README.md`, `design/evidence/r5/release-gate-report.md`,
  and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.1 command reproducibility now has an executable audit for
  all 78 documented `melodee-cli` paths, including the complete top-level
  operator groups and every documented subcommand, plus
  `legacy_rehearsal.py drill --help`. No requirement was marked terminal;
  approved production baseline evidence remains external.
- ADRs: no ADR was accepted or applied. ADR-0148 remains the accepted
  migration authority; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 scripts/check_command_help.py` passed for 78 CLI paths and
  the rehearsal help path; `python3 -m unittest
  tests/python/test_command_help.py -v` passed 2/2; Python compilation,
  `git diff --check`, `./scripts/check-smoke.sh`, and the current-tree
  `./scripts/check.sh` passed. The full gate ran 197 Python tests, the
  Admin UI gate with 44 files and 248 unit tests, all workspace/PostgreSQL
  Rust tests and doctests, formatting, Clippy with warnings denied, and
  cargo-deny; the complete 78-path audit was rerun after the gate to include
  the nested notification-template command.
- Compatibility impact: none; only `--help` is invoked and no native v1,
  OpenSubsonic, or Jellyfin route, DTO, property, identifier, status, auth,
  pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: none; the child processes receive only `PATH` and
  never open a database, migration source, canonical media root, or backup.
- Security/data-loss impact: positive fail-closed audit boundary; local
  `.env`, credentials, and deployment settings cannot turn the check into an
  operational command.
- Performance/operational impact: 78 bounded CLI subprocesses and one Python
  help subprocess, each with a 30-second timeout; no persistent state.
- Documentation impact: Updated — this report, the R5 report and release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`; public command documentation was
  unchanged because all documented paths were verified.

### R4.4 current native differential rerun — 2026-08-20

- Exact files changed: `design/evidence/r4/dual-run-report-native.json`,
  this report, `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the current isolated migrated replay completed all 12 native
  groups and refreshed the committed report. It records 267 classified and
  185 unclassified field differences. The replay keeps seven aggregate
  play-count fields visible as migration residuals; no comparator rule or
  disputed ADR-0150 mapping was added. R4.4 remains Partial.
- ADRs: no ADR was accepted or applied. ADR-0148 remains the accepted
  migration authority; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 scripts/run_r4_dual_run.py --only native --skip-build`
  completed every native group and exited 1 fail-closed because 185
  unexplained differences remain. The isolated run used disposable
  `r4_dual_source`/`r4_dual_target` databases and regenerated the committed
  machine-readable report. Repository and smoke gates remain green after the
  probe correction.
- Compatibility impact: no native-v1 route, DTO, property, identifier,
  status, auth, pagination, range, or intentional 410/501 behavior changed;
  the report only records current replay results.
- Data/migration impact: the run rebuilt disposable migrated targets from the
  frozen local Phase 0 fixture and did not access production data or media.
- Security/data-loss impact: the differential remains fail-closed; no
  unexplained field was masked or waived. The residual aggregate play-count
  mapping is retained for decision/implementation review.
- Performance/operational impact: one full isolated native replay across all
  12 groups; no production runtime path changed.
- Documentation impact: Updated — this report, the R5 release-gate report,
  and `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because
  the replay artifact is internal release evidence.
- Remaining risks and owner: release/compatibility owner must resolve the
  current ordering, metadata, state, aggregate play-count, provider,
  playlist/share, and operational residuals.
- External input still required: accepted ADR-0150 disposition and any
  product/client decision needed for aggregate play-count semantics, plus the
  approved production-sized snapshot and final differential authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.
- Remaining risks and owner: release owner/operator must still run the
  commands against the approved isolated production snapshot and retain the
  resulting baseline; external production/client/ADR evidence remains open.
- External input still required: approved snapshot URL, backup location,
  canonical-root mapping, production topology, and ADR owner dispositions.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 ADR-0150 cross-surface decision inventory — 2026-08-20

- Exact files changed: `design/evidence/r4/adr-0150-decision-package.md`,
  `design/adr/ADR-0150-legacy-media-metadata-dispositions-for-phase-11-migration.md`,
  `design/adr/ADR-0149-phase-11-rehearsal-freeze-cutover-and-rollback-procedures.md`,
  `design/adr/ADR-0151-use-debian-trixie-runtime-base-for-release-images.md`,
  `tests/python/test_scripts.py`,
  this report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the ADR-0150 decision package now records current native,
  OpenSubsonic, and Jellyfin residual totals (185/330/17), overlapping
  metadata/state family counts, compatibility impact, schema impact,
  operational/rollback impact, and a field-family disposition matrix. No
  requirement was marked terminal and no residual was waived.
- ADRs: ADR-0150 remains Proposed and no decision was accepted or applied;
  ADR-0149 and ADR-0151 remain Proposed.
- Tests: the three committed JSON reports were parsed successfully; their
  `unexpected` arrays contained exactly 185 native, 330 OpenSubsonic, and 17
  Jellyfin entries. `python3 scripts/validate_repository.py`,
  `python3 scripts/extract_requirements.py --check`, and `git diff --check`
  passed; `python3 -m unittest tests/python/test_scripts.py -v` passed 34/34,
  and `./scripts/check-smoke.sh` passed, including 116 Rust library tests.
  The subsequent current-tree `./scripts/check.sh` also passed with 198
  Python tests, the Admin UI gate, all workspace/PostgreSQL Rust tests and
  doctests, formatting, Clippy with warnings denied, and cargo-deny.
- Compatibility impact: none; no route, DTO, property, identifier, status,
  authentication, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: none; this is decision evidence only and adds no
  schema, mapping, source write, canonical-media operation, or comparator rule.
- Security/data-loss impact: positive; the package keeps unresolved metadata
  visible and prohibits identity use, broad masks, or destructive migration
  behavior from being inferred.
- Performance/operational impact: none at runtime; the matrix identifies the
  capacity, rebuild, and rollback evidence required if a projection is chosen.
- Documentation impact: Updated — this R4 report, the ADR-0150 decision
  package, and `IMPLMENTATION_OUTSTANDING.md`; public documentation remains
  unchanged because no behavior is authorized or implemented.
- Remaining risks and owner: release owner and compatibility/product owner
  must choose field-level dispositions, including ordering, metadata,
  user-state, playlist/share, provider, and instant-mix semantics.
- External input still required: accepted ADR-0150 disposition and the
  approved migrated production snapshot/topology for final replay.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 current native migrated replay and ADR-0150 triage — 2026-08-20

- Exact files changed: `design/evidence/r4/dual-run-report-native.json`, this report,
  `design/evidence/r5/README.md`, and `design/evidence/r5/release-gate-report.md`.
- Requirements: a fresh native-v1 migrated replay completed all configured scenario groups and
  retained 185 unclassified differences. The replay confirms the current residuals rather than
  silently carrying an older report; no requirement was marked terminal.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
  Genre, bit-depth, aggregate play-counter, free-form metadata, and related projection residuals
  remain governed by the ADR-0150 decision package; no disputed mapping was implemented.
- Tests: `python3 scripts/run_r4_dual_run.py --only native --skip-build` executed against a
  disposable migrated target and exited 1 as required by the fail-closed differential gate. The
  report contains 267 classified and 192 unclassified field differences. Current examples include
  the 117-entry catalog ordering/projection cluster, reference-only genre absence, and source
  aggregate play counters that must not be converted into synthetic play events.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO family, property,
  identifier, status, auth, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: no migration mapping, schema, source database, or canonical media was
  changed; the replay used isolated disposable databases and roots.
- Security/data-loss impact: fail-closed residual reporting was preserved; no comparator mask or
  metadata invention was added.
- Performance/operational impact: the replay remains bounded and reproducible, but it is not
  production-sized evidence and does not establish release readiness.
- Documentation impact: Updated — this R4 report and the corresponding R5 evidence reports;
  no public behavior documentation changed.
- Remaining risks and owner: release owner/compatibility owner must decide ADR-0150 and the
  separate ordering, playlist/share, user-state, provider, and production-traffic dispositions.
- External input still required: accepted ADR-0150 disposition and an approved production-shaped
  migrated snapshot/topology for final differential reruns.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 OpenSubsonic clean-target share-fixture isolation — 2026-08-20

- Exact files changed: `scripts/legacy_oracle.py`, `scripts/run_r4_dual_run.py`,
  `tests/python/test_native_v1_capture.py`, this report,
  `design/evidence/r4/dual-run-report-opensubsonic.json`,
  `design/evidence/r5/README.md`, `design/evidence/r5/release-gate-report.md`,
  and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.4 replay reproducibility is strengthened. The deterministic native public-share
  fixture is now included only for `native-community.json`; OpenSubsonic and Jellyfin clean-target
  replays omit native-only setup. The current OpenSubsonic report dropped the three share-count
  fixture residuals, from 333 to 330 unclassified differences. No requirement was marked terminal.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed; this
  does not decide native share update semantics or legacy media metadata projection.
- Tests: the updated fixture-isolation regression was included in the subsequent full Python
  gate, which passed 193/193 tests;
  `python3 -m py_compile scripts/legacy_oracle.py scripts/run_r4_dual_run.py` passed;
  `python3 scripts/legacy_oracle.py --help` and `python3 scripts/legacy_oracle.py seed --help`
  passed; `python3 scripts/run_r4_dual_run.py --only opensubsonic --skip-build` completed the
  migrated 11-group replay, wrote the committed report, and exited 1 fail-closed because 330
  unexplained differences remain.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO, property, identifier,
  status, auth, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: only explicitly named disposable source/target databases and temporary
  roots were used; the change controls fixture seeding before migration and does not alter source
  or canonical media.
- Security/data-loss impact: positive reproducibility correction; a native-only share cannot leak
  into a clean OpenSubsonic/Jellyfin replay, and no comparator exclusion was added.
- Performance/operational impact: no runtime impact; one deterministic seed branch is selected per
  compatibility surface during isolated replays.
- Documentation impact: Updated — this report, the R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: release owner/compatibility owner must resolve the remaining 330
  OpenSubsonic differences and the other surface residuals; ADR-dependent metadata/state/share
  decisions and external evidence remain open.
- External input still required: accepted ADR-0150 disposition and approved migrated
  snapshot/topology for final differential closure.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

## R4.4 package report — 2026-08-19 OpenSubsonic contributor projection

- Exact files changed: `apps/melodee-server/src/opensubsonic/annotation.rs`,
  `design/evidence/r4/README.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.4 received a narrow OpenSubsonic projection correction. Linked and
  name-only contributor roles now restore the captured legacy enum casing (for example,
  `FeaturedArtist`), and the standalone contributor `ArtistID3` shape keeps the captured
  `albumCount: 0` instead of querying an unrelated catalog aggregate. At this package's
  replay point, R4.4 remained Partial with 192 native, 340 OpenSubsonic, and 17 Jellyfin
  unclassified differences.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed;
  no metadata mapping or schema decision was inferred from this wire-projection correction.
- Tests: `cargo fmt --all` passed; `cargo test -p melodee-server --lib
  opensubsonic::annotation::tests -- --nocapture` passed 3/3; `cargo test -p
  melodee-server --test opensubsonic_browse_api -- --nocapture` passed 23/23;
  `python3 scripts/run_phase8_differential.py --only opensubsonic --skip-build` completed
  its disposable frozen-fixture replay with 321 classified and 0 unexpected differences.
  That fixture result is not the migrated-snapshot report: the latter remains the
  authoritative R4 artifact at that package point and retained 340 unclassified differences.
  The later current-tree replay is recorded above. The local smoke and
  full gates were rerun after the implementation; the full gate ran 187 Python tests, Admin
  UI 44 files/248 tests, all workspace/PostgreSQL Rust tests, formatting, Clippy, and
  cargo-deny successfully.
- Compatibility impact: OpenSubsonic contributor response values now match the captured
  legacy casing and contributor aggregate shape. No route, DTO family boundary, identifier,
  status, auth, pagination, or range contract was changed; native v1 and Jellyfin were not
  changed.
- Data/migration impact: none. PostgreSQL role values remain canonical migration data; only
  the OpenSubsonic adapter serialization changed. No schema, source database, target database,
  canonical media, or search projection was mutated.
- Security/data-loss impact: no new write, delete, path, credential, or retention behavior;
  the adapter correction only changes response serialization and preserves unknown role values.
- Performance/operational impact: contributor lookup removes an unnecessary standalone artist
  aggregate join and remains bounded by the requested track ids; no production-sized latency
  claim is made.
- Documentation impact: Updated — this internal R4 evidence report and
  `IMPLMENTATION_OUTSTANDING.md`; no public operational behavior or installation guidance
  changed.
- Remaining risks and owner: release owner/compatibility owner must resolve the remaining
  migrated-snapshot metadata, ordering, user-state, provider, playlist/share, and other
  residuals before R4.4 can complete. ADR-0150-dependent metadata remains blocked on its
  decision owner.
- External input still required: accepted ADR-0150 disposition and an approved migrated
  snapshot/topology for final differential reruns.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

## R4.1 package report — 2026-08-19 source-backup validation

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`,
  `docs/src/content/docs/installation/legacy-migration.md`,
  `design/evidence/r4/README.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: existing-snapshot rehearsal now requires an explicit
  `--source-backup`, validates the custom-format dump before target creation,
  records its byte count, SHA-256, and TOC-entry count, and rejects direct or
  parent symlink components. R4.1 remains Partial because the approved
  production snapshot and backup have not been supplied or run.
- ADRs: no ADR was accepted or applied; ADR-0149, ADR-0150, and ADR-0151
  remain Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -v`
  passed 10/10; `python3 -m py_compile scripts/legacy_rehearsal.py`,
  `python3 scripts/legacy_rehearsal.py drill --help`, `./scripts/check-smoke.sh`,
  and `git diff --check` passed. The subsequent `./scripts/check.sh` also passed
  with 184 Python tests, Admin UI 44 files/248 tests, all workspace/PostgreSQL
  tests, formatting, Clippy, and cargo-deny. No external database, backup, or
  canonical media root was opened.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes,
  DTOs, identifiers, statuses, authentication, pagination, and range behavior
  are unchanged.
- Data/migration impact: existing-source mode remains read-only; validation
  occurs before any disposable Rust target database is created and does not
  restore, modify, or delete the supplied source backup.
- Security/data-loss impact: malformed, empty, structurally invalid, symlinked,
  or path-indirect backup inputs fail closed without emitting connection values
  or backup contents.
- Performance/operational impact: one bounded filesystem inspection and
  `pg_restore --list` validation are added before the existing rehearsal; the
  large canonical root is still metadata-only unless `--hash-media` is chosen.
- Documentation impact: Updated — `docs/src/content/docs/installation/legacy-migration.md`,
  this report, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: the release owner/operator must provide the
  isolated production snapshot, backup artifact, canonical-root mapping, and
  authorization for the real rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

## R4.2 package report — 2026-08-20 disk-budget preflight

- Exact files changed: `scripts/legacy_rehearsal.py`,
  `tests/python/test_legacy_rehearsal.py`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the rehearsal harness now accepts explicit minimum free-byte and
  free-space-ratio thresholds, records the observed `preflight.disk` metrics, and
  fails before target creation when either threshold is not met. R4.2 remains
  Partial because the approved approximately 5-TB snapshot and repeated real
  topology measurements are still absent.
- ADRs: none accepted or applied; ADR-0149, ADR-0150, and ADR-0151 remain
  Proposed.
- Tests: `python3 -m unittest tests/python/test_legacy_rehearsal.py -q` passed
  13/13; `python3 scripts/legacy_rehearsal.py drill --help`, Python compilation,
  repository validation, requirements validation, `git diff --check`, and the
  latest smoke gate passed. The full local gate had already passed 189 Python
  tests and all Rust/PostgreSQL, Admin UI, formatting, Clippy, and cargo-deny
  checks before this narrow follow-up.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes,
  DTOs, identifiers, statuses, authentication, pagination, and range behavior
  are unchanged.
- Data/migration impact: no source, target, or canonical-media mutation was
  added; the preflight applies only to the disposable target/search workdir.
- Security/data-loss impact: an operator can reproduce insufficient-headroom
  handling without creating a partial target, and invalid thresholds fail before
  filesystem inspection; no capacity claim is inferred for production.
- Performance/operational impact: one bounded `statvfs`-equivalent disk query
  precedes target creation and adds the configured thresholds to machine-readable
  evidence.
- Documentation impact: Updated — this report and `IMPLMENTATION_OUTSTANDING.md`;
  no public behavior documentation changed.
- Remaining risks and owner: release owner/operator must execute the preflight
  and full drill against the approved production topology, including actual
  failure/retry and headroom observations.
- External input still required: approved isolated snapshot, backup, canonical
  mapping, topology, and authorization for the real rehearsal.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

## R4.4 package report — 2026-08-19 native play-ledger projection

- Exact files changed: `apps/melodee-server/src/native/mod.rs`,
  `apps/melodee-server/tests/native_catalog_api.rs`,
  `design/evidence/r4/dual-run-report-native.json`, and this report.
- Requirements: R4.4 received a narrow native-v1 correction: existing global
  `native_track_plays` facts now populate the existing `Song.playCount` field
  for both single-song and batched catalog projections, even when the
  requesting user has no `native_track_user_states` row. R4.4 remains Partial;
  the migrated replay still records 192 unclassified native differences,
  including 23 play-count fields.
- ADRs: no ADR was accepted or applied. ADR-0150 remains Proposed; the
  migration's legacy `Songs.PlayedCount` reference-only boundary was not
  converted into fabricated play events or a new mapping.
- Tests: `cargo build -p melodee-server` passed; `cargo test -p melodee-server
  --test native_catalog_api --all-features` passed 7/7; the rebuilt
  `python3 scripts/run_r4_dual_run.py --only native --skip-build` replayed all
  native groups, updated the committed report, and exited 1 with 192
  unclassified differences. The focused regression proves counts 3/3 through
  `/api/v1/songs/{id}` and `/api/v1/albums/{id}/songs` with no user-state row.
- Compatibility impact: no route, property, identifier, status, auth,
  pagination, or range contract changed. The existing native-v1 `playCount`
  property now reads the already-authoritative global play ledger; OpenSubsonic
  and Jellyfin DTOs and readers were not changed.
- Data/migration impact: no schema or migration change; no source, target, or
  canonical media mutation. Legacy `Songs.PlayedCount` remains reference-only
  in the migration until the ADR-0150 decision owner resolves its semantics.
- Security/data-loss impact: no new write or deletion path; play facts are
  read-only for this projection and overflow remains fail-closed.
- Performance/operational impact: one bounded correlated count per requested
  Track in the existing single/batch annotation query; the indexed
  `native_track_plays(track_id, played_at DESC, id)` path remains available.
- Documentation impact: Updated — this report and
  `IMPLMENTATION_OUTSTANDING.md`; public behavior documentation did not change
  because the wire contract was corrected rather than expanded.
- Remaining risks and owner: release owner/ADR-0150 decision owner must
  decide how legacy aggregate `Songs.PlayedCount` is represented after
  migration; compatibility owner must then rerun all three differentials.
- External input still required: accepted ADR-0150 disposition and the
  approved production snapshot/rehearsal required for final differential
  evidence.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

## R4.6 package report — 2026-08-19 OIDC probe hardening

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, and this report.
- Requirements: R4.6 local production-probe preparation now validates the
  discovered OIDC issuer and JWKS endpoint, bounds the key set to 256 entries,
  and records only key count/types; production integration acceptance remains
  external and no requirement was marked terminal.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain
  Proposed.
- Tests: `python3 -m unittest tests/python/test_r4_integration_probe.py -v`
  passed 5/5; `python3 -m py_compile scripts/r4_integration_probe.py` passed.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO,
  property, identifier, status, auth, pagination, or range contract changed.
- Data/migration impact: none; the probe remains read-only and does not access
  production databases or canonical media.
- Security/data-loss impact: JWKS key material is never emitted; responses are
  bounded and endpoint output remains fingerprinted. No credentials are
  accepted or sent by the probe.
- Performance/operational impact: one bounded read-only request is added after
  explicitly requested OIDC discovery; the response remains capped at 4 MiB.
- Documentation impact: Updated — this internal R4 evidence report.
- Remaining risks and owner: release owner/operator must still provide the
  approved non-production OIDC endpoint and test account, then execute the
  real provider, proxy/TLS, SMTP, Last.fm, podcast, storage, and restore probes.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

## R4.5 package report — 2026-08-19

- Exact files changed: `scripts/run_phase8_client_smoke.py`,
  `scripts/validate_r4_client_acceptance.py`, `tests/python/test_phase8_client_smoke.py`,
  `tests/python/test_r4_client_acceptance.py`, `compatibility/clients/r4/README.md`,
  `compatibility/clients/r4/acceptance-matrix.json`, `scripts/check-smoke.sh`,
  `scripts/check.sh`, `IMPLMENTATION_OUTSTANDING.md`, and this report.
- Requirements: R4.5/PRD-TEST-004 local launch and evidence-shape preparation is satisfied;
  physical-client acceptance remains external, so no requirement registry status was changed.
- ADRs: no new decision was applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the focused launcher/validator run passed 10 tests before this validator hardening;
  the R4.5 validator regression suite now passes 6 tests, including rejection of a missing
  repository-relative evidence file; `./scripts/check-smoke.sh` passed;
  `./scripts/check.sh` passed with 174 Python tests, Admin UI 44 files/248 tests plus typecheck,
  lint, i18n, and build, workspace Clippy, all-feature Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO, property, identifier,
  status, auth, pagination, or range contract changed. The Feishin launcher adds only a no-seed,
  no-drop path for an explicitly isolated migrated target.
- Data/migration impact: fixture mode retains its disposable seed/drop lifecycle; existing-target
  mode reads the database URL from an environment variable, validates a non-symlink storage root,
  and never seeds, truncates, or drops the supplied database or media root.
- Security/data-loss impact: credentials are not printed; result evidence rejects obvious secrets,
  absolute/escaping paths, and evidence paths that do not resolve to an existing file inside the
  repository. No production database, media root, or provider was accessed.
- Performance/operational impact: no client latency claim was added. The launcher records client
  evidence separately from the synthetic rehearsal and keeps the target cleanup boundary explicit.
- Documentation impact: Updated — `compatibility/clients/r4/README.md`,
  `design/evidence/r4/README.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: all five current-client rows are `pending-external`; the release owner
  must provide current Android phone, Android Auto/DHU, MeloAmp, OpenSubsonic, and Jellyfin clients
  plus an approved migrated target and operator.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.5 launcher readiness and path containment — 2026-08-19

- Exact files changed: `scripts/run_phase8_client_smoke.py`,
  `tests/python/test_phase8_client_smoke.py`,
  `compatibility/clients/r4/README.md`, and this report.
- Requirements: the migrated-target launcher now rejects symlinked existing
  storage-root parents and waits for both local helper listeners before
  starting the client probe. R4.5 remains Partial because all five current
  client rows require external migrated-target runs.
- ADRs: no ADR was accepted or applied; ADR-0149/0150/0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_phase8_client_smoke.py -v`
  passed 8/8; `./scripts/check-smoke.sh` passed after the change. The prior
  current-tree full gate passed with 178 Python tests, Admin UI 44 files/248
  tests, workspace Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; the launcher does not change any API product
  route, DTO, identifier, status, auth, pagination, or range behavior.
- Data/migration impact: existing-target mode remains no-seed, no-drop, and
  no-media-mutation; the added checks only fail closed before launching.
- Security/data-loss impact: the launcher cannot follow a symlinked storage
  parent and cannot begin client automation before helper listeners are ready.
- Performance/operational impact: bounded local readiness waits add at most
  30 seconds per helper on startup; normal startup is connection-ready based.
- Documentation impact: Updated — `compatibility/clients/r4/README.md` and
  this report; no public product behavior changed.
- Remaining risks and owner: physical-client acceptance, approved migrated
  target, and operator evidence remain release-owner owned.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6 integration-probe path containment — 2026-08-19

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, `IMPLMENTATION_OUTSTANDING.md`,
  and this report.
- Requirements: canonical-root and backup probes now reject a symlink in any
  existing path component, including a symlinked parent, before collecting
  filesystem or backup facts. R4.6 remains Partial because no production
  provider or deployment endpoint has been authorized or exercised.
- ADRs: no ADR was accepted or applied; ADR-0149/0150/0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_r4_integration_probe.py -v`
  passed 6/6; `python3 scripts/validate_repository.py`,
  `python3 scripts/extract_requirements.py --check`, `git diff --check`,
  `./scripts/check-smoke.sh`, and the current `./scripts/check.sh` all passed.
  The full gate included 177 Python tests, Admin UI 44 files/248 tests,
  workspace Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin wire
  behavior changed.
- Data/migration impact: none; the probe remains read-only and does not open a
  database or mutate canonical media.
- Security/data-loss impact: the probe no longer follows a supplied symlinked
  parent while validating canonical or backup topology; unreadable components
  fail closed.
- Performance/operational impact: one bounded component-by-component `lstat`
  pass per supplied path.
- Documentation impact: Updated — this internal report and
  `IMPLMENTATION_OUTSTANDING.md`; public behavior documentation did not change.
- Remaining risks and owner: external endpoint credentials, deployment proxy,
  backup/restore targets, and operator acceptance remain release-owner owned.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6 bounded proxy/TLS header probe — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, and this report.
- Requirements: R4.6 local probe preparation now supports an explicitly supplied
  `--proxy-url`, optional HTTPS enforcement, and narrow response-header assertions. Header values
  are compared in memory and omitted from evidence; production proxy/TLS acceptance remains
  external, so no requirement was marked terminal.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_r4_integration_probe.py -q` passed 7/7;
  `python3 scripts/r4_integration_probe.py --help` and Python compilation passed; the latest
  repository validation, requirement check, and smoke gate passed.
- Compatibility impact: none; the probe is read-only and no native v1, OpenSubsonic, or Jellyfin
  route, header, auth, range, or DTO behavior changed.
- Data/migration impact: none; no database, canonical media, backup, or deployment state is
  mutated.
- Security/data-loss impact: expected response values are never emitted, endpoint identities remain
  fingerprinted, HTTPS can be required, and no cookies or credentials are accepted.
- Performance/operational impact: one bounded GET and a narrow in-memory header comparison when
  explicitly requested; the existing 4 MiB response cap remains in force.
- Documentation impact: Updated — this internal R4 report; public behavior documentation did not
  change.
- Remaining risks and owner: release owner/operator must run the probe against the approved
  deployment proxy/TLS endpoint and retain the sanitized result.
- External input still required: approved endpoint, expected header policy, TLS termination
  ownership, and the rest of the R4.6 provider/storage/restore inputs.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6 integration-probe redirect containment — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.6 probe preparation now validates malformed URL ports and uses a no-redirect
  HTTP opener for health, range, OIDC, podcast, Last.fm, and proxy/TLS checks. Backup inspection
  now opens every path component and the regular-file leaf through held no-follow descriptors,
  rejects FIFOs/special files before reads, and passes the held descriptor to `pg_restore`. A
  redirect is reported as the supplied endpoint's response rather than followed to an unreviewed
  target; production integration acceptance remains external and no requirement was marked
  terminal.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_r4_integration_probe.py -v` passed 9/9,
  including redirect, malformed-port, leaf-symlink, and FIFO regressions; `python3 -m py_compile
  scripts/r4_integration_probe.py`, `python3 scripts/r4_integration_probe.py --help`,
  `git diff --check`, repository validation, requirements-index validation, override audit,
  `./scripts/check-smoke.sh`, and the exact-tree `./scripts/check.sh` passed. The full gate ran
  203 Python tests, Admin UI typecheck/lint/i18n/build and 248 unit tests, the full
  workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with warnings
  denied, and cargo-deny.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, or range behavior changed.
- Data/migration impact: none; the read-only probe does not connect to a migration source or
  target, modify canonical media, or send provider credentials.
- Security/data-loss impact: positive fail-closed boundary; operator-supplied endpoint checks
  cannot silently follow a redirect to a different host, malformed ports fail before network
  access, and a backup path replacement cannot redirect `pg_restore` or block it on a FIFO. No
  secret, cookie, response body, or credential is recorded by the new path.
- Performance/operational impact: one opener dispatch per explicitly requested HTTP check with the
  existing 10-second timeout and 4 MiB response bound; redirecting deployments now require the
  operator to supply and validate the final endpoint explicitly.
- Documentation impact: Updated — this internal R4 report and `IMPLMENTATION_OUTSTANDING.md`;
  public behavior documentation did not change because the probe is internal tooling.
- Remaining risks and owner: release owner/operator must run the hardened probe against approved
  deployment endpoints and retain sanitized results; SMTP delivery, provider authorization,
  storage topology, restore, and physical deployment evidence remain open.
- External input still required: approved endpoint list, expected proxy header policy, non-sensitive
  provider/test accounts, backup/restore targets, and operator authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.6 integration-probe descriptor-bound topology facts — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `tests/python/test_r4_integration_probe.py`, this report,
  `design/evidence/r5/README.md`, `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.6 local probe evidence now obtains canonical free/total bytes, headroom, and
  device identity from a held no-follow directory descriptor, and reuses the held backup
  descriptor's device identity for same/cross-filesystem topology reporting. No requirement was
  marked terminal; production storage and restore validation remain external.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: the focused integration-probe suite passed 9/9; Python compilation, `--help`,
  `git diff --check`, repository validation, requirements-index validation, override audit,
  `./scripts/check-smoke.sh`, and the exact-tree `./scripts/check.sh` passed. The full gate ran
  203 Python tests, Admin UI typecheck/lint/i18n/build and 248 unit tests, the full
  workspace/PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with warnings
  denied, and cargo-deny.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, or range behavior changed.
- Data/migration impact: no source, target, canonical media, backup, or external endpoint was
  contacted or modified; this remains read-only local tooling.
- Security/data-loss impact: positive evidence-integrity correction; topology facts cannot be
  assembled from a backup pathname that changed after validation, and directory capacity facts
  are tied to the descriptor that was opened without following path components.
- Performance/operational impact: one descriptor-relative directory walk and filesystem-stat call
  per explicitly requested root; no server or migration runtime behavior changed.
- Documentation impact: Updated — this R4 report, the R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because the probe is internal.
- Remaining risks and owner: release owner/operator must run the probe against approved production
  canonical, backup, restore, proxy, provider, and deployment targets and retain sanitized output.
- External input still required: approved isolated production snapshot/backup and canonical-root
  mapping, deployment endpoints/test accounts, and operator authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

R3's migration requirements remain evidenced by the existing report. R4
extends that evidence for `PRD-MIG-007` rollback behavior and the rehearsal
legs of `PRD-TEST-005`, `PRD-TEST-006`, `PRD-TEST-007`, `PRD-PERF-004`,
`PRD-SCALE-001`, and `PRD-SCALE-002`. The requirement registry is not marked
terminal until the external R4 legs below are completed.

No compatibility DTO, status, identifier, or authentication contract was
redesigned. The search-suggest, login-version, cookie-cache, and chart-error
implementations were corrected against frozen legacy behavior without
changing the intended public contract. Migration remains source-read-only,
non-destructive, and media adopting rather than relocating or re-encoding.

### R4.6 integration-probe artifact and parser hardening — 2026-08-20

- Exact files changed: `scripts/r4_integration_probe.py`,
  `scripts/check_command_help.py`, `tests/python/test_r4_integration_probe.py`,
  `tests/python/test_command_help.py`, this report, the R5 report, the release-gate report,
  and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.6 local probe preparation now validates its stable schema-1 JSON artifact,
  bounds and cross-checks range measurements, rejects malformed OIDC documents and XML DTD/entity
  declarations as controlled probe failures, and includes the integration probe in the sanitized
  command-help audit. No requirement was marked terminal; real endpoints and operator evidence
  remain external.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 -m unittest tests.python.test_r4_integration_probe tests.python.test_command_help
  -v` passed 15/15; Python compilation, `python3 scripts/r4_integration_probe.py --help`, and
  `python3 scripts/check_command_help.py` passed. The subsequent exact `cargo fmt --all &&
  MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh` exited 0 with 212 Python tests,
  Admin UI 44 files/248 unit tests, the complete workspace/PostgreSQL Rust and doctest matrix,
  formatting, workspace Clippy with warnings denied, and cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, identifiers,
  statuses, auth, pagination, range behavior, and intentional 410/501 behavior are unchanged.
- Data/migration impact: none; the probe remains read-only and no database, canonical media,
  backup, or external endpoint was contacted.
- Security/data-loss impact: positive fail-closed correction; malformed provider responses cannot
  escape as uncontrolled exceptions, XML entity declarations are rejected, bounded ranges must
  match their measured body, and generated evidence rejects secret-shaped values.
- Performance/operational impact: validation is in-memory over already bounded response metadata;
  no runtime server or migration path changed.
- Documentation impact: Updated — this R4 report, the R5 report, the release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; public behavior documentation did not change because this is
  internal probe tooling.
- Remaining risks and owner: release owner/operator must run the validated probe against approved
  deployment/provider/storage/restore endpoints and retain sanitized results; R4.6 remains Partial.
- External input still required: approved endpoint list, expected proxy policy, non-sensitive test
  accounts, backup/restore targets, and operator authorization.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 primary-release aggregate correction and rebuilt replay — 2026-08-20

- Exact files changed: `apps/melodee-cli/src/legacy/import.rs`,
  `crates/melodee-persistence/src/catalog.rs`,
  `crates/melodee-persistence/src/catalog_mutations.rs`,
  `apps/melodee-server/tests/jellyfin_media_api.rs`, the three surface-scoped
  differential reports, this report, `design/evidence/r5/README.md`,
  `design/evidence/r5/release-gate-report.md`, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the durable `artist_catalog_aggregates.release_count` projection now counts
  distinct primary `Release` billing rows in legacy import, normal catalog import, and full
  maintenance rebuild paths. The regression test forces the full maintenance rebuild before
  checking Jellyfin `ChildCount`, proving featured/secondary credits cannot inflate the count.
  R4.4 remains Partial; no requirement was marked terminal.
- ADRs: no ADR was accepted or applied. This correction follows the existing captured
  primary-release contract; ADR-0149, ADR-0150, and ADR-0151 remain Proposed. No ADR-0150
  metadata mapping was implemented.
- Tests: `cargo build -p melodee-cli -p melodee-server` passed; the focused Jellyfin regression
  passed 1/1; the rebuilt isolated replays exited 1 fail-closed with 181 native, 322
  OpenSubsonic, and 17 Jellyfin unclassified differences. `git diff --check` passed.
- Compatibility impact: intentional correction to existing artist release-count projections;
  native v1, OpenSubsonic, and Jellyfin wire DTOs and route contracts remain separate, and no
  route, property, identifier, status, auth, pagination, range, or intentional 410/501 behavior
  was changed.
- Data/migration impact: legacy source, target databases, and canonical media used by replay were
  disposable and isolated. The aggregate is rebuilt from authoritative migrated PostgreSQL
  `release_artists` rows; no source mutation, media movement, or identity regeneration occurs.
- Security/data-loss impact: positive bounded correction; it removes a false count without
  synthesizing metadata or user state, and the differential harness remained fail-closed.
- Performance/operational impact: one correlated distinct-primary-release count in existing
  projection maintenance; no new dependency or runtime service was introduced. Production-sized
  performance measurements remain absent.
- Documentation impact: Updated — this report, the R5 report, the R5 release-gate report, and
  `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because the correction restores
  an existing compatibility invariant rather than adding user-visible capability.
- Remaining risks and owner: release/compatibility owner must resolve the remaining 181/322/17
  residuals, including ADR-0150 metadata/state/order families and the separate playlist/provider
  decisions; production-sized and external acceptance evidence remains open.
- External input still required: accepted ADR-0149/0150/0151 dispositions, approved production
  snapshot/topology, and release-owner authorization for external differential and cutover runs.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

## Remaining R4 acceptance

These require deployment credentials, production-shaped snapshots, or client
hardware and cannot be honestly generated by this isolated local harness:

- R4.2 repeat on the real approximately-5 TB production-shaped snapshot;
- R4.4 dual-run replay against equivalent C# and Rust snapshots. The local
  executable reports are `dual-run-report-native.json`,
  `dual-run-report-opensubsonic.json`, and `dual-run-report-jellyfin.json`:
  all three ran from isolated per-file migrated snapshots. Native stream
  HEAD/full/range requests passed; the current reports record 181, 322, and 17
  unclassified differences respectively. None is silently waived and each
  still requires compatibility classification or fixes.
- R4.5 physical Android, Android Auto/DHU, MeloAmp, OpenSubsonic, and Jellyfin
  client workflows. Current migrated-client checklist and launch preparation
  are in `compatibility/clients/r4/`, but every matrix row remains pending
  external device/client evidence;
- R4.6 production Google OIDC, Last.fm, SMTP, real podcast, proxy/TLS, and
  storage-topology integrations; and
- R4.7/R4.8 operator-authorized production freeze, traffic switch, and rollback.

The executable entry points are `scripts/run_r4_dual_run.py`,
`scripts/legacy_rehearsal.py`, and `scripts/r4_integration_probe.py`; their
reports must be attached here after those
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

- Updated — this internal R4 evidence report, the migration rehearsal and
  integration-probe scripts, the ADR-0149 and ADR-0150 decision packages, the
  rehearsal safety tests, and the public migration command reference. Public
  migration documentation remains truthful about the rehearsal-gated
  production cutover and therefore has no support status change yet.

### R4.2/R5.1 existing-snapshot rehearsal and composite user-state correction — 2026-08-20

- Exact files changed: `apps/melodee-cli/src/legacy/import_state2.rs`,
  `apps/melodee-cli/src/legacy/verify.rs`, `apps/melodee-cli/tests/legacy.rs`,
  `scripts/legacy_rehearsal.py`, `tests/python/test_legacy_rehearsal.py`,
  `design/evidence/r4/rehearsal-drill-existing-snapshot.json`, this report,
  `design/evidence/r5/README.md`, `design/evidence/r5/release-gate-report.md`,
  and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the migration now handles multiple users annotating one Track,
  Release, or Artist without treating the catalog entity UUID as a unique user-state
  target. Verification compares the composite state key directly. The rehearsal seed
  also bounds small-shape playlist rows to the available unique pair space. No requirement
  was marked terminal.
- ADRs: ADR-0148 remains the accepted migration authority; no schema change or new ADR
  was applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `cargo fmt --all` passed; the full CLI legacy suite passed 11/11; the new
  `user_state_import_allows_multiple_users_on_one_track` regression passed 1/1; the
  rehearsal safety module passed 14/14. The existing-snapshot rehearsal passed with
  50 Artists, 100 Releases, 101 Tracks, 10 users, 0 verification mismatch gaps,
  0 duplicate public IDs, successful search rebuild, backup/restore count equality,
  `source_mutated_by_harness=false`, and `canonical_tree_unchanged=true`.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, range, or intentional 410/501 behavior changed.
- Data/migration impact: positive correction to user-state idempotency and verification;
  source and canonical media remain read-only. The committed rehearsal artifact uses only
  an isolated synthetic existing snapshot and backup.
- Security/data-loss impact: prevents a production-shaped multi-user state collision from
  aborting import and avoids false mapping uniqueness for composite facts; no destructive
  operation or schema mutation was introduced.
- Performance/operational impact: one bounded composite-key existence query per imported
  user-state row and set-based verification batches; the 101-track rehearsal recorded
  import wall time 2.72 seconds and peak import RSS 30,527,488 bytes.
- Documentation impact: Updated — this report, the R5 report, the release-gate report,
  `IMPLMENTATION_OUTSTANDING.md`, and the committed machine-readable rehearsal artifact.
- Remaining risks and owner: real approximately-5-TB source scale, operator topology,
  final differential semantics, and external R4 acceptance remain with the release owner.
- External input still required: approved production snapshot/backup/canonical mapping,
  accepted ADR-0149/0150/0151 dispositions, physical clients, and production integrations.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.1 reproducible operator-command help audit — 2026-08-20

- Exact files changed: `scripts/check_command_help.py`,
  `tests/python/test_command_help.py`, this report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: R4.1 command reproducibility was initially strengthened by an
  executable audit for 11 migration/search `melodee-cli` command paths plus
  the existing-snapshot rehearsal help path. The later complete documented
  command audit is recorded above. No requirement was marked
  terminal because the production snapshot baseline remains external.
- ADRs: no ADR was accepted or applied. ADR-0148 remains the accepted
  migration authority; ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 scripts/check_command_help.py` passed for 11 CLI paths plus
  `legacy_rehearsal.py drill --help`; `python3 -m unittest
  tests/python/test_command_help.py -v` passed 2/2; Python compilation passed.
- Compatibility impact: none; this invokes help paths only and changes no
  native v1, OpenSubsonic, or Jellyfin route, DTO, property, identifier,
  status, authentication, pagination, range, or intentional 410/501 behavior.
- Data/migration impact: none; the child environment contains only `PATH`, and
  no source database, target database, canonical media, or backup is opened.
- Security/data-loss impact: positive; local `.env` values, credentials, and
  deployment settings cannot turn the audit into an operational command.
- Performance/operational impact: 11 bounded CLI subprocesses and one Python
  help subprocess, each with a 30-second timeout; no persistent state.
- Documentation impact: Updated — this R4 report and the operator command
  audit script; the public command reference was unchanged because its
  documented commands were verified rather than modified.
- Remaining risks and owner: release owner/operator must still run the
  commands against the approved isolated production snapshot and retain the
  resulting baseline.
- External input still required: approved snapshot URL, backup location,
  canonical-root mapping, production topology, and ADR owner dispositions.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.2 second isolated existing-snapshot rehearsal — 2026-08-20

- Exact files changed: `design/evidence/r4/rehearsal-drill-existing-snapshot-r4dbg-second.json`,
  this report, and `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the no-seed rehearsal path was exercised a second time against an already
  populated isolated PostgreSQL source. It validated the source backup before target creation,
  imported 10 Artists, 10 Releases, 19 Tracks, and 1 user, rebuilt search from migrated
  PostgreSQL state, restored the target backup with matching counts, and completed the bounded
  freeze/rollback proof. R4.2 remains Partial because this is still a small isolated fixture,
  not the required approximately-5-TB production snapshot.
- ADRs: ADR-0148 remains the accepted migration authority; no ADR was accepted or applied.
  ADR-0149, ADR-0150, and ADR-0151 remain Proposed.
- Tests: `python3 scripts/legacy_rehearsal.py drill --help` was verified before the run. The
  machine-readable artifact records `result=pass`, zero verification mismatch gaps, zero
  duplicate public IDs, source counts equal to target counts, a validated 549,673-byte backup,
  restored counts equal to 10/10/19/1, 20 unchanged canonical-tree files totaling 4,093,382
  bytes with content hashes collected, and `source_mutated_by_harness=false`.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, identifiers,
  statuses, authentication, pagination, range behavior, and intentional 410/501 behavior are
  unchanged.
- Data/migration impact: the source connection was session-enforced read-only; the source
  database and canonical fixture were not seeded, written, or dropped by the existing-snapshot
  drill. Only the isolated rehearsal target and temporary restore target were used.
- Security/data-loss impact: positive bounded evidence for backup validation, canonical media
  byte immutability, target restore equality, and failed-target retention; no destructive
  operation touched a production database or media root.
- Performance/operational impact: import wall time was 0.60 seconds, peak import RSS was
  30,027,776 bytes, search rebuild wall time was 0.35 seconds, and backup/restore wall time was
  2.93 seconds on the local PostgreSQL container. These are fixture measurements only.
- Documentation impact: Updated — this R4 evidence report and `IMPLMENTATION_OUTSTANDING.md`;
  no public documentation changed because the operator-facing procedure did not change.
- Remaining risks and owner: the release owner/operator must provide the approved production
  snapshot URL, canonical-root mapping, backup location, production-sized capacity, and
  authorization for the real rehearsal.
- External input still required: approved approximately-5-TB snapshot and backup, real
  production topology, and operator authorization; no external endpoint or production data was
  contacted.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 disabled-podcast phase replay correction — 2026-08-20

- Exact files changed: `scripts/run_r4_dual_run.py`,
  `tests/python/test_scripts.py`,
  `design/evidence/r4/dual-run-report-opensubsonic.json`,
  `design/evidence/r4/README.md`, `design/evidence/r5/README.md`,
  `design/evidence/r5/release-gate-report.md`, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the R4 dual-run harness now includes the separate
  `opensubsonic-conditional-off.json` phase that the legacy oracle captures
  outside its main scenario list. The three disabled-podcast captures now
  replay and compare successfully; no requirement was marked terminal because
  333 other OpenSubsonic differences remain.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151
  remain Proposed; the correction changes only replay coverage.
- Tests: `python3 -m unittest tests/python/test_scripts.py -q` passed 33/33;
  `python3 -m py_compile scripts/run_r4_dual_run.py` passed;
  `python3 scripts/run_r4_dual_run.py --help` passed; the migrated
  OpenSubsonic replay exited 1 fail-closed with 318 classified and 333
  unclassified differences, and all three conditional-off scenarios had no
  unexplained differences.
- Compatibility impact: no native v1, OpenSubsonic, or Jellyfin route, DTO,
  property, status, auth, pagination, or range behavior changed. OpenSubsonic
  differential coverage increased by three required conditional captures.
- Data/migration impact: none; the replay continued to use disposable
  migrated databases and isolated roots.
- Security/data-loss impact: positive evidence correction; a required phase can
  no longer be silently omitted from the migrated differential.
- Performance/operational impact: one additional isolated disabled-podcast
  replay phase per OpenSubsonic run; no production runtime path changed.
- Documentation impact: Updated — this report, the R5 report, the release-gate
  report, and `IMPLMENTATION_OUTSTANDING.md`.
- Remaining risks and owner: release owner/compatibility owner must resolve
  the remaining 333 OpenSubsonic differences and provide external client and
  production evidence.
- External input still required: accepted ADR-0150 disposition and approved
  migrated snapshot/topology for final differential closure.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.5 launcher port isolation — 2026-08-19

- Exact files changed: `scripts/run_phase8_client_smoke.py`,
  `tests/python/test_phase8_client_smoke.py`, `IMPLMENTATION_OUTSTANDING.md`,
  and this report.
- Requirements: the current Feishin launch path now selects three distinct
  ephemeral loopback ports while holding the reservations together, preventing
  self-collisions between the Melodee server, client web server, and
  OpenSubsonic relay in one run. R4.5 remains Partial because current migrated
  physical-client acceptance is still external.
- ADRs: no ADR was accepted or applied; native v1, OpenSubsonic, and Jellyfin
  contracts remain separate, and ADR-0149/0150/0151 remain Proposed.
- Tests: `python3 -m unittest tests/python/test_phase8_client_smoke.py -v`
  passed 7/7; `python3 scripts/validate_r4_client_acceptance.py`, repository
  validation, requirement-index validation, `git diff --check`,
  `./scripts/check-smoke.sh`, and the final current `./scripts/check.sh` all
  passed. The full gate included 176 Python tests, Admin UI 44 files/248
  tests, workspace Rust/PostgreSQL tests, and cargo-deny.
- Compatibility impact: none; only local launcher port allocation changed.
- Data/migration impact: none; existing-target mode remains read-only and no
  database or canonical media is accessed by the allocator.
- Security/data-loss impact: no new external input or credential handling;
  distinct ports reduce cross-run interference without weakening validation.
- Performance/operational impact: one bounded socket-allocation pass per
  launch; a startup race with an unrelated process still fails closed through
  the existing readiness check.
- Documentation impact: Updated — this internal report and
  `IMPLMENTATION_OUTSTANDING.md`; public client behavior documentation did not
  change.
- Remaining risks and owner: current migrated physical-client runs and
  operator evidence remain release-owner owned.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.5 workflow-complete client evidence schema — 2026-08-20

- Exact files changed: `scripts/validate_r4_client_acceptance.py`,
  `tests/python/test_r4_client_acceptance.py`,
  `compatibility/clients/r4/acceptance-matrix.json`,
  `compatibility/clients/r4/README.md`, this report, the R5 report, and
  `IMPLMENTATION_OUTSTANDING.md`.
- Requirements: the R4.5 evidence artifact is now schema version 3 and carries the required
  workflow IDs for Android phone, Android Auto/DHU, MeloAmp, OpenSubsonic, and Jellyfin rows.
  Any reported pass/fail row must provide exactly one explicit pass/fail result for every named
  workflow; a passing row must have no failed workflow, and a failed row must identify at least
  one failed workflow with notes. The committed matrix remains honestly `pending-external`; no
  physical-client requirement became terminal.
- ADRs: no ADR was accepted or applied. ADR-0149, ADR-0150, and ADR-0151 remain Proposed; no
  wire contract or client behavior changed.
- Tests: `python3 -m unittest tests/python/test_r4_client_acceptance.py -v` passed 8/8;
  `python3 -m py_compile scripts/validate_r4_client_acceptance.py tests/python/test_r4_client_acceptance.py`
  passed; and `python3 scripts/validate_r4_client_acceptance.py` passed with 0 pass, 0 fail, and
  5 pending-external rows. The subsequent exact local `MELODEE_TEST_DATABASE_URL=postgres://... ./scripts/check.sh`
  exited 0 with 216 Python tests, Admin UI 44 files/248 unit tests, the complete workspace/
  PostgreSQL Rust and doctest matrix, formatting, workspace Clippy with `-D warnings`, and
  cargo-deny.
- Compatibility impact: none; native v1, OpenSubsonic, and Jellyfin routes, DTOs, properties,
  identifiers, statuses, auth, pagination, and range behavior remain unchanged and separate.
- Data/migration impact: none; this is evidence-schema validation only and does not access a
  database, migration source, canonical media, or client account.
- Security/data-loss impact: positive fail-closed evidence correction; incomplete workflow claims
  cannot be promoted to a passing client row, and the existing secret/path validation remains active.
- Performance/operational impact: bounded in-memory validation of five rows; no runtime path changed.
- Documentation impact: Updated — this report, `compatibility/clients/r4/README.md`, the R5 report,
  and `IMPLMENTATION_OUTSTANDING.md`; no public documentation changed because no client behavior
  was changed.
- Remaining risks and owner: release owner/operator still owns the current migrated physical-client
  runs and must provide sanitized evidence for every workflow.
- External input still required: disposable migrated target, current Android/Auto/DHU/MeloAmp/
  OpenSubsonic/Jellyfin client builds, platform access, and operator participation.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started; 239 requirements —
  211 tested, 9 implemented, 12 in-progress, 7 deferred.

### R4.4 differential comparator exactness — 2026-08-19

- Exact files changed: `scripts/run_phase8_differential.py`,
  `tests/python/test_scripts.py`, `IMPLMENTATION_OUTSTANDING.md`, and this report.
- Requirements: R4.4 local differential classification was hardened; the three former
  empty-scenario image/date rules were replaced with 38 exact scenario-and-complete-field
  rules. At the time of this 2026-08-19 package, the committed replays contained 192 native,
  336 OpenSubsonic, and 17 Jellyfin unclassified differences; the later 2026-08-20
  disabled-podcast replay correction supersedes the OpenSubsonic count with 333.
- ADRs: no ADR was accepted or applied. ADR-0150 remains Proposed; its metadata decision
  package remains the authority for the unresolved legacy release-date boundary. ADR-0149
  and ADR-0151 also remain Proposed.
- Tests: `python3 -m unittest tests/python/test_scripts.py -q` passed 32/32;
  `python3 -m unittest tests/python/test_phase8_client_smoke.py -q` passed 8/8;
  `python3 -m py_compile scripts/run_phase8_differential.py`,
  `python3 scripts/run_phase8_differential.py --help`, and `git diff --check` passed.
  The subsequent `./scripts/check-smoke.sh` and `./scripts/check.sh` both passed; the full
  gate ran 187 Python tests, Admin UI 44 files/248 tests, all workspace/PostgreSQL Rust tests,
  formatting, Clippy, and cargo-deny.
  Regression coverage proves every exact registry entry classifies, unrelated image/date
  fields remain unexplained, scenario-prefix collisions do not widen, and existing prefix
  rules on the same scenario remain active.
- Compatibility impact: none; no native v1, OpenSubsonic, or Jellyfin route, DTO, property,
  identifier, status, auth, pagination, or range contract changed.
- Data/migration impact: none; this changes only replay classification and does not alter
  migration mappings, PostgreSQL authority, canonical media, or search data.
- Security/data-loss impact: positive fail-closed evidence change; a new image/date field or
  a same-named field on another route is no longer silently classified by an empty-prefix rule.
- Performance/operational impact: negligible comparator registry lookup cost; no runtime
  server path changed.
- Documentation impact: Updated — this internal R4 report and
  `IMPLMENTATION_OUTSTANDING.md`; no public behavior or public documentation changed.
- Remaining risks and owner: release owner/compatibility owner must resolve the current 192/333/17
  residuals, including the ADR-0150 metadata boundary, accepted ordering/state/provider
  decisions, and production-traffic evidence. No residual was waived by this change.
- External input still required: accepted ADR-0150 disposition and an approved migrated
  snapshot/topology for final differential reruns.
- Updated counts: 45 packages total — 26 Complete, 13 Partial, 6 Not started;
  239 requirements — 211 tested, 9 implemented, 12 in-progress, 7 deferred.
