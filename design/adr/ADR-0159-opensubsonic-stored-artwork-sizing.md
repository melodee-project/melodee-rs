# ADR-0159: OpenSubsonic stored-artwork sizing

- **Date:** 2026-08-24
- **Status:** Accepted
- **Decision owners:** Feature Completion Program Phase 0 (explicitly user-assigned)
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MEDIA-005, PRD-API-OS-001, PRD-API-OS-005

## Context

OpenSubsonic `getCoverArt` (and the legacy `GetImageForApiKeyId` behind `getCoverArt`/
`getAvatar`) accepts a `size` query parameter (`ImageSize` enum: `small`, `medium`, `large`,
`extralarge`, `max`) and the C# implementation **resizes** the stored album/artist/playlist/chart
image to that size before serving it (`src/Melodee.Common/Services/OpenSubsonicApiService.cs:1330-1487`;
`AlbumService.GetAlbumImageBytesAndEtagAsync(apiKey, size, …)` at line 1481 is the resize/caching
leg; the image processor is injected at line 74; an `ImageSize`-keyed cache region
`urn:openSubsonic:artist-and-album-images` at line 78 caches resized variants).

Rust currently **ignores** the `size` parameter for stored artwork. In
`apps/melodee-server/src/opensubsonic/media.rs:758-785`, the `Artwork::Stored(bytes, mime_type)`
branch serves the raw stored bytes and a SHA-256 ETag regardless of `requested_size`; only the
`Artwork::Default` placeholder branch branches on `requested_size` (serving the processed default
JPEG for a sized request). This means an OpenSubsonic client that requests
`getCoverArt?id=…&size=small` receives the full-resolution stored cover, not a `small`-sized
cover — a **compatibility gap** against the frozen C# behavior.

The Rust workspace already depends on `image = 0.25.10` (jpeg/png/webp/gif features enabled) for
artwork decoding; it is the natural dependency for a resize step. The existing
`catalog_entity_images` table (migration 0030, ADR-0143) stores the canonical stored artwork;
ADR-0143's image-endpoint precedence (stored → default → generated) is preserved.

## Decision

1. **Resize stored artwork to the requested `size`.** The `Artwork::Stored` branch of
   `getCoverArt`/`getAvatar` decodes the stored bytes with the `image` crate, resizes to the
   `size` parameter's target dimension (preserving aspect ratio, fitting within the target box,
   using a high-quality resampling filter — `image`'s `Lanczos3` or `Triangle`), re-encodes to
   JPEG (or the stored format if it is PNG/WebP, matching the captured `content-type`), and
   serves the resized bytes.
2. **Size → target-dimension mapping.** The `ImageSize` enum maps to a maximum edge length in
   pixels, matching the C# `ImageProcessor` defaults:
   - `small` → 100×100
   - `medium` → 200×200
   - `large` → 500×500
   - `extralarge` → 1000×1000
   - `max` (or absent `size`) → no resize (serve the stored bytes as-is, the current
     `Artwork::Stored` behavior).
   A `size` value that maps to a target **larger** than the stored image's dimensions does **not**
   upscale; the stored image is served at its native size (the C# resizer does not upscale by
   default).
3. **ETag.** The ETag for a resized stored image is the SHA-256 hex of the **resized** bytes
   (not the stored bytes), so a sized request has a different ETag from an unsized request for the
   same artwork — matching the C# per-size cache-key behavior (`GenerateImageCacheKeyForApiKey` at
   line 1322). An unsized request keeps the current SHA-256-of-stored-bytes ETag.
4. **Caching (in-process, bounded).** Resized variants are cached in a bounded in-process
   `LruCache` keyed by `(artwork_id, size, stored_sha256)` so repeated sized requests for the same
   artwork do not re-decode/re-encode. The cache is bounded (e.g., 256 entries) and is **not** the
   source of truth; the `catalog_entity_images` stored bytes are. A cache miss re-resizes from the
   stored bytes. (A durable per-size artifact table is **not** introduced; the in-process cache is
   sufficient because the stored bytes are cheap to re-read and the resize is a CPU-bound
   operation that benefits from a short-lived LRU.)
5. **Fail-degraded.** If decoding the stored bytes fails (corrupt image, unsupported format), the
   endpoint falls back to the current `Artwork::Default` sized behavior (the processed default
   JPEG for a sized request) rather than returning an error; the ETag and content-type match the
   default branch.
