# Phase 0 Report — Baseline Freeze and Compatibility Capture

**Status:** Final — Phase 0 closure (FIRST-10-ISSUES.md Issue 10)
**Date:** 2026-08-09
**Scope:** Issues 1-10 complete. This document is the canonical consolidation of all Phase 0
evidence and the gate for Phase 1.
**Baseline tag:** `phase-0-baseline` (annotated) marks the complete Phase 0 repository state,
including this report. Resolve the tagged commit with `git rev-parse phase-0-baseline^{}` and
the tag object with `git rev-parse phase-0-baseline`. The tag annotation enumerates the
baseline contents; the SHA is deliberately not embedded here because this file is part of the
tagged tree.

---

## Executive Summary

Phase 0 froze the legacy Melodee server (C#) as a behavioral oracle and captured its observable
contract across all three compatibility surfaces plus the media pipeline, at three scale tiers,
with fully reproducible tooling.

**What was learned:**

1. **The observable contract is now fully reconstructible from this repository** — 202 native
   v1 routes, 81 OpenSubsonic endpoints, and 60 Jellyfin routes are inventoried; every critical
   workflow has golden evidence (424 sanitized captures total) that reproduces byte-identically
   across runs after declared normalization.
2. **The published OpenAPI document is not the contract.** Live routes are missing from it;
   worse, 35 routes (queue + party) exist in source but are **excluded from compilation** and
   answer 404 at runtime. The unified route inventory is the authority.
3. **The legacy server has serious, measured defects** that the rewrite must not preserve: a
   security-critical unsigned-JWT acceptance on the OpenSubsonic surface, corrupted
   nonzero-offset seeks on OpenSubsonic streaming, a playlist-mutation corruption class across
   all surfaces, destructive same-size file deletion in ingestion, and a metadata-container
   gate that silently skips tagless media.
4. **Scale findings justify the architecture**: OpenSubsonic `getRandomSongs` takes ~4 minutes
   at 5M tracks (26-29 GB server RSS after random queries); native global search runs ~1s at
   5M tracks (~20x the PRD-SEARCH-006 target); OpenSubsonic browse endpoints are 10-25x slower
   than native equivalents at the same scale.
5. **Rewrite assumptions validated**: PostgreSQL set-based operations sustain 24.6-30.6k
   rows/s through 5M rows; the three-surface adapter separation is confirmed necessary (three
   surfaces, three different invalid-range behaviors, three different unknown-id behaviors);
   the fixture/oracle tooling proves Phase 1+ can develop against a reproducible baseline.

**Primary compatibility risks:** (R1) the unsigned-JWT acceptance must be deliberately broken
(security) while documented for clients that may depend on it; (R2) OpenSubsonic seek repair
changes observable behavior for clients that worked around it; (R3) playlist mutation fixes
change error behavior MeloAmp partially depends on (its add-to-playlist is already broken);
(R4) public-ID preservation across migration is mandatory and evidenced.

**Readiness for Phase 1:** all foundational decisions needed for Phase 1A are ratified
(ADR-0106/0107/0108/0110/0111/0112 accepted this phase; ADR-0109 Carbon remains Proposed for
Phase 9 validation). No unidentified foundational blocker remains.

---

## Legacy Oracle

| Field | Value |
|---|---|
| Repository | `melodee-project/melodee` (local checkout `/home/steven/src/github/melodee`) |
| **Authoritative commit SHA** | **`4028b58dbd3302ed431e2c5b32d9dde5634db218`** |
| Product version | 2.2.0 (`v2.1.4-19-g4028b58d`) |
| DB migration fingerprint | head `20260714155936_AddDeleteSourceResidueAfterIngestSetting` |
| Toolchain | .NET 10.0.10 SDK, EF tools 10.0.9, PostgreSQL 17.10 (Docker :5433), ffmpeg 8.1.2 |
| Recreation | `python3 scripts/legacy_oracle.py recreate` (fails closed on SHA mismatch) — full procedure: `design/evidence/phase-0/legacy-baseline.md` section 6 + section 9 amendments |
| Capture | `python3 scripts/legacy_oracle.py capture` (reseeds + restarts per scenario file; podcast-disabled phase toggles settings) |

The oracle was recreated from scratch multiple times during Phase 0 (including dropped
databases and cold starts) and never depended on session state.

## API Surface Summary

| Surface | Known | Classification | Golden captures | Evidence |
|---|---:|---|---:|---|
| Melodee API v1 | 202 routes | captured: 34; client-used-uncaptured: 69; documented-uncaptured: 45; undocumented-uncaptured: 19; excluded-from-build: 35 | 118 | `compatibility/melodee-api-v1/` |
| OpenSubsonic | 81 endpoints (164 routes) | supported: 56; conditional: 15; gone(410): 6; not-implemented(501): 4 | 179 | `compatibility/opensubsonic/` |
| Jellyfin | 60 routes | covered: 60/60 | 127 | `compatibility/jellyfin/` |
| Media pipeline | 31 corpus cases | extraction + pipeline + validation per case | baseline.json | `compatibility/media/` |

Undocumented-but-live routes and intentionally-Gone/501 routes are first-class rows in the
inventories (`compatibility/*/contracts/route-inventory.json`); `410`/`501` behavior is
automated golden evidence, not prose.

## Client Dependency Summary

- **melodee-player (Android, commit `7008eed1`):** 13 native v1 routes; Android Auto paths
  mapped (browse tree, voice search, favorites, scrobble); stream/image URLs consumed verbatim
  with no auth headers — self-authenticating `streamUrl` is a hard requirement.
- **MeloAmp (desktop, commit `096c76d8`):** 98 native v1 routes used; `?jwt=` appended to
  stream URLs; queue fully client-side; its add-to-playlist is broken against the baseline
  (object body vs bare `Guid[]` — D5).
- **OpenSubsonic workflows:** server-side contract captured for login (all auth forms),
  browse, search, lists, stream+seek, playlists, download, star/rating/scrobble, shares,
  radio, scan. Third-party client smoke is Phase 8 verification.
- **Jellyfin workflows:** discovery, auth (all 5 token transports), views, browsing, artists/
  genres, direct play + correct ranges, user state, sessions, playlists, images — 100% route
  coverage. Finamp/Feishin/Streamyfin smoke is Phase 8 verification.

## Media Baseline Summary

`design/evidence/phase-0/media-baseline.md` + `compatibility/media/baseline.json` (31
deterministic synthetic cases; generator committed, no copyrighted media):

- **Formats:** MP3/FLAC/OGG/OPUS/M4A extract and convert; **tagless WAV/AIFF and bare streams
  are silently skipped** (metadata-container gate, F1).
- **Conversion:** non-MP3 → MP3 384k VBR-q4 48kHz on ingest only; no streaming transcoding
  exists anywhere at baseline (advertised-but-absent on both API surfaces).
- **Artwork:** embedded/external precedence, 300/600/1600 square tiers, invalid artwork
  deleted, defaults at the image routes.
- **Validation:** strict sequential numbering, minimum song count/duration, year ranges;
  reasons categorized structural/metadata/enrichment/artwork/policy for PRD-INGEST-002.
- **Classification:** title/directory regexes (live → Other + manual validation; EP title
  trimming quirk recorded); **no keyword-based deletion exists**; non-studio directories block
  auto-storage (data-availability hazard, not destructive).
- **Destructive-policy hazards:** F2 same-size deletion without content comparison (with
  dangling melodee.json references); partial non-atomic mutations.

## Scale Summary

`design/evidence/phase-0/scale-baseline.md` + `compatibility/benchmarks/`:

| Measurement | Tier A (15k) | Tier B (560k) | Tier C (5M) |
|---|---:|---:|---:|
| OS getRandomSongs | 0.95 s | 24.6 s | **230 s** |
| Native search (broad/no-result) | ~1000 ms | ~1000 ms | ~1030 ms |
| OS getArtists | 261 ms | 607 ms | 1187 ms |
| OS search3 | 278 ms | 513 ms | 2098 ms |
| Native paged lists | 9-17 ms | 10-48 ms | 21-173 ms |
| JF items (audio) | 25 ms | 260 ms | 1523 ms |
| Server RSS after random queries | — | — | **26.2 GB (peak 29.1 GB)** |
| DB bulk insert | 30.6k rows/s | 28.6k rows/s | 24.6k rows/s |

Bottlenecks ranked in the evidence doc; proposed Rust budgets are labeled
requirement-vs-proposal (PRD-SEARCH-006 p95<50ms is already a requirement; browse/random/
memory budgets are proposals requiring ratification).

## Compatibility Risks (ranked)

1. **R1 — OpenSubsonic unsigned-JWT acceptance (security-critical).** Any username + forged
   JWT authenticates. **RESOLVED (2026-08-09):**
   [ADR-0113](../../adr/ADR-0113-do-not-preserve-insecure-opensubsonic-unsigned-jwt-authentication.md)
   (Accepted) — the Rust implementation MUST reject unsigned/invalid JWTs; recorded as an
   Approved Compatibility Deviation in the coverage matrix (D1).
2. **R2 — OpenSubsonic seek repair.** Correcting double range-processing changes 416/mangled
   behavior some clients may have adapted to. *Intentional fix; documented.*
3. **R3 — Playlist mutation repairs.** Fixing the corruption class changes error behavior;
   MeloAmp's object-body add-songs needs a product decision (accept the object form or keep
   the 400). **RECORDED (2026-08-09):**
   [ADR-0114](../../adr/ADR-0114-accept-meloamp-playlist-add-songs-object-body-alongside-canonical-array-form.md)
   (**Proposed**, Phase 7 scope): preserve the bare-array contract AND additionally accept
   MeloAmp's object body. Does not block Phase 1A.
