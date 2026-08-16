# R1.3 — Security Baseline Inventory

Date: 2026-08-16. Scope: inventory of security-sensitive operations, secrets handling, HTTP
security boundaries, and CI security scanning, per `IMPLMENTATION_OUTSTANDING.md` §6.4. This is
an **inventory with assigned follow-ups**, not a closure claim — per the backlog, no PRD-SEC-*
requirement is declared tested from this document alone; R5 owns security closure.

## 1. Security-sensitive operations → controls

### 1.1 Path-consuming operations

| Operation | Location | Controls |
|---|---|---|
| Inbound source roots | `crates/melodee-ingestion/src/config.rs` | Roots resolved at config load; per-source existence/readability probed in onboarding preflight |
| Acquisition → staging handoff | `crates/melodee-ingestion/src/acquisition.rs` | Lease-fenced durable handoff; occurrence identity = dev+inode fence; BLAKE3 tree hash as content fence; R0 adversarial suite (traversal, adversarial names, path reuse A/B) in `crates/melodee-ingestion/tests/acquisition.rs` |
| Staging / work / canonical roots | `apps/melodee-server/src/onboarding.rs` | Four ADR-0145 roles probed and reported separately; work area passes when creatable under a writable ancestor; unit tests assert each verdict branch |
| Normalization media paths | `crates/melodee-media/src/normalize.rs` | Path containment and resource-bound tests; bounded preflight (`max_artwork_bytes`, 1600px cap) |
| Theme asset serving | `apps/melodee-server/src/native/discovery.rs` (`theme_asset`) | Request segment validated against theme-entry name rules; resolved only as a DB key — no filesystem path is ever constructed from the request segment |

### 1.2 Subprocess invocations

| Invocation | Location | Controls |
|---|---|---|
| ffprobe | `crates/melodee-media/src/lib.rs` (`run_ffprobe_cancellable`) | Timeout; stdout read through `read_limited` at 1 MiB; typed parse failure (no panic); cancellation-aware |
| ffmpeg | `crates/melodee-media/src/normalize.rs` | Timeout; bounded output; conversion profiles fixed by configuration, not by request input |
| fpcalc | `crates/melodee-media/src/normalize.rs` | Timeout; argv includes `--` separator before the path |
| Onboarding tool probes | `apps/melodee-server/src/onboarding.rs` | `-version` only, 5 s bounded probe |

Follow-up: ffprobe argv lacks the `--` separator fpcalc carries (R2-SEC-2 below); mitigated
today because all paths are absolute.

### 1.3 Upload endpoints

| Upload | Location | Controls |
|---|---|---|
| Operator artwork (artist/release) | `apps/melodee-server/src/admin_catalog.rs` | Admin gate; magic-byte sniffing + declared Content-Type equality; bounded size; ADR-0143 precedence keeps frozen image endpoints byte-identical without overrides |
| User avatar | `apps/melodee-server/src/native/avatars.rs` | Native JWT; magic-byte + pipeline-bounded; `avatarLastChanged` derived from store |
| Playlist image | `apps/melodee-server/src/native/playlists.rs` | Native JWT; magic-byte sniffing. **Gap:** declared Content-Type equality is not enforced here (R2-SEC-3) |
| Theme-pack import | `apps/melodee-server/src/native/discovery.rs` | Admin gate; name/extension/size rules per entry; DB-sourced entries re-validated on export |
| Config import | `apps/melodee-server/src/admin_config.rs`, `crates/melodee-persistence/src/settings.rs` | Admin gate; 1 MiB body cap; ≤1000 keys; unknown keys rejected without explicit flag; redaction marker fail-closed; preview-first |
| OPML import | `apps/melodee-server/src/native/podcasts.rs` | Manual 2 MiB check + axum DefaultBodyLimit fallback. **Gap:** no explicit route-level body limit (R2-SEC-4) |

### 1.4 Script engine

