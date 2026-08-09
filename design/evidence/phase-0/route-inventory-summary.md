# Phase 0 Evidence — Route Inventory Summary

**Issue:** FIRST-10-ISSUES.md Issue 3 — Capture published API specifications and route manifests
**Requirements:** PRD-API-M1-001, PRD-API-OS-001, PRD-API-JF-001
**Baseline:** legacy commit `4028b58dbd3302ed431e2c5b32d9dde5634db218` (see
[legacy-baseline.md](legacy-baseline.md))
**Recorded:** 2026-08-07

## 1. Method

Two complementary captures, both tied to the frozen baseline commit:

1. **Published OpenAPI document.** The running legacy server (disposable environment, PostgreSQL
   schema migrated through `20260714155936_AddDeleteSourceResidueAfterIngestSetting`, version
   reported `2.2.0`) serves `GET /openapi/v1.json`. Captured to
   `compatibility/melodee-api-v1/contracts/openapi-v1.json` with the volatile `servers` URL
   normalized to `https://{melodee-host}/`.
2. **Static route manifests.** `scripts/generate_route_manifest.py` parses the attribute-routed
   ASP.NET controllers under `src/Melodee.Blazor/Controllers/` and emits stable, sorted JSON
   manifests (no timestamps) recording method, path, verbs, auth attributes, controller/action,
   intentional 410/501 statuses, and implementation classification. Regeneration is deterministic
   (`--check` verifies no drift) and the parser is covered by tests in
   `tests/python/test_scripts.py`.

## 2. Captured artifacts

| Artifact | Contents |
|---|---|
| `compatibility/melodee-api-v1/contracts/openapi-v1.json` | OpenAPI 3.1.1 document: 146 paths, 121 schemas |
| `compatibility/melodee-api-v1/contracts/route-manifest.json` | 202 routes across 32 controllers (native v1 + unversioned Themes) |
| `compatibility/opensubsonic/contracts/route-manifest.json` | 160 routes across 16 controllers |
| `compatibility/jellyfin/contracts/route-manifest.json` | 58 routes across 13 controllers |

## 3. Route counts and implementation status

| Surface | Routes | Implemented | Gone (410) | Not Implemented (501) | Conditional |
|---|---:|---:|---:|---:|---:|
| melodee-api-v1 | 202 | 202 | 0 | 0 | 0 |
| opensubsonic | 160 | 138 | 12 | 8 | 2 |
| jellyfin | 58 | 58 | 0 | 0 | 0 |

OpenSubsonic intentional negative behavior at the baseline (PRD-API-OS-005 compatibility data):

- **410 Gone:** chat (`getChatMessages`/`addChatMessage`), video/caption surfaces, and other
  deprecated routes; 2 additional routes return Gone conditionally (e.g., jukebox when disabled).
- **501 Not Implemented:** user-management routes (`getUsers`, `createUser`, `updateUser`,
  `deleteUser`, `changePassword` and their `.view` forms).

Auth conventions recorded in the manifests:

- **melodee-api-v1:** 188 of 202 routes carry class/method `[Authorize]` attributes
  (JWT bearer scheme; some with role constraints such as `Admin`). 14 routes have no ASP.NET
  authorization attribute (public or internally authenticated handlers — e.g., public shares,
  onboarding, stream).
- **opensubsonic:** no ASP.NET authorization attributes; authentication is query/form-parameter
  based (`u`, `t`/`s`, `p`, API key/JWT extensions) handled inside the controller base — exactly
  the behavior that must be preserved.
- **jellyfin:** no ASP.NET authorization attributes at the controller level; the adapter uses its
  own token parsing (`JellyfinTokenParser`) and internal authorization checks.

## 4. Discovery — the published OpenAPI document is incomplete

The served `/openapi/v1.json` documents 146 of the 176 unique `/api/v1/*` paths present in the
route manifest. Missing from the published document:

- Party surface: `party-sessions` (session lifecycle, participants, playback control, queue
  collaboration, moderation) and `party-endpoints` (endpoint registry).
- Native podcasts surface: `/api/v1/podcasts/**` (PodcastsController is explicitly
  `[ApiExplorerSettings(IgnoreApi = true)]`).
- Play queue: `/api/v1/queue`.

Consequences for compatibility work:

- The OpenAPI document **cannot** be the sole native-v1 contract source. Golden traffic captures
  (Issues 5–7) must cover the undocumented-but-live routes; the route manifest is the authoritative
  route inventory.
- Native clients (Android/MeloAmp) may depend on routes absent from the published spec.
- Whether the omission is intentional (party surface still settling) or a defect is a product
  question; it does not change the parity obligation — current observable behavior is the
  contract.

### Addendum (Issue 5, 2026-08-07): party and queue routes are not compiled at all

The party/queue omission from the OpenAPI document has a harder cause than documentation
settings: `Melodee.Blazor.csproj` at the frozen commit contains `<Compile Remove>` entries for
`QueueController` and all five Party controllers. Their routes exist in source but answer
**404** at runtime (captured: `catalog-queue/*` golden steps). The manifest generator now reads
the csproj exclusion list and marks the 35 affected routes `excluded-from-build`
(`implementation` field); podcasts remain compile-in but `IgnoreApi`. The unified contract
inventory (`compatibility/melodee-api-v1/contracts/route-inventory.json`, Issue 5) records this
per route alongside OpenAPI presence and client usage.

## 5. Discovery — OpenAPI document scope mixing

The published document is titled "Melodee API v1" and explicitly describes itself as the native
API, but it also contains:

- the unversioned native stream route `/song/stream/{apiKey}` (PRD-STREAM-001 relevant), and
- the OpenSubsonic podcast routes (`/rest/*Podcast*` with `.view` forms) — a subset of the
  OpenSubsonic surface.

No OpenSubsonic system/browsing/searching or Jellyfin routes appear in the published document;
those surfaces are inventoried only by the static manifests.

## 6. Reproduction

```bash
# Static manifests (no server required):
python3 scripts/generate_route_manifest.py --legacy-root /path/to/melodee
python3 scripts/generate_route_manifest.py --legacy-root /path/to/melodee --check  # drift check

# OpenAPI document (requires the baseline server, see legacy-baseline.md section 6):
curl -s http://127.0.0.1:5157/openapi/v1.json
# normalize servers[0].url to https://{melodee-host}/ before committing
```

## 7. Privacy review

- No credentials, tokens, user data, real hostnames, or private filesystem paths are present in
  the committed artifacts (enforced by `test_openapi_contract_is_sanitized`).
- The capture environment used a disposable database seeded by EF migrations only; no production
  data was accessed.

### Second addendum (Issue 6, 2026-08-08): scan routes were invisible to the parser

`OpenSubsonic/MediaLibraryScanning.cs` defines `startScan`/`getScanStatus` (plain and `.view`)
but its class lacks the `Controller` name suffix, so the Issue 3 parser skipped it. The
generator's class regex was relaxed; the OpenSubsonic manifest is now **164 routes** (was 160).
The route counts in section 2 are superseded accordingly (native 202, OpenSubsonic 164,
Jellyfin 58).

