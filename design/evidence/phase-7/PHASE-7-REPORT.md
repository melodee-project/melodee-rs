# Phase 7 Report — Native Melodee API v1

**Status:** Final — independently reviewed and accepted

**Evidence captured:** 2026-08-12

**Base commit:** `5c75673d5c8524cd2212aaa4d78bfad5bf081fcc` plus the pending Phase 7 change set

**Authority:** the requirements listed in the
[Phase 7 work package](../../ROAD-TO-FEATURE-COMPLETION.md), SPEC native API, authentication,
compatibility, migration, and resource-bound invariants, ADR-0100 through ADR-0105, ADR-0107,
ADR-0108, ADR-0110, ADR-0111, ADR-0114, and ADR-0123 through ADR-0129.

## Final decision

The frozen native Melodee API v1 surface is implemented in Rust and every canonical inventory row
has an executable disposition. The generated matrix contains 202 routes: 150 are implemented and
differentially tested, 17 are compatibility normalizations explicitly authorized by accepted ADRs,
and 35 preserve the frozen build's intentional Party/Queue absence. No route is pending, deferred,
or claimed complete without route-specific legacy and Rust evidence.

The implementation includes native authentication and user migration, catalog browse/search and
user state, images, direct streaming, playlists and smart playlists, scrobbling, requests, shares,
podcasts, discovery/analytics/recommendations, playback settings/backends, and endpoint sessions.
It also includes bounded workers, rate limiting, library authorization, additive migrations,
operator configuration, public documentation, and differential capture tooling.

The repository gate, documentation gate, current-catalog performance run, and Android, Android
Auto-mode, and MeloAmp client exercises are green. Independent review initially returned BLOCK on
security, authorization, worker-liveness, boundedness, compatibility, and executable-evidence
findings. No finding was waived. The final frozen tree remediates those findings with adversarial,
concurrency, fault, scale, and differential tests; independent cross-slice re-review returned
ACCEPT with no residual P0/P1. Phase 7 is complete and Phase 8 is active.

## Route disposition

The canonical inventory is
`compatibility/melodee-api-v1/contracts/route-inventory.json`; route-specific evidence is
`compatibility/melodee-api-v1/contracts/phase7-route-evidence.json`; and the generated joined view
is `design/agents/PHASE-7-ROUTE-MATRIX.json`.

| Disposition | Routes |
| --- | ---: |
| Implemented and differentially tested | 150 |
| Approved compatibility normalization | 17 |
| Preserved intentional legacy absence | 35 |
| Implementation pending | 0 |
| Negative compatibility pending | 0 |
| Explicitly deferred | 0 |
| **Total** | **202** |

The frozen legacy baseline is commit `4028b58dbd3302ed431e2c5b32d9dde5634db218`.
Promoted captures now cover all 167 compiled routes and all 35 excluded routes. The matrix builder
rejects unknown or duplicate routes, missing promoted scenarios, missing Rust tests or requirement
IDs, normalizations without an accepted ADR, and false absence/deferment claims.

## Implemented architecture

- `melodee-auth` owns secret-safe values, legacy bcrypt verification, exact HS256 access-token
  validation, refresh/reset digests, and signed cookie primitives.
- `melodee-persistence` owns additive native-user and role/library mappings, refresh-family
  rotation and replay revocation, password reset, social identities, user state, preferences,
  playlists, requests, shares, podcasts, discovery facts, endpoint sessions, and atomic PostgreSQL
  rate limits.
- `apps/melodee-server/src/native/` is a compatibility adapter. Its wire DTO vocabulary preserves
  legacy `album`/`song` names without introducing them into core domain modules.
- Catalog handlers use stable public IDs, library-scoped SQL predicates, bounded server-side
  paging, and the rebuildable search projection where appropriate. Unknown and inaccessible IDs
  do not become membership oracles.
- Direct streaming validates the captured seven-day HMAC format, User/Track/client binding and
  expiry, handles full/HEAD/single-range requests, limits concurrent streams, and opens canonical
  assets with no-follow containment checks.
- Podcast discovery and download workers bound HTTP response size, XML work, concurrency, leases,
  retries, redirects, retention, and owned paths. User-supplied hosts are DNS-resolved and rejected
  when they target non-public address space; resolved addresses are pinned to resist rebinding.
- The native scheduler and long-running workers propagate cancellation, use ownership-fenced
  leases, and retain expiry as the recovery boundary when best-effort release cannot reach
  PostgreSQL.

## Compatibility decisions

Exact legacy behavior remains the default. ADR-0114 accepts both captured playlist add-item body
shapes. ADR-0128 enumerates the only additional normalized defects, including typed unknown-ID
responses, deterministic recents, transactional scrobble visibility, uniform stream authorization,
functional endpoint ownership, non-placeholder analytics/recommendations, valid chart artwork
links, and durable smart playlists instead of the frozen deployment's missing-table 500.

