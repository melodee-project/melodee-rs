# Phase 0 Work Package — Baseline Freeze and Compatibility Capture

## Mission

Make the current C# Melodee server an executable product oracle before broad Rust feature work begins.

## Required reading

- `AGENTS.md`
- `PRD.md` sections 6, 7, 21–24, 49–57
- `SPEC.md` sections 19, 24, and 26
- `IMPLEMENTATION.md` Phase 0
- ADR-0100 through ADR-0105

## Scope

This work package includes contract capture, fixture normalizers, baseline datasets/benchmarks,
real-client smoke procedures, legacy configuration/media-profile inventory, and foundational ADR
review.

It does not include implementing catalog parity, durable ingestion, search, podcasts, party mode, or
the admin SPA.

## Inputs

- A checkout of the existing C# repository at a recorded commit/tag.
- A disposable legacy database with representative data.
- A running legacy server.
- Sanitized test accounts and media fixtures.
- Existing Android, MeloAmp, OpenSubsonic, and supported Jellyfin clients.

Never point exploratory capture/destructive tooling at the only copy of the production library.

## Outputs

```text
compatibility/
  melodee-api-v1/contracts/
  melodee-api-v1/golden/
  opensubsonic/contracts/
  opensubsonic/golden/
  jellyfin/contracts/
  jellyfin/golden/

design/evidence/phase-0/
  legacy-baseline.md
  route-inventory.md
  auth-lifecycle.md
  streaming-baseline.md
  media-normalization-profile.md
  configuration-inventory.md
  performance-baseline.md
  real-client-smoke-results.md
```

## Work order

Complete the issues in `FIRST-10-ISSUES.md` in order unless an issue explicitly documents a safe
parallel dependency.

## Fixture design

Every captured scenario needs:

- stable scenario ID;
- surface (`native-v1`, `opensubsonic`, `jellyfin`);
- legacy commit/version;
- method, route, query/body shape;
- authentication mode;
- expected status and selected headers;
- response body fixture;
- normalizer for volatile values;
- privacy review; and
- a test asserting the expected comparison.

## Critical native v1 workflows

At minimum capture:

- login, refresh, logout, replay/invalid refresh behavior;
- current user;
- artist list/detail/Releases/Tracks;
- Release detail/Tracks;
- Track detail, image URL, stream URL;
- search;
- playlist list/detail/create/update/delete/reorder;
- star/rate/hate/pin behaviors currently exposed;
- queue persistence;
- scrobble now-playing/played;
- requests and unread activity;
- podcasts/resume when enabled; and
- public shares.

## Critical OpenSubsonic workflows

At minimum capture:

- ping/license/extensions/music folders;
- getIndexes/getArtists/getArtist/getAlbum/getSong;
- search3/random/genre/list modes;
- stream/range/seek/download/cover art;
- star/rating/scrobble;
- playlists/bookmarks/play queue/shares/radio;
- scans;
- optional podcast/jukebox behavior; and
- intentional 410/501 responses.

## Critical Jellyfin workflows

At minimum capture:

- public discovery/ping;
- username/password login and token forms;
- user views/items/detail;
- Artists/AlbumArtists/genres;
- direct and universal audio streaming/ranges;
- favorites/played state;
- sessions progress/stopped/logout;
- playlists and images; and
- absent non-music behavior probed by supported clients.

## Privacy and secret review

Before promoting a capture to `golden/`:

- replace real usernames/emails with fixture identities;
- replace hostnames/IPs with documented placeholders where not semantically important;
- remove passwords, bearer tokens, refresh tokens, OpenSubsonic hashes/salts, Jellyfin tokens, API
  keys, cookies, and private filesystem paths;
- verify artwork/audio fixture licensing; and
- rerun the fixture test after sanitization.

## Exit checklist

- [ ] Legacy baseline commit/tag recorded.
- [ ] Native v1 contract inventory complete for critical paths.
- [ ] OpenSubsonic matrix converted to executable scenarios.
- [ ] Jellyfin current subset converted to executable scenarios.
- [ ] Authentication and streaming lifecycle fixtures captured.
- [ ] Current FFmpeg normalization behavior captured.
- [ ] Synthetic dataset generators committed.
- [ ] Initial benchmark report reproducible.
- [ ] Real-client smoke results recorded.
- [ ] Foundational Phase 1 ADRs accepted.
- [ ] Requirement traceability updated.