6. **No new runtime dependency.** The `image` crate (already a workspace dependency) is the only
   dependency for the resize step; no new crate is added.
7. **Scope.** This applies to the OpenSubsonic `getCoverArt` and `getAvatar` endpoints (the
   `Artwork::Stored` branch). The native-v1 and Jellyfin image endpoints are **unaffected** — this
   ADR does not change their sizing behavior (they have their own captured contracts). The
   `Artwork::Default` branch and the `Artwork::Generated` (if introduced later) branch are
   unchanged.
8. **Compatibility gate.** The change is gated by a capture of the C# `getCoverArt` with a
   `size` parameter for a stored album/artist image, showing the resized byte length and ETag;
   the Rust implementation must produce a byte-for-byte (or near-byte-for-byte, within a documented
   JPEG-encoder tolerance) match on the resized output for the same input and size.

## Alternatives considered

### Serve stored bytes unresized (current behavior)

Rejected: this is the captured C# behavior; an OpenSubsonic client that requests `size=small`
expects a `small`-sized image, and serving the full-resolution image wastes bandwidth and may
exceed the client's expected dimensions. The frozen capture shows the C# resizes.

### Store per-size artifacts in the database

Rejected: a durable per-size artifact table would multiply storage by the number of `size` values
and require a re-encode on every stored-image change. An in-process LRU is simpler, bounded, and
sufficient because the resize is a cheap CPU operation and the stored bytes are always available.

### Use a separate image-resize crate (e.g., `fast_image_resize`)

Rejected: the `image` crate is already a workspace dependency with the needed codecs (jpeg/png/
webp); adding a second resize crate would increase the supply-chain surface for no benefit.

## Rationale

The `size` parameter is part of the captured OpenSubsonic `getCoverArt` contract; ignoring it is
a compatibility gap. The `image` crate is already present, so the resize step is a bounded
workstream: decode → resize → re-encode → serve, with a bounded in-process cache. The ETag change
(size-scoped) matches the C# per-size cache-key behavior and keeps the `If-None-Match` /
`304 Not Modified` flow correct for sized requests.

## Consequences

### Positive

- Closes the `getCoverArt`/`getAvatar` `size`-parameter compatibility gap.
- Bandwidth savings for clients that request small/medium sizes.
- No new runtime dependency.

### Negative / trade-offs

- A sized request now does a decode + resize + re-encode (CPU) on a cache miss; the bounded LRU
  amortizes this for repeated requests.
- The ETag for a sized stored image changes from the current SHA-256-of-stored-bytes to
  SHA-256-of-resized-bytes; this is a **compatible** change (the ETag is opaque to the client)
  but is a behavioral change that must be capture-diffed.

### Follow-up work

- Feature Completion Phase 2 (ART / ARTIST workstream): the resize step in
  `opensubsonic/media.rs`; the LRU cache; the ETag change; a capture of the C# `getCoverArt`
  with `size` for a stored image; a Rust test that decodes/resizes/re-encodes a known fixture
  and asserts the output dimensions and ETag.

## Validation

- Capture: a C# `getCoverArt?id={album-api-id}&size=small` (and `medium`, `large`) response for a
  stored album image, recording the byte length, content-type, and ETag.
- Rust test: decode a known stored JPEG fixture, resize to `small` (100×100), re-encode, and
  assert the output is ≤100px on the long edge, the content-type is `image/jpeg`, and the ETag is
  the SHA-256 of the resized bytes.
- ETag test: a sized and an unsized request for the same artwork return different ETags.
- `If-None-Match` test: a sized request with the sized ETag returns `304 Not Modified`.
- Fail-degraded test: a corrupt stored image with a `size` parameter returns the default sized
  JPEG (not an error).
- No existing unsized `getCoverArt` behavior changes (golden diff for the `size`-absent case).

## References

- [PRD.md](../../PRD.md) §13 (PRD-MEDIA-005), §20 (PRD-API-OS-001/005)
- [ADR-0143](ADR-0143-catalog-artwork-override-storage-and-image-endpoint-precedence.md)
  (image-endpoint precedence; stored bytes are the source)
- `src/Melodee.Common/Services/OpenSubsonicApiService.cs:1330-1487` (C# `GetImageForApiKeyId`,
  behavioral oracle)
- `apps/melodee-server/src/opensubsonic/media.rs:721-800` (current Rust `getCoverArt`)
