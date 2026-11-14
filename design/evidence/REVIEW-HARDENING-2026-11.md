# November 2026 review hardening program

## Scope and authority

The user authorized implementation of the combined project-review checklist on 2026-11-13,
including parallel implementation. Baseline: `9364b09` (Artist status rename already committed).
This program fixes runtime defects and strengthens local qualification; it does not authorize
production rehearsal, private-data access, provider credentials, cutover, rollback, or publication.
Accepted ADRs, SPEC.md, PRD.md, and AGENTS.md govern every workstream.

## Work ledger

| Task | Outcome | Owner |
|---|---|---|
| T01 proxy-aware OpenSubsonic authentication | Implemented: secure default, explicit legacy opt-in, shared proxy setting and old alias; ADR-0191 | Security |
| T02 isolated test databases and cleanup | Implemented: parsed connection options, database assertion, full UUID names, panic cleanup | Automation |
| T03 terminable bounded scripting | Implemented: child process, VM and address-space bounds, deadline kill/reap; ADR-0193 | Runtime |
| T04 bounded theme-pack migration | Implemented: shared compressed/expanded ZIP bounds and positive import coverage | Runtime |
| T05 Artist reuse eligibility | Implemented: distinct Artist grouping, full ambiguity set and external-ID eligibility | Catalog |
| T06 import evidence and projection consistency | Implemented: resolved overlays, reused Artist outbox revisions, correct decisions; ADR-0194 | Catalog |
| T07 Admin timestamps | Implemented: typed RFC 3339 timestamps, real-router and live-browser checks | Integration |
| T08 complete Artist status vocabulary migration | Implemented: audit/spec/UI vocabulary and additive projection/overlay repair migration 0045 | Catalog / Integration |
| T09 bounded password execution | Implemented: finite admission and blocking workers, cancellation-safe permits | Security |
| T10 provider streaming byte limits | Implemented: limits enforced while receiving bodies, including chunked responses | Runtime |
| T11 immutable revision-pinned release publication | Implemented: one validated SHA, digest checks, immutable exact tags, serialized publication and retry rules | Automation |
| T12 PR database-free and PostgreSQL coverage | Implemented: pure integration targets in fast/unit gates; full validation on PRs | Automation |
| T13 persistent installation identity | Implemented: migration 0046 and per-installation Jellyfin identity; ADR-0192 | Security |
| T14 cumulative migration issues | Implemented: retain every invalid playback-field note on the same row | Runtime |
| T15 accurate evidence and public status | Corrected: failed historical theme import no longer establishes preservation acceptance | Integration |
| T16 measured migration batching | Implemented: bounded catalog/Track writes and set-based play-history import; synthetic measurement below | Runtime |
| T17 faster isolated integration setup | Implemented: disposable migrated template; forward-migration tests retain empty databases | Automation |
| T18 shared library and worker lifecycle boundaries | Implemented: real shared services in melodee-application; CLI no longer depends on server; ADR-0195 records remaining boundary choices | Runtime |
| T19 visible translation completeness | Implemented: generated coverage validation and preview labels for incomplete locales | Integration |
| T20 pinned pnpm setup | Implemented: exact project version via global match, Corepack or npx | Automation |
| T21 container build caching | Implemented: selective source copies and BuildKit caches; Docker build check passed; image timing remains unmeasured | Automation |
| T22 tested Python CI orchestration | Implemented: Python helpers behind thin workflow and Make entry points | Automation |
| T23 portable paths and repository hygiene | Addressed with explicit retained-file dispositions below | Integration |
| T24 exact-candidate qualification | Local prechecks and final isolated live audit passed; external release gates remain operator-controlled | Integration |

## Validation and closeout

Results below record executed checks. Existing acceptance artifacts must not be relabelled as
evidence for this changed candidate. All 15 non-terminal requirements retain their release-owner
gates until supported by candidate-specific evidence.

Initial focused validation passed: 328 Python governance tests; Admin UI static checks, dependency
audit, 301 unit tests and production build; 316 browser tests (62 intentional viewport-specific
skips); documentation dependency audit, type/link checks, and all 46 static pages. Final workspace
gates and the isolated live audit are recorded at closeout below.

Test-database setup measurement: fresh create plus migrations 1.413 seconds, template clone
0.175 seconds on the same local PostgreSQL instance (about 8.1 times faster for setup).
The runner removed its own disposable databases. This is a local setup measurement, not a
claim about whole-suite speed or production migration performance.