4. **R4 — Public-ID preservation.** All golden evidence pins deterministic public IDs;
   migration must preserve them verbatim (PRD-API-M1-004).
5. **R5 — Advertised-but-broken extensions** (`apiKeyAuthentication`, `transcodeOffset`):
   keep advertising (client feature-detection compatibility) or stop (honesty)? *Decision
   needed in Phase 8.*
6. **R6 — Auth bypass contract** (localhost/baseUrl/images) is load-bearing for current
   deployments; the rewrite must reproduce it deliberately, with the security trade-off
   documented.

## Legacy Bugs / Oddities — Disposition

| # | Behavior | Disposition |
|---|---|---|
| D1 | Unsigned-JWT acceptance (OS) | **Do not preserve** (security; needs explicit product sign-off — R1) |
| D2 | Double range-processing seek corruption (OS) | **Do not preserve** (fix; R2) |
| D3 | Unknown-id behavior split (native 500 / OS ok+error or 302 / JF 404) | **Needs explicit decision** per surface (recommend: clean 404 everywhere; document deviation) |
| D4 | Playlist mutation corruption class | **Do not preserve** (fix; R3 for the MeloAmp body shape) |
| D5 | MeloAmp object-body add-songs → 400 | **Needs explicit decision** (accept object form or keep 400) |
| D6 | Queue/Party excluded from build (404) | **Preserve absence** for parity release; decide party/queue roadmap separately |
| D7 | Native JWT access tokens survive logout | **Preserve for compatibility** (stateless JWT semantics; document) |
| D8 | Advertised non-functional extensions | **Needs explicit decision** (R5) |
| D9 | F1 metadata-gate silent skips; F2 same-size deletion | **Do not preserve** (PRD-MEDIA-003) |
| D10 | No-tiebreak list sorts (nondeterministic order) | **Do not preserve** (add tiebreaks; ordering stability is PRD-API-M1-005) |
| — | Envelope quirks (status:ok+error, playQueue.current int, license trailExpires typo, XML type casing, search2 placeholder fields, suffix dot JSON/XML split) | **Preserve exactly** (golden-frozen wire behavior) |
| — | Timestamp format inconsistency (ISO vs culture-formatted, `---` nulls) | **Preserve exactly** per surface |
| — | Blank-artist defaults (`9999-01-01`) in nested payloads | **Preserve for compatibility** (harmless; clients tolerate) |

