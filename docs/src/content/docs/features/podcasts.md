---
title: Podcasts
description: Podcast subscriptions, playback, and resume in Melodee.
sidebar:
  order: 2
---

The native Melodee API supports podcast subscription CRUD, bounded channel and episode browsing,
episode search, explicit refresh, download queue state, playback history, resume bookmarks, OPML
import/export, and iTunes-backed discovery, trending, and lookup.

Podcast routes require native-v1 bearer authentication plus the `HasPodcastRole` role (an
administrator is also allowed). Set `MELODEE__PODCAST__ENABLED=false` to disable the surface; disabled
routes return `404`. See [Configuration](../installation/configuration/) for discovery endpoint and
security details.

Subscriptions and playback state are isolated by user. Creating the same feed again, including a
case-only URL variation, adopts the existing subscription instead of duplicating it. Playback
heartbeats update one active episode per user rather than adding completed-history entries.
Bookmark writes are upserts, page sizes and OPML input are bounded, and deleting a subscription
removes only derived podcast state. It never deletes music-library media.

OPML import parses bounded XML, rejects malformed documents and document-type declarations, accepts
RSS outlines with HTTP or HTTPS `xmlUrl` values, and reports imported, duplicate, and invalid feeds
separately. Export produces an OPML 2.0 subscription document.

Discovery uses outbound HTTPS with redirects disabled and strict concurrency, timeout, response
size, and result limits. An upstream outage returns the native API's generic failure envelope; it
does not expose the upstream response or send Melodee credentials.

When the podcast worker and its dedicated storage root are configured, refreshes parse bounded
RSS/Atom feeds, including CDATA metadata, and idempotently upsert episodes by feed identity.
Downloads use just-in-time PostgreSQL leases, claimant-specific temporary files, bounded retries and
body sizes, and no-replace publication. Expired leases are recoverable without allowing a stale
worker to remove its successor's temporary file;
retention excludes bookmarked episodes; cleanup removes only verified direct children of the owned
podcast root in bounded, set-based batches. Feed and enclosure requests reject redirects and
non-public IPv4/IPv6 destinations, including translated or embedded private addresses. The root must
not overlap inbound, normalization, canonical-media, or search storage.