`apps/melodee-server/src/scripting.rs` — Boa capability-less sandbox (ADR-0135): no host
capabilities, wall-clock timeout, circuit breaker (`MAX_LEAKED_THREADS` 64), context payload
capped at 256 KiB (`MAX_CTX_BYTES`), return value capped at 16 KiB (`MAX_RETURN_BYTES`).
Accepted limitation (ADR-0135): no instruction fuel or heap limit; a single run can burn
CPU/memory until the wall-clock timeout — documented in-code and in the migration guide.

### 1.5 Provider responses

`apps/melodee-server/src/lastfm.rs` — single-attempt dispatch, 10 s timeout, fail-degraded
(errors 4/9/10 clear the link per legacy ADR-0004); HTTPS; response capped at 256 KiB.
**Gap:** the cap is post-read (`bytes()` buffers the full body first) — R2-SEC-1.
`external_provider_cache`: bounded jsonb payloads, hashed request keys, expiry.

### 1.6 Archive / sidecar / external-data parsers

- ffprobe JSON: 1 MiB `read_limited` bound (above).
- Google JWKS: 256 KiB body cap, ≤32 keys, per-key fields bounded (`native/auth.rs`).
- Config import payload: 1 MiB / 1000 keys (above).
- Boa context/return caps (above).
- Theme export re-validates every entry on the way out (poisoned-row resistance).

### 1.7 Filesystem removal operations

| Removal | Location | Guards |
|---|---|---|
| Inbound directory after acquisition | `acquisition.rs` `cleanup_inbound` | Refuses unless a **direct child** of the inbound root (`starts_with` + `parent()==root`); re-verifies dev+inode fence **and** full tree hash against the durable receipt before `remove_dir_all`; mismatched tree reported `Replaced`, never deleted; directory fsync; durable bookkeeping |
| Partial staging | `acquisition.rs` `remove_partial` | Refuses unless parent is the staging `releases/` dir; worker-owned only |
| Podcast episode/temp files | `podcast_worker.rs` `remove_owned_file` | `parent()==root` guard; `symlink_metadata` requires regular non-symlink; ownership asserted by DB rows under lease; `local_path` re-checked in the clearing transaction |
| Search generation retirement | `crates/melodee-search/src/generation.rs` `secure_remove_generation` | `O_DIRECTORY\|O_NOFOLLOW` root; two-phase rename-to-retiring; 64 KiB-bounded manifest identity re-verified; dir-fd-relative `unlinkat`/`openat` with `SYMLINK_NOFOLLOW`; **fails closed** on non-Linux/Android/Apple platforms |
| Onboarding probe | `onboarding.rs` | Removes only the uniquely named probe file it created |
| DB-only deletions | theme delete (`NOT is_built_in` guard), artwork override delete (row + atomic audit), avatar delete (row only) | Canonical library media is never deleted by any endpoint (Phase 5 decision contract) |

Note: `secure_fs` race resistance is Unix-only; the Windows fallback uses plain `std::fs`
without nofollow semantics (search retirement fails closed instead). Documented platform
boundary.

## 2. Secrets and credentials

### 2.1 Secret inventory

14 registry keys are classified `secret: true` in `KNOWN_SETTINGS`
(`crates/melodee-persistence/src/settings.rs`; pinned by test), matching the Phase 9
configuration inventory's "12 hard secrets + 2 credential-adjacent" with the two
credential-adjacent keys promoted to full secret. Compromised shipped defaults
(`encryption.privateKey`, `jellyfin.token.pepper` placeholder) were deliberately **not**
carried. Env-only secrets outside the registry: `MELODEE__DATABASE__URL` (missing ⇒ exit 2
without echoing), `MELODEE__NATIVE__AUTH__JWT_KEY` (32–4096 bytes; the documented placeholder
is rejected at startup), `MELODEE__JELLYFIN__TOKEN_PEPPER` (≥32 bytes), the bounded JSON
config-file layer (`MELODEE__CONFIG_FILE`, 1 MiB / 1000 keys, fail-closed).