## Phase 1 Preconditions

| Precondition | Status |
|---|---|
| Legacy oracle precisely identified (SHA) | ✅ `4028b58d...`, enforced by tooling |
| Native API critical path captured | ✅ 118 golden, A/B reproducible |
| Native-client route usage inventoried | ✅ both clients, pinned commits |
| OpenSubsonic critical behavior captured | ✅ 179 golden, 81/81 endpoints classified |
| Jellyfin critical behavior captured | ✅ 127 golden, 60/60 routes |
| Media-processing baseline reproducible | ✅ generator + harness + 31 cases |
| Synthetic scale generators | ✅ 3 deterministic tiers |
| Baseline benchmarks | ✅ all surfaces, environment metadata attached |
| Route manifests reproducible | ✅ generator + `--check` |
| Capture normalization deterministic | ✅ A/B zero-diff across 22 surfaces |
| No secrets in committed captures | ✅ automated scans in test suite |
| Requirements traceability current | ✅ status-overrides + index check green |
| Parity matrix current | ✅ updated with Phase 0 evidence |
| ADRs resolved for Phase 1 | ✅ 0106/0107/0108/0110/0111/0112 accepted; 0109 deliberately Proposed (Phase 9) |
| Full repository validation | ✅ `./scripts/check.sh` exit 0 |
| No unidentified Phase 1 blocker | ✅ none identified |