The 32 Party and three Queue routes were excluded from the frozen legacy build. Rust deliberately
does not mount them, and promoted negative captures plus one exact method/path regression cover all
35 absences. Phase 9 owns broader domain completion; no captured native-v1 route was deferred to it.

## Authentication and security

- Access tokens preserve the captured HS256 issuer `MelodeeApi`, audience `MelodeeClient`, 15-minute
  lifetime, and zero-skew validation. The public User API ID is resolved once to the internal
  principal; access requests do not consult refresh/revocation/session state.
- Refresh tokens are 64 random bytes and only SHA-256 digests persist. Rotation is row-locked;
  replay revokes the family; logout/revoke prevents refresh reuse while an already-issued access
  JWT retains the captured stateless behavior.
- Password-reset tokens are one-use, hash-only, bounded, and atomically update the password while
  revoking refresh families. Cookie authentication is domain-separated and Bearer credentials do
  not satisfy cookie-only routes.
- Google OIDC validates RS256, issuer/audience/time/email/domain, immutable provider-subject
  ownership, HTTPS-only production JWKS, denied redirects, bounded response size/key count, and
  singleflight key refresh. The dependency uses the audited `aws_lc_rs` backend, not the affected
  RSA backend.
- Trusted client addresses, strict native-only CORS, and the PostgreSQL rate limiter are explicitly
  configured and bounded. Error bodies and diagnostics do not expose credentials, token material,
  database URLs, or canonical filesystem paths.

## Data and migration impact

Migrations `0010` through `0019` are additive and cover authentication, user state/community,
podcasts, genre preferences, discovery, playlists, endpoint sessions, catalog browse indexes,
indexed Playlist import, and bounded recommendation projections.
They preserve migrated public UUIDs, use UUIDv7 for new internal identities, enforce foreign keys,
and have PostgreSQL integrity, authorization, replay, and lifecycle tests. No source or canonical
media is rewritten by a migration.

Binary downgrade retains the additive tables. A schema rollback drains native and podcast workers,
backs up post-migration state that must be retained, and restores a verified pre-0010 database into
a new empty PostgreSQL instance before forward migration. Search remains rebuildable and canonical
media remains untouched. Operator steps and new configuration are documented in
`design/runbooks/LOCAL-DEVELOPMENT.md` and the public upgrading/configuration guides.

## Performance evidence

The default gate runs a 2,000-Track bounded correctness/latency smoke. The release-mode ignored
current-catalog harness seeds 560,000 Tracks, 50,000 Releases, and 35,000 Artists, then performs one
warm-up and 20 measured requests per route. It validates required browse/search indexes, status,
response size, page size, stable identity, determinism, and exact selective search results. The
measured interval uses a preinstalled authenticated principal and a deterministic bounded test
signer; JWT verification, public-ID lookup, production HMAC signing, and media filesystem I/O are
explicitly outside this metadata-route harness rather than reported as synthetic zero counters.

```text
cargo test --release -p melodee-server --test native_phase7_performance \
  native_api_current_catalog_scale_evidence -- --ignored --exact --nocapture
```

The final remediated run seeded the dataset in 237,323 ms. Recorded warm p95 latency:

| Native operation | p95 |
| --- | ---: |
| Artists page | 11.413 ms |
| Releases (`albums`) page | 48.990 ms |
| Tracks (`songs`) page | 14.037 ms |
| Selective native Track search | 14.027 ms |
| Random Tracks, bounded to 25 | 11.964 ms |
| Track detail | 7.280 ms |

All browse/search/detail results met the 50 ms candidate budget; random met its 500 ms candidate
budget. These metadata routes have no media-open operation; the harness states that scope but does
not claim to instrument filesystem reads. Generated rows and media are disposable test data and are
not committed.

## Real-client acceptance

The detailed commands, environment, workflows, and limitations are in
[REAL-CLIENT-ACCEPTANCE.md](REAL-CLIENT-ACCEPTANCE.md).

- Android 1.8.0 on an Android 17/API 37 emulator passed login, browse, playlist, search, direct
  playback, seek, scrobble, background playback, and credential restoration against a disposable
  Rust server.
- The shipping exported MediaBrowser/MediaSession path passed Android Auto-mode root browse,
  playlist navigation, search/voice play, next/previous, ended-event auto-advance, and post-process
  authentication restoration. The emulator's installed Google Android Auto package is a stub, so
  this is not a physical head-unit or Desktop Head Unit certification run.
- MeloAmp passed its 29-suite/210-test client gate and production build. Its browser app and actual
  Electron shell passed auth, browse, search, direct HMAC playback, seek, and credential restoration
  against Rust.

An existing MeloAmp desktop-shell defect emits an unhandled `formatArtUrl is not defined` error in
its MPRIS album-art IPC. Renderer playback and all server interactions still passed; the client
repository was not modified, and this is recorded as a client follow-up rather than hidden as a
server success.

## Executed gates

The frozen implementation passed:

