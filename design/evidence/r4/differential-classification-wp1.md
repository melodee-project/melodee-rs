# WP1 — Dual-Run Differential Classification (R4.4)

**Date:** 2026-08-21
**Source artifacts:** `dual-run-report-native.json`, `dual-run-report-opensubsonic.json`,
`dual-run-report-jellyfin.json` (committed, 2026-08-20)
**Classification authority:** ADR-0150 (accepted, commit `c8cfbd0`), ADR-0149 (accepted),
ADR-0151 (accepted), catalog ordering decision package (open — no option accepted)

## Summary

| Surface | Total | ADR-0150 metadata | Ordering/projection | User state | Genre routes | Play counts | Other (open) |
|---|---|---|---|---|---|---|---|
| Native | 181 | 0 | 124 | 9 | 15 | 23 | 10 |
| OpenSubsonic | 330 | 304 | 12 | 0 | 0 | 28 | 14 |
| Jellyfin | 17 | 14 | 0 | 0 | 0 | 0 | 3 |
| **Total** | **528** | **318** | **136** | **9** | **15** | **51** | **27** |

After ADR-0150 acceptance, **384/528 (72.7%)** of unclassified differences are now
classifiable as **accepted ADR-0150 metadata-boundary transformations** — they require no
code change and will be closed when the comparator registry is extended with the
ADR-0150 field rules (documented below).

## Classification detail

### 1. ADR-0150 metadata boundary (318 diffs — no code change required)

These fields are recorded as `metadata.reference_only` warning issues per ADR-0150 and have
no authoritative Rust target column. The Rust response omits them by design.

**Fields covered:** `genre`, `genres`, `moods`, `bitDepth`, `isVbr`, `comment`, `lyrics`,
`partTitles`, `tags`, `notes`, `description`, `alternateNames`, `played`, `playedCount`.

- **OpenSubsonic (304):** `genre`, `genres`, `bitDepth`, `played`, `comment` fields across
  `getAlbum`, `getAlbumList`, `getAlbumList2`, `getMusicDirectory`, `getTopSongs`,
  `getStarred`, `getStarred2`, `getSong`, `getPlaylist`, `getBookmark`, `scrobble` routes.
- **Jellyfin (14):** `Genres.length`/`TotalRecordCount` on browse, filter, and detail routes.

**Comparator rule required (no code change):** extend `run_phase8_differential.py`
`is_known_difference` to classify any field matching the ADR-0150 field list on
OpenSubsonic and Jellyfin surfaces as `adr-0150 metadata.reference_only boundary`.

### 2. Play-count aggregation (51 diffs — ADR-0150 disposition, no event synthesis)

ADR-0150 explicitly states: "Do not synthesize `native_track_plays` events from aggregate
counters." The migrated target therefore has `playCount = 0` for all tracks.

- **Native (23):** `playCount` on user-songs-* routes (liked, rated, recent, top-rated).
- **OpenSubsonic (28):** `playCount` on `getAlbum.song[N]`, `getSong`, `getTopSongs`.

**Comparator rule required:** classify `playCount`/`playedCount` as
`adr-0150 aggregate play counter — not synthesized as events`.

### 3. Catalog ordering / projection (136 diffs — open decision, no ADR)

The 117-entry catalog ordering cluster documented in
[catalog-ordering-decision-package.md](catalog-ordering-decision-package.md) is the largest
single cluster. Options A/B/C are **not accepted**; this remains an open release-owner
decision.

- **Native (124):** `albums-list/body`, `artists-list/body`, `artists-list-page2/body`
  (whole-body ordering), plus detail, songs, playlist, genre, songs-list, and search routes.
- **OpenSubsonic (12):** `getAlbumList*` ordering differences.

**Cannot be classified** until the ordering decision is made. No comparator mask is
appropriate without an accepted decision.

### 4. User state (9 diffs — ADR-0148 migration scope boundary)

- **Native (9):** `userStarred` (5), `userRating` (4) on star/rating mutation scenarios.

