---
title: Discovery and playback tools
description: Charts, recommendations, listening analytics, audio features, backends, and themes.
sidebar:
  order: 6
---

Native Melodee clients can use several catalog-derived discovery surfaces:

- visible curated charts, including linked Release details and generated Track playlists;
- bounded recent-catalog recommendations filtered by native content type;
- per-user listening totals and top Track, Release, and Artist rankings for day, week, month,
  year, or all-time periods;
- bounded tempo searches and stored audio-feature details.

Analytics are calculated from committed scrobbles and current catalog facts. Results are scoped to
the authenticated user and use stable public IDs. Chart playlists include Tracks from linked
Releases in chart-rank and Track order; unlinked chart rows remain visible in chart details but do
not manufacture playlist entries.

Administrators can import a chart snapshot from a bounded UTF-8 CSV without reading media:

```bash
melodee charts import \
  --file ./chart.csv \
  --slug critics-2026 \
  --title "Critics' releases of 2026" \
  --source-name "Example publication" \
  --year 2026 \
  --generated-playlist
```

Rows use `Rank,ArtistName,ReleaseTitle,ReleaseYear`; the header and year are optional. The import is
atomic, accepts at most 5 MiB and 5,000 items, requires unique positive ranks, and links a row only
when its Artist, Release title, and optional year identify exactly one active catalog Release.
Ambiguous or missing observations remain unlinked for later review; names are never treated as
catalog identity.

A periodic maintenance worker (see
[discovery maintenance settings](../installation/configuration.md#discovery-maintenance)) keeps the
discovery projections current. It reconciles stored recommendation popularity against committed
scrobbles and retries linkage for unlinked chart rows as the catalog grows, applying the same
unique-exact-match rule as the import. Existing links are never rewritten or cleared by the worker;
an ambiguous observation stays unlinked until it identifies exactly one active Release. Each pass
runs in bounded batches and is idempotent, so re-running it never duplicates or double-counts
popularity.

## Audio features and tempo

The audio-feature detail and tempo-search routes serve stored per-Track feature rows. Tempo values
are stored in the bounded range 0–1000 BPM; tempo searches require a finite, non-negative
`min`/`max` pair with `min` not greater than `max`, and results are paged (default page size 50).
Melodee does not analyze media to derive these features itself — rows populated by external tooling
are served as-is, and Tracks without a stored feature row are absent from tempo search results and
return `404` from the audio-feature detail route.

## Playback backend

The native playback-backend routes report status and capabilities, initialize or stop the optional
backend, register its shared endpoint, and accept active-session heartbeats. When the backend is
disabled, status, capabilities, initialization, and registration preserve the native `400` response
with `Jukebox is not enabled`; shutdown remains a safe successful no-op.

## Themes

Theme discovery and individual theme assets are public so the sign-in experience can render them.
Personal preference and theme administration use the browser session cookie, not native bearer
authentication. Administrators can rescan, import, export, and delete non-built-in theme packs.
Built-in themes cannot be deleted.

Theme ZIP imports are capped at 50 MiB compressed and 100 MiB expanded, with at most 256 entries
and 10 MiB per asset. Archives may contain only supported theme asset types using Stored or DEFLATE
compression. Encrypted entries, links, special files, absolute/traversal/backslash paths, duplicate
paths, unsupported extensions, and missing `theme.json` or `theme.css` are rejected before any
database write. Exported packs are deterministic and never expose an arbitrary filesystem path.