Credential material at rest: bcrypt user passwords (cost 12 for new hashes; dummy-hash timing
equalization on unknown users); OpenSubsonic per-user API secrets cleartext in an
adapter-owned table — deliberate ADR-0130 decision 6 (token+salt auth requires
recoverability; isolated from password hashes, never logged); Jellyfin tokens as
HMAC-SHA256(pepper, salt+token) with 168 h expiry, max 10/user; native refresh tokens as
SHA-256 digests only, bounded session family with replay detection; password-reset tokens as
SHA-256 digests, 1 h expiry; Last.fm session keys AEAD-sealed (ChaCha20-Poly1305 under a
domain-separated key derived from `security.secretKey`) plus SHA-256 lookup digest —
ADR-0134 §7 amended; public share bearers 128-bit CSPRNG with collision retry (ADR-0129).

### 2.2 Redaction guarantees

- Config list/get/export redact every secret to the fixed marker `__MELODEE_SECRET_REDACTED__`
  with a header warning; the marker is **fail-closed rejected** as a submitted value, so a
  redacted export cannot overwrite a stored secret. Import previews name secret keys only;
  audit records counts/actor only. There is no show-secrets flag anywhere (CLI included).
- Onboarding/diagnostics evidence is explicitly "never secret material" (store contract and
  module docs); admin diagnostics expose no secrets, arbitrary filesystem contents, or raw
  SQLx rows.
- The HTTP `TraceLayer` span records only the matched route template — never raw URI or
  query — because stream/reset-validation URLs carry bearer material in path segments;
  covered by test.
- Secret-safe `Debug` impls (`SecretString`, `SecretPepper`, `SecretCipher`,
  `RefreshTokenDigest`, `NativeAuthService`, `EmailSettings.smtp_password`) with leak tests.
- Raw secret access is confined to `SettingsStore::read_raw_value`/`list_raw`, documented
  internal-only.
- `.env*`/`*.pem`/`*.key`/`*.pfx` are gitignored; `.env.example` ships placeholders the
  server rejects and a dev-only database password under a "never commit real credentials"
  header; `compose.yml` uses required-variable guards.

Observation (documented, not a claimed gap): no code path zeroizes memory after secret use.

## 3. HTTP security headers and authentication boundaries

Middleware stack (`apps/melodee-server/src/main.rs`): routes → admin SPA nest → Jellyfin
routing middleware → `TraceLayer` (template-only spans) → Jellyfin case-rewriting fallback.

| Surface | Auth boundary |
|---|---|
| Native API v1 | HS256 JWT bearer (15 min, fixed iss/aud, zero skew) via route layer; separate cookie-only group (`Secure; SameSite=Strict; HttpOnly`, HMAC domain-separated); DB-backed auth rate limiting with `Retry-After`; public surface enumerated in `native/mod.rs` |
| OpenSubsonic | Captured token (`md5(secret+salt)`) or password auth on every route via route layer; 2 MiB form bound; `jwt`/`apiKey` parameters never consulted (ADR-0113/0130); failures are the captured error-40 shape; captured loopback/baseUrl/`/images/` bypass preserved |
| Jellyfin | Per-handler `resolve_token` across five transports (ADR-0132 `api_key` alias); anonymous surface limited to system-info/ping |
| Admin API | `AdminPrincipal` extractor on **every** handler: native JWT bearer or session cookie (ADR-0139), enabled/unlocked/not-blacklisted + `is_admin`; stable 401/403 envelope; onboarding status admin-gated because it reveals paths/features |
| Admin SPA static | Anonymous by design (ADR-0138); all data via the gated Admin API |
| Media streams | Native signed-URL token: HMAC-SHA256, 7-day expiry, bound to user+track+client IP, library ACL re-checked at serve time, indistinguishable 401 (ADR-0125); OS/JF behind their adapters' auth |
| Public shares | Anonymous 128-bit CSPRNG bearer in path; 120/min per-client-IP rate limit; identical 404 for unknown/expired/revoked/malformed (ADR-0129) |
| Google OIDC | Client-supplied `id_token` validated against Google JWKS: RS256-only, pinned issuers, audience + hosted-domain allowlist, zero leeway, 10 s / 256 KiB bounded JWKS fetch with singleflight caching; no client secret exists in this design |
| Health/readiness | Unauthenticated by design (infrastructure probes); readiness is a bounded 2 s `SELECT 1` |

