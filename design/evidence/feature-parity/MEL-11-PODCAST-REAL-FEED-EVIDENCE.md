# MEL-11 — Podcast Real-Feed Qualification Evidence (Partial)

**Status:** Partial — real public-feed refresh/download/recovery qualified on the
dev topology, with three real-wire defects found and fixed. Storage-root adoption of
legacy downloaded media and configured quota/retention legs remain open.

**Candidate:** repository main at the evidence commit; dev topology
(`batman.home.arpa`, debug build, dev PostgreSQL 18, podcast worker enabled with
`MELODEE__PODCAST__STORAGE_ROOT=/mnt/incoming/melodee_test/podcasts`).
**Evidence date:** 2026-10-20/21.

## Real feeds exercised

- **Lex Fridman Podcast** — `https://lexfridman.com/feed/podcast/` (2,121,803-byte RSS,
  502 items, enclosures fronted by `media.blubrry.com` with 302 → CDN redirect chains).
- **Up First (NPR)** — `https://feeds.npr.org/510318/podcast.xml` (IPv6+IPv4, 500+ items).

## Defects the real feeds exposed (fixed in this change)

1. **`podcast.http.maxFeedBytes` was dead configuration; the worker hardcoded a
   2 MiB feed bound.** The real Lex Fridman feed (2.1 MB) failed every refresh with
   `InvalidResponse` (API 404) while the documented setting allowed 10 MiB — and
   legacy C# reads the setting at refresh time (`PodcastService.RefreshChannelAsync`).
   *Fix:* `resolve_feed_limits` resolves `podcast.http.maxFeedBytes` from the layered
   settings, clamped to a 64 MiB hard cap.
2. **Over-limit feeds failed the whole refresh; legacy truncates.** The feed has 502
   items; the parser errored when reaching the 500-item bound. Legacy applies
   `feed.Items.Take(maxItemsPerChannel)` — truncation, never a failed refresh.
   *Fix:* `parse_feed_with_limits` truncates at the configured
   `podcast.refresh.maxItemsPerChannel` (default 500, now honored from settings).
3. **Redirects were rejected; real enclosure hosts require them.** The worker built
   clients with `redirect::Policy::none()`, so the blubrry 302 → CDN chain failed every
   enclosure download. Legacy follows redirects **manually, validating every hop
   through the SSRF validator** (`PodcastHttpClient.GetAsync` +
   `ValidateRedirectAsync`, max `podcast.http.maxRedirects`, default 10).
   *Fix:* `outbound_get_following` re-implements the legacy loop: each hop is
   scheme-checked, DNS-resolved (3s bound, ≤16 addresses, public-IP enforced), pinned
   to the resolved addresses, and capped at the configured redirect bound. The client
   keeps `Policy::none()` — the loop owns hop validation, preserving per-hop SSRF
   protection (a redirect cannot bypass the private-address checks).

Tests: `feed_limits_fall_back_and_clamp` (settings fallback/clamp), `parser_truncates_
at_the_item_limit_like_legacy_take` (502-item feed truncates to 500), plus the
existing worker suite. Public docs updated in the same change
(`docs/src/content/docs/features/podcasts.md` — redirect/fetch bounds paragraph).

## Live exercise record (redacted; no credentials involved)

| Step | Result |
|---|---|
| Subscribe both real feeds (native `POST /api/v1/podcasts/channels`) | 200, channels created idempotently by (owner, lower(feed_url)) |
| Refresh Lex Fridman (pre-fix) | 404 `InvalidResponse` (defect 1/2) |
| Refresh Lex Fridman (post-fix) | 200; 500 episodes stored (truncated from 502), title/description/image parsed, `last_sync_at` set |
| Refresh NPR | 200; 500 episodes stored |
| Download one Lex episode (`POST .../download`) | 200 → `Downloaded`; **249,955,445 bytes** fetched through the blubrry 302 chain, published no-replace as `<public_id>.media`; `file`/`ffprobe` verify real ID3v2.3 MP3 (19,317 s) |
| Dead-feed refresh (`example.invalid`) | clean error classification (no hang, no retry storm) |
| Invalid-enclosure failure path | bounded retries with backoff → `Failed` after attempt limit; `podcasts stuck` lists the episode with channel/episode context |
| `podcasts recover --retry-failed` | requeued with reset attempt counters; re-download of the same enclosure converged **byte-identical** (249,955,445 bytes) — no partial publication |

## Remaining for full MEL-11 acceptance

- **Legacy downloaded-media adoption at cutover:** imported channels re-queue their
  downloads (absolute host paths requeue per ADR-0148); the accepted
  adoption/re-download disposition needs a real migrated-subscription drill
  (cutover-context, operator data).
- **Quota/retention enforcement:** the `podcast.quota.*` and `podcast.retention.*`
  settings are registered but not yet consumed by the Rust worker (legacy
  `PodcastDownloadJob` enforces per-user/per-channel quotas and retention). Recorded
  as the remaining gap; discovery-client redirect posture unchanged by this fix.
- Lease/restart recovery for an in-flight download mid-transfer (the mock/fixture
  suites cover it; a real-transfer kill test remains).