**Phase 1 may begin.**

---

## Appendix A — Phase 0 evidence index

| Issue | Evidence |
|---|---|
| 1-2 (baseline/oracle) | `design/evidence/phase-0/legacy-baseline.md` |
| 3 (contracts) | `compatibility/*/contracts/`, `design/evidence/phase-0/route-inventory-summary.md` |
| 4 (harness) | `scripts/capture_baseline.py`, `scripts/compare_captures.py`, `compatibility/scenarios/` |
| 5 (native v1) | `design/evidence/phase-0/native-api-v1-capture.md`, `native-client-route-inventory.md`, `compatibility/melodee-api-v1/golden/` |
| 6 (OpenSubsonic) | `design/evidence/phase-0/opensubsonic-capture.md`, `compatibility/opensubsonic/golden/` |
| 7 (Jellyfin) | `design/evidence/phase-0/jellyfin-capture.md`, `compatibility/jellyfin/golden/` |
| 8 (media) | `design/evidence/phase-0/media-baseline.md`, `compatibility/media/` |
| 9 (scale) | `design/evidence/phase-0/scale-baseline.md`, `compatibility/benchmarks/` |
| 10 (closure) | this report, `compatibility/COVERAGE-MATRIX.md` + `coverage-matrix.json`, ADR ratifications |

## Appendix B — ADR disposition (Issue 10)

| ADR | Old status | New status | Basis |
|---|---|---|---|
| ADR-0100 ground-up Rust rewrite | Accepted | Accepted (unchanged) | Phase 0 confirms feasibility |
| ADR-0101 Release/Track language | Accepted | Accepted (unchanged) | Wire vocabulary preserved at boundaries; domain language clean |
| ADR-0102 separate API adapters | Accepted | Accepted (unchanged) | Three surfaces proved behaviorally distinct (D3 and others) |
| ADR-0103 FOSS dependencies | Accepted | Accepted (unchanged) | All Phase 0 tooling passes the gate |
| ADR-0104 modular monolith | Accepted | Accepted (unchanged) | No contradictory evidence |
| ADR-0105 PostgreSQL authoritative | Accepted | Accepted (unchanged) | 24.6-30.6k rows/s at 5M rows; set-based SQL proven |
| ADR-0106 Release-centric storage | Proposed | **Accepted** | PRD-STORAGE-002 explicit; media baseline hazards support |
| ADR-0107 Tantivy projection | Proposed | **Accepted** | PRD-SEARCH-001 explicit; legacy ~1s@5M vs 50ms target; tantivy 0.26.1 MIT active (verified 2026-08-09) |
| ADR-0108 FFmpeg/ffprobe/Chromaprint | Proposed | **Accepted** | PRD-MEDIA-003/PRD-FP-003 explicit; conversion path measured; chromaprint MIT (upstream LICENSE.md), ffmpeg 8.1.2 active |
| ADR-0109 Carbon admin UI | Proposed | **Proposed (unchanged)** | Validation (UI shells) is Phase 9 scope; tech revalidated active/Apache-2.0 (2026-08-08) |
| ADR-0110 SQLx + explicit SQL | Proposed | **Accepted** | SPEC.md selects SQLx 0.9.0; crates.io verified 0.9.0 MIT/Apache-2.0 (2026-05-21) |
| ADR-0111 no broker/Redis initially | Proposed | **Accepted** | PRD-ARCH-006/PRD-WORK-001 explicit; Phase 0 ran brokerless throughout |
| ADR-0112 Astro Starlight docs | Proposed | **Accepted** | Implemented in /docs with CI; astro 7.2.0 + starlight 0.41.7 MIT verified current (2026-08-09) |
| ADR-0113 Do not preserve insecure OpenSubsonic unsigned-JWT auth | — (new) | **Accepted** | Security deviation for discrepancy D1/R1; explicitly authorized by the Phase 0 closure work package |
| ADR-0114 Accept MeloAmp playlist add-songs object body | — (new) | **Proposed** | D5/R3 recorded for Phase 7; deliberately not Accepted (API phase is later) |

No unresolved Proposed ADR blocks Phase 1. ADR-0109 (Carbon) and ADR-0114 (playlist body
shapes) remain Proposed for their respective later phases.