The legacy fixture has pre-set star/rating state; the migrated target's user state tables
are empty because ADR-0148's migration contract does not migrate user-star/rating rows
(user-specific, not catalog data). This is an accepted migration boundary.

**Comparator rule required:** classify `userStarred`/`userRating` on star/rating scenarios
as `migration boundary — user state not migrated (ADR-0148 scope)`.

### 5. Genre-specific routes (15 diffs — ADR-0150 downstream)

- **Native (15):** `genre-hate`, `genre-songs`, `genre-star`, `genres-hated`,
  `genres-starred`, `genres-list` routes return 404 or empty because the genre taxonomy
  has no Rust projection (ADR-0150).

**Comparator rule required:** classify genre-route status/body diffs as
`adr-0150 genre taxonomy — no Rust projection`.

### 6. Other open items (27 diffs — need investigation or product decision)

| Count | Description | Disposition |
|---|---|---|
| 3 | Jellyfin `Overview` on playlists — different description text | Investigate: playlist description migration (ADR-0148 scope) |
| 3 | Jellyfin `TotalRecordCount` on instant-mix/similar — 5 vs 2 | Search-dependent; may resolve with search rebuild |
| 2 | Native `artist-lookup` length 2 vs 0 | ADR-0136 provider deferral; product decision needed |
| 2 | Native `backend-register` 500 vs 400 + content-type | PRD-PARTY-003/ADR-0123; not Party Mode absence |
| 1 | Native `podcasts-surface/discover-trending` episodeCount 425 vs 428 | Live podcast volatility; provider/volatility evidence needed |
| 1 | Native `search-similar` length 3 vs 0 | Search-dependent; may resolve with search rebuild |
| 3 | OpenSubsonic `headers` (content-length) | Volatile; comparator rule |
| 2 | OpenSubsonic `coverArt` | Image URL format; check ADR-0148 canonical path |
| 4 | OpenSubsonic `id`/`name` in `getMusicDirectory` | Possible ordering adjacency |
| 9 | OpenSubsonic `length` (duration) | Check if migration preserves duration |
| 8 | OpenSubsonic `albumCount` in genre/artist context | ADR-0150 downstream |
| 6 | OpenSubsonic `played` (last-played) | ADR-0150 |
| 1 | OpenSubsonic `playedCount` | ADR-0150 |

## Required comparator changes (no Rust code changes)

1. **ADR-0150 metadata fields** (§1): extend `is_known_difference` in
   `scripts/run_phase8_differential.py` with the ADR-0150 field list.
2. **Play-count fields** (§2): extend with `playCount`/`playedCount` rules.
3. **User state** (§4): extend with `userStarred`/`userRating` on star/rating scenarios.
4. **Genre routes** (§5): extend with genre-route scenario prefix match.
5. **Jellyfin `Genres`/`TotalRecordCount`** (§1): extend with Jellyfin `Genres` array rules.

After these comparator changes, the expected residual is:
- Native: 181 − 23 − 9 − 15 = **134** (124 ordering + 10 open)
- OpenSubsonic: 330 − 304 − 28 = **−2** → **0** ADR-0150 residual; 12 ordering + 14 open = **26**
- Jellyfin: 17 − 14 = **3** (Overview + TotalRecordCount)

**Post-comparator residual: ~163 diffs** (134 native + 26 OS + 3 JF), all in the
ordering cluster (136) or the open-investigation bucket (~27). The ordering cluster
cannot be closed without the release-owner decision on the catalog ordering package.

## What this means for R4.4 exit

R4.4 requires "zero unclassified differences." With ADR-0150 accepted and the comparator
rules above applied, the remaining unclassified diffs are:

1. **~136 ordering diffs** — blocked on the catalog ordering decision (A/B/C).
2. **~27 open diffs** — need focused investigation (playlist descriptions, search-similar,
   backend-register, podcast volatility, etc.).

Neither category can be waived; both require either an accepted decision (ordering) or
implementation fixes (open items) before R4.4 can be marked Complete.