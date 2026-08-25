---
title: Provider enrichment
description: Optional external metadata, identity evidence, artwork, cache, and recovery behavior.
sidebar:
  order: 4
---

Melodee can enrich catalog Artists and Releases from optional external providers. Provider results
are evidence and metadata, never Melodee identity authorities: a provider cannot allocate an
Artist or Release and a normalized name never merges Artists by itself.

## Supported purposes

The production provider boundary supports Artist lookup, Release metadata lookup, Artist artwork,
and Release artwork for the accepted provider inventory:

- MusicBrainz, including a configured local DecentDB replica and HTTP fallback;
- Discogs, Spotify, iTunes, Deezer, Wikidata, Brave, Metal-API, and Last.fm metadata lookup; and
- Melodee self-search for the local catalog's Artist lookup.

Last.fm account linking and scrobbling are separate from Last.fm metadata lookup. They use their
own credentials and lifecycle.

Provider calls are optional. A Release with no provider access, a disabled provider, an invalid
credential, a timeout, a malformed response, or a rate limit still imports when its local media is
structurally valid. The failure is recorded as provider work evidence and the worker retries within
its bounded policy.

## Configuration

Provider settings are layered like other Melodee settings. Set them through the admin settings
surface or `melodee-cli config import`; environment variables use the normal upper-case
`MELODEE__` form with dots replaced by `__`.

Common examples:

| Setting | Effect |
| --- | --- |
| `searchEngine.musicbrainz.enabled` | Enables MusicBrainz lookup. |
| `searchEngine.musicbrainz.storagePath` | Optional DecentDB file or directory containing `musicbrainz.ddb`; JSON artist exports are also accepted for fixtures and portable replicas. |
| `searchEngine.<provider>.baseUrl` | Selects the provider endpoint, which is useful for a local deterministic fixture or a compatible proxy. |
| `searchEngine.<provider>.enabled` | Enables the provider only when required credentials are also present. |
| `searchEngine.<provider>.timeoutMs` | Bounds one provider request from 100 to 30,000 milliseconds. |
| `searchEngine.provider.cacheTtlHours` | Sets cache freshness from 1 hour to 7 days. |
| `searchEngine.discogs.userToken` | Discogs token; secret values are redacted in exports and diagnostics. |
| `searchEngine.spotify.accessToken` | Spotify access token; it is sent only to Spotify. |
| `searchEngine.brave.apiKey` | Brave Search API key; it is sent only to Brave. |
| `scrobbling.lastFm.apiKey` | Last.fm API key used by metadata lookup and the separate scrobbling integration. |

The runtime capability endpoint reports whether a provider is enabled and configured. Blank secret
values do not count as configured. Secrets are not written to the provider cache, enrichment task
rows, attempt history, or logs.

## Artwork precedence

Administrator artwork remains first. Valid embedded and sidecar artwork continue to participate in
the catalog's accepted artwork rules; provider artwork is stored separately as evidence and is used
only when no higher-precedence catalog image is available. Provider refreshes never overwrite an
administrator image. Provider images are bounded to 10 MiB, decoded, dimension-checked, and
normalized to PNG before persistence.

See [Configuration](../installation/configuration/) for the complete settings layering rules and
[System and diagnostics](../admin/system-diagnostics/) for operational checks.