Migration batching validation imported 1,002 synthetic Tracks and 10,001 plays in 12.08 seconds
with a 257-row source batch; a repeat preserved all internal/public IDs and play counts. A separate
manual comparison on 202 Tracks and 2,001 plays measured 14.843 seconds with one-row source pages
and 2.806 seconds with 257-row pages (about 5.3 times faster). Both runs use the corrected importer:
the one-row control is not a benchmark of the old revision, and neither substitutes for the
560,000-Track/multi-million-history operator acceptance workload. The ignored manual benchmark
can be invoked explicitly from the CLI legacy integration suite.

The real-API live audit additionally exposed duplicate Artist credit rows when one Track had
multiple billing/contribution associations. The query now counts and pages distinct Tracks,
retaining performance evidence and deterministic representative billing. A regression failed
before the fix, and all seven focused catalog-browse tests pass after it.
The next live run exposed a same-screen navigation defect: the Artist statistic link changed
`?tab=tracks` while local component state left Overview selected. Tab selection now follows
the URL. The existing unit test was strengthened to click the statistic itself, failed before
the correction, and all ten Artist-detail tests pass after it. The locale preview suffix is
also included in the language switcher's accessible name.

Final boundary review moved the inherited onboarding and podcast SQL into persistence helpers.
The fifteen podcast SQL literals remain byte-identical; application services retain transaction
boundaries and filesystem/network orchestration. A database-free boundary regression enforces
the SPEC section 7 separation and absence of the CLI-to-server dependency.

The [final isolated live evidence](review-hardening/isolated-live-audit.json) records freshly
built server, CLI, script-worker and UI hashes after the final boundary corrections: ten real-API
browser tests passed, with sixteen runtime invariants passing before and after the UI edits.
Three generated FLAC Releases reached `indexed` and resolved to two Artists/three Tracks.
The temporary server, owned database and browser authentication state were cleaned up.

The first full integration pass caught a fixture-lifetime regression: reconciliation and manual
performance scenarios moved the pool/name out of `TestDb`, dropping its new cleanup guard before
later queries. Those scenarios now retain the complete owner. `TestDb` implements `Drop` so future
partial moves fail to compile, and a regression checks returned-fixture reconnect/cleanup behavior.
The corrected full gate subsequently passed, including all fifteen reconciliation tests.

Do not overlap full PostgreSQL test sweeps on the shared development server. An overlapping
diagnostic run produced connection-pool setup timeouts; the affected legacy migration and
OpenSubsonic media targets then passed sequentially (15 and 32 tests respectively). Final
prechecks run without another database test sweep. The final Python run after the development
launcher fix passed all 333 tests.

## Remaining release qualification

These obligations remain explicit; this review does not change their requirement statuses:

| Gate | Required evidence |
|---|---|
| MEL-9 / MEL-10 | Authorized configured Last.fm and metadata-provider acceptance with controlled credentials |
| MEL-11 | Real-feed podcast download, retry, retention, and downloaded-media cutover acceptance |
| MEL-12 | Exact release candidate, both OCI architectures, runtime smoke/link checks, scans, SBOM, attestations, approved publication |
| MEL-16 | Target-scale performance/fault budgets; local synthetic measurements above are preparatory |
| MEL-17 / MEL-19 | Five-client acceptance and configured OIDC/SMTP/MPD/MPV evidence |
| MEL-18 and preservation follow-up | Reconciled candidate evidence, including a corrected authorized theme migration rehearsal |

Historical cutover/rollback and differential reports are retained for their recorded candidates.
New production-shaped migration, cutover/rollback, private-data access, and credential use require
the documented operator authorization. Actual container cache timing and fresh architecture
image execution are not claimed by `docker buildx build --check .`.

## Compatibility, migration, and documentation impact

ADRs 0191–0195 record the authentication default, persistent installation identity, script
resource boundary, catalog repair, and shared application boundary. OpenSubsonic clients that
relied on anonymous local/Host access must configure credentials. Existing Jellyfin clients may
need to reconnect once. Source installations must deploy the matching script-worker binary;
hard process memory enforcement is qualified on Linux, while unsupported platforms fail-degrade
script evaluation. Protocol DTOs remain separate, and catalog public IDs are preserved.

Migrations 0045 and 0046 are additive. The earlier 0044 migration is unchanged. The upgrade guide
documents search catch-up, restart-safe overlay repair, maintenance/WAL headroom, installation
identity behavior, and restoring the prior database with its matching binary for rollback.

Documentation impact: Updated the following public pages in the same change:

- `docs/src/content/docs/admin/appearance-and-localization.md`
- `docs/src/content/docs/admin/provider-enrichment.md`
- `docs/src/content/docs/admin/scripting-hooks.md`
- `docs/src/content/docs/api/admin-api.md`
- `docs/src/content/docs/api/jellyfin.md`
- `docs/src/content/docs/api/opensubsonic.md`
- `docs/src/content/docs/clients/native-melodee-clients.md`
- `docs/src/content/docs/installation/build-from-source.md`
- `docs/src/content/docs/installation/configuration.md`
- `docs/src/content/docs/installation/docker.md`
- `docs/src/content/docs/installation/legacy-migration.md`
- `docs/src/content/docs/installation/upgrading.md`
- `docs/src/content/docs/library-management/artists.md`

Internal SPEC, ADRs, requirement references, and historical evidence corrections are updated.
All 239 requirement dispositions retain their previous status; 15 remain non-terminal.

## Final local closeout

All five selected prechecks passed in 945.1 seconds with no concurrent PostgreSQL test sweep:

```text
npx --yes --package=node@26.11.1 -- python3 scripts/do-prechecks.py --include-e2e
```

| Check | Final result |
|---|---|
| Working/staged whitespace checks | Passed |
| Repository, requirements, and compatibility-evidence validation | Passed; 239 requirements, 15 non-terminal |
| Python governance/automation tests | 333 passed |
| Rust formatting and workspace Clippy, all targets/features, warnings denied | Passed |
| Rust workspace library/binary/integration/doc tests | 1,317 passed; 5 explicitly ignored harnesses |
| Cargo dependency policy | Advisories, bans, licenses, and sources passed |
| Admin UI dependency audit, typecheck, lint, localization check, unit tests, build | Passed; 301 unit tests |
| Documentation dependency audit, typecheck, relative/build links, build | Passed; 46 pages |
| Admin UI browser/visual/accessibility suite | 316 passed; 62 intentional viewport-specific skips |
| Final isolated real-API browser audit | 10 passed; no mocks or skips |
| Isolated runtime invariants before/after browser edits | 16 checks each, zero violations |

The five ignored Rust targets are the manual migration batch measurement, settling performance
snapshot, media-inspection evidence harness, interactive native client fixture, and current-catalog
native API scale evidence. The migration batch measurement was run explicitly and is reported
above. The other harnesses are not claimed as executed by this gate. The browser skip count comes
from cases intentionally restricted to specific viewport projects, not failed tests.

The program addresses T01–T23 through implementation or an explicit reviewed disposition.
T24 is locally validated and remains open for the release/operator gates listed above. No
production migration, credentialed provider qualification, registry publication, or cutover is
represented by these results. All edits remain reviewable in the working tree.

## Repository hygiene dispositions

- The historical MEL-13 report moved to `feature-parity/MEL-13-REHEARSAL-REPORT.md`; its
  failed theme import remains visible, and stale completion claims have an explicit correction.
- Executable Feishin browser discovery uses Playwright's installed executable path, retaining
  the existing `CHROMIUM_EXECUTABLE` override. Historical checkout descriptions are portable.
- `.kilo-tmp/` was already ignored at the baseline. No temporary/private artifacts were read or
  deleted. `.kilo/kilo.jsonc` is intentional shared configuration (`snapshot: false`), not a
  credential or generated artifact; it is retained. A blanket `.kilo*` ignore would hide it.
- `migrations/README.md` is an intentional discovery pointer to the authoritative SQLx migration
  directory, not a second migration tree; it is retained to avoid breaking that convention.

## Dependency decision

The mandatory documentation dependency audit found existing Astro advisories
[GHSA-mh32-jxcr-mh9f](https://github.com/advisories/GHSA-mh32-jxcr-mh9f) and
[GHSA-r3h5-4585-7x3w](https://github.com/withastro/astro/security/advisories/GHSA-r3h5-4585-7x3w).
Update the existing exact Astro pin from 7.3.7 to 7.4.7 and regenerate the docs lockfile;
the MIT license, static-site deployment, and Starlight integration remain the same.
This is a maintenance update within the existing documentation dependency decision, with
no new production dependency. The audit, type/link checks, and build must pass for this pin.

The docs checker also required a package extension local to `@astrojs/astro2tsx@0.1.2`:
its pinned emnapi core 1.11.3 was paired with an auto-selected runtime 2.0.0 beta, which
failed before diagnostics. Pin the missing runtime dependency to the matching 1.11.3
without changing Rolldown's independent optional WASM pair. The docs engine minimum is
now 22.23.4, matching Astro; validation uses the already documented CI Node 26.11.1.