```text
./scripts/check.sh
python3 scripts/build_native_inventory.py --check
python3 scripts/build_phase7_route_matrix.py --check
python3 scripts/validate_repository.py
python3 scripts/extract_requirements.py --check
git diff --check
cd docs && pnpm install --frozen-lockfile
cd docs && pnpm check
cd docs && pnpm check:links
cd docs && pnpm build
```

`check.sh` passed repository/requirement validation, the full Python suite, formatting,
warnings-denied workspace Clippy, every workspace all-feature Rust test and doc-test, and
`cargo deny`. Cargo-deny reported only the documented benign duplicate-version and
`license-not-encountered` warnings. Repository validation reported 30 ADRs, 8 mockups, and 232
requirements. The docs gate reported no diagnostics, valid links, and built 36 pages.

Focused PostgreSQL/HTTP/fault tests cover auth rotation/replay/reset/rate limiting, library ACLs,
catalog and image DTOs, stream token/range/path/capacity behavior, playlist/smart lifecycle,
community/share/request rules, podcast worker theft/retry/delete/retention/SSRF behavior,
discovery/endpoint ownership, excluded routes, and the real-client fixture.

## Independent review decision

The first cross-slice review returned **BLOCK**. It found a predictable public-share bearer (P0),
raw secret-bearing URI logging, account/reset issuance races, catalog and discovery ACL leaks,
large-catalog random/similarity and recommendation defects, unbounded image/M3U paths, podcast
lease/temp/playback/SSRF/RSS defects, and route evidence that was structurally present but not
semantically route-complete. Review also identified lower-severity schema, paging, proxy, OIDC,
rollback-documentation, and reproducible-client-evidence gaps.

No finding was waived. Remediation now provides:

- 128-bit OS-CSPRNG public-share tokens, a usable alias, uniform rate-limited failures, target ACL
  revalidation, and replay-safe secure-preserve/weak-rotate import under ADR-0129;
- sanitized matched-route tracing, revision-locked account-state-fenced refresh issuance, real OIDC
  singleflight, stronger lifecycle/owner constraints, redacted Debug, rejected example secrets, and
  right-to-left bounded proxy resolution;
- fail-closed library ACL activation, ACL-scoped Artist counts, bounded full-domain random selection,
  captured similarity behavior, bounded paging/search inputs, trigram indexes, and measured native
  search;
- ACL-safe smart/discovery state, materialized recommendations, indexed set-based M3U import,
  pre-load/body-lifetime image admission, decoded upload validation, and bounded charts;
- just-in-time podcast claims, worker-specific temporary files, lease-fenced publication, durable
  heartbeat semantics, comprehensive public-address rejection, CDATA/OPML handling, and bounded
  orphan reconciliation; and
- evidence validators that resolve real annotated Rust tests and verify capture provenance, body
  hashes/bytes, methods, and normalized paths, plus a committed clean-checkout Android Auto probe.

The first re-review found further cross-library catalog/share leaks, lifecycle gaps, recommendation
projection drift and starvation, and a public-share check/load race. Those were also fixed rather
than waived: native reads and public Playlist/chart/image paths are caller-ACL and active-Release
scoped; recommendation updates use common deterministic Track locking and request evaluation uses
an indexed 4,096-candidate ceiling with an ADR-authorized typed 503 when completeness cannot be
proven; public-share authorization, complete DTO assembly, and visit accounting use one
repeatable-read transaction with ACL/resource locks. Deterministic concurrent play/reassignment,
ACL revocation, deprecation, candidate-exhaustion, and response-capacity tests prove the boundaries.

The final targeted re-review returned **ACCEPT** with no residual P0/P1. The remediated matrix is 202
complete with no pending/deferred row, strict workspace Clippy and the post-review full gate are
green, and the final 560k run is recorded above.

## Remaining operational risk

- Physical Android Auto head-unit/DHU behavior, production Google OIDC providers, real podcast
  feeds, and production-scale network/filesystem behavior still require operational observation.
- Current-catalog performance is local PostgreSQL evidence, not a guarantee for every deployment;
  operators must monitor latency, rate limiting, worker backlog, disk use, and stream capacity.
- Public Playlist-share assembly is bounded to 10,000 items but currently performs sequential
  per-Track DTO loads; observe latency for unusually large shared Playlists.
- The route-evidence validator rejects comments, literals, ignored/non-test symbols, and missing
  promoted provenance, but remains a lexical validator rather than a complete Rust `cfg` evaluator.
- Third-party client bugs remain owned by their repositories. The MeloAmp MPRIS issue above did not
  affect its renderer/server compatibility gate.
- OpenSubsonic and Jellyfin routes and DTOs remain separate Phase 8 work and are not mounted by the
  native adapter.

Documentation impact:
- Updated — native API/auth/client, users/access, playlists, podcasts, requests, sharing,
  party-mode absence, discovery/playback, installation configuration/upgrading, `.env.example`,
  local migration/restore runbook, technology baseline, feature/route matrices, requirement
  traceability, accepted ADRs, compatibility captures, evidence registry, and this report.