**GAP (release-blocking inventory finding):** no surface sets CSP, X-Content-Type-Options,
X-Frame-Options/frame-ancestors, HSTS, or Referrer-Policy; the only header layers are
`TraceLayer` and a no-wildcard exact-origin CORS layer on the native surface; the admin SPA
has no CSP `<meta>`. Per-endpoint captured cache headers exist where the frozen contract
defines them. Tracked as R5-SEC-1 below (frozen API surfaces need compatibility review before
any header addition; the admin SPA CSP is safe to add).

## 4. CI security scanning

| Control | Where | Gates |
|---|---|---|
| `cargo deny check` (RustSec advisories, yanked=deny, license allowlist, wildcard bans, crates.io-only sources) | `deny.toml`; `scripts/check.sh`; `full-validation.yml` (nightly + manual); `release-container.yml` quality gate | Nightly, release; **not PRs** (PR smoke intentionally excludes dependency auditing) |
| Trivy image scan (CRITICAL always blocks; HIGH blocks when fixable; no ignore file — exceptions require a policy change) | `release-container.yml` build-and-test | Release only; no PR/nightly container scan |
| Multi-arch runtime hardening assertions (uid 10001 non-root, health contracts, clean SIGTERM, OCI labels) | `release-container.yml` | Release |
| SPDX SBOM attestation (BuildKit `sbom: true`) + keyless GitHub build-provenance attestation (`actions/attest@v4`) | `release-container.yml` publish; package-write credentials only ever on `release: published` | Release supply-chain evidence |
| `pnpm audit --prod` | `admin-ui.yml` | PRs/pushes touching `admin-ui/**` |
| Clippy `-D warnings` incl. panic/unwrap bans in production code | workspace lints via `check.sh`/`check-smoke.sh` | Every gate |

Gaps: no secret scanning (gitleaks/trufflehog or GitHub-native), no SAST (CodeQL), no
Dependabot/Renovate dependency-update automation, container scanning is release-triggered
only, `docs/` pnpm dependencies are never audited, and image provenance attestation exists
but the image is not cosign-signed. Test-only synthetic JWT key/pepper values visible in the
release workflow are synthetic by construction.

## 5. Assigned follow-up work items

| ID | Item | Owner phase |
|---|---|---|
| R2-SEC-1 | Last.fm 256 KiB response cap is post-read; switch to a streaming bound like `podcast_worker.rs::fetch_bounded` | R2 |
| R2-SEC-2 | Add `--` separator to ffprobe argv before the path (parity with fpcalc) | R2 |
| R2-SEC-3 | Playlist image upload: enforce declared Content-Type equality with sniffed type (parity with artwork/avatar uploads) | R2 |
| R2-SEC-4 | OPML import: set an explicit route-level body limit instead of relying on the manual check + DefaultBodyLimit fallback | R2 |
| R2-SEC-5 | Canonical artwork serve path: verify recorded `byte_hash` (currently size-only; equal-length swap would go undetected) | R2 |
| R5-SEC-1 | HTTP security headers: define and ship a header policy (CSP for the admin SPA at minimum; API surfaces need a compatibility check against frozen captures before any addition) | R5 |
| R5-SEC-2 | Add repository secret scanning (gitleaks CI job or documented GitHub-native enablement) | R5 |
| R5-SEC-3 | Add SAST (CodeQL or equivalent) and dependency-update automation (Dependabot/Renovate) or record an accepted decision not to | R5 |
| R5-SEC-4 | Extend `pnpm audit` coverage to `docs/`; consider nightly Trivy or record the release-only scan as an accepted boundary | R5 |

Documented boundaries kept (no new work): Boa fuel/heap limit (ADR-0135 accepted trade-off),
non-Unix `secure_fs` fallback (fails closed where racy), no memory zeroization (observation),
lexical route-evidence validator, Chromium-only visual baselines.
