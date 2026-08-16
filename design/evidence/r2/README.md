# R2 — Mandatory Runtime Implementation Gaps

Date: 2026-08-16. Status: In progress. Depends on: R1 (`2c54be5`). Scope per
`IMPLMENTATION_OUTSTANDING.md` §7: R2.1 media-format matrix, R2.2 CUE/sidecar, R2.3 configured
transcoding, R2.4 bounded resource classes, R2.5 observability/health, R2.6 scheduled/provider
behavior, R2.7 performance/fault evidence — plus the five bounded-input fixes assigned to R2 by
the R1.3 security baseline inventory (R2-SEC-1..5).

## 1. R2-SEC — Security-inventory bounded-input fixes

Source: [`../r1/security-baseline-inventory.md`](../r1/security-baseline-inventory.md) follow-up
table. Four fixes implemented; one rejected on compatibility grounds. No external contract
changed: each fix tightens only server-internal handling on paths whose captured behavior is
preserved byte-for-byte.

### 1.1 Dispositions

| ID | Inventory finding | Disposition | Proof |
|---|---|---|---|
| R2-SEC-1 | Last.fm 256 KiB response cap was post-read (`bytes()` buffered the full body before the limit check) | **Fixed.** `lastfm.rs::parse_response` now rejects early on a declared `Content-Length` over `MAX_RESPONSE_BYTES` and otherwise enforces the cap per chunk while streaming `bytes_stream()`, so a hostile or proxied endpoint cannot force an unbounded pre-check allocation | `tests/lastfm_scrobble.rs::oversized_chunked_response_is_capped_while_streaming` — mock endpoint returns a chunked body (no `Content-Length`) of 40×8192 bytes; the client must surface `MalformedResponse` without buffering it. Suite: 10 passed |
| R2-SEC-2 | ffprobe argv lacked the `--` separator fpcalc carries | **Fixed.** Pure `ffprobe_args(path)` in `crates/melodee-media/src/lib.rs` appends `--` before the path operand (defense in depth; all current paths are absolute, but the separator keeps the operand interpretation if that invariant is ever relaxed) | Unit test `ffprobe_argv_separates_options_from_the_media_path` asserts the separator position and sole trailing operand. Suite: 3 passed |
| R2-SEC-3 | Playlist-image upload does not enforce declared Content-Type equality with the sniffed type (artwork/avatar uploads do) | **Rejected — no wire change.** Rationale in §1.2 | Compatibility review below |
| R2-SEC-4 | OPML import had no explicit route-level body limit (manual 2 MiB check + axum implicit default fallback) | **Fixed.** `native/podcasts.rs` pins `DefaultBodyLimit::max(MAX_OPML_BYTES)` (2 MiB) on `POST /api/v1/podcasts/opml/import`, making the effective bound explicit at the route instead of relying on axum's implicit 2 MiB default coinciding with the manual check. No observable contract change: the effective limit was already 2 MiB | Oversize assertion (2 MiB + 1 byte → 413) added to `tests/native_podcasts_api.rs::all_native_podcast_routes_are_tenant_scoped_bounded_and_authorized`. Suite: 3 passed |
| R2-SEC-5 | Canonical artwork serve path verified only the recorded byte size, so an equal-length content swap would go undetected | **Fixed.** `native/images.rs::canonical_artwork_bytes` now verifies the BLAKE3 hash of the served bytes against the recorded publication `byte_hash`; mismatch logs a warning and falls back to the captured no-artwork placeholder (the frozen 404/placeholder behavior) instead of serving unverified bytes | `tests/admin_artwork_api.rs::canonical_artwork_with_corrupted_storage_falls_back_to_the_captured_placeholder` — seeds a real artwork asset, corrupts one byte same-length in a tempdir canonical root, asserts the placeholder is served. Suite: 6 passed |

Dependency note: R2-SEC-5 adds `blake3` to `apps/melodee-server`'s dependency list. BLAKE3 is
already a pinned workspace dependency (exact `=` version, committed `Cargo.lock`) used by the
domain crate; no new third-party crate enters the tree, so no ADR or dependency decision is
required.

### 1.2 R2-SEC-3 rejection rationale (compatibility review)

The inventory proposed enforcing equality between the client-declared `Content-Type` and the
sniffed image type on the native playlist-image upload (`native/playlists.rs`), matching the
artwork/avatar upload validation. The change is rejected for release 1.0 on compatibility
grounds:

1. **The route is frozen.** `POST /api/v1/playlists/{id}/image` is native API v1 route matrix id
   104 (Phase 7 acceptance). The compatibility rules forbid changing any existing external
   route's validation behavior without (a) a compatibility test showing current behavior, (b) a
   product decision, (c) an accepted ADR, and (d) an explicit migration/client plan. None exist
   for this route's Content-Type handling.
2. **Current behavior is not fully captured.** The frozen capture set covers only the empty-body
   scenario for this route. The legacy server's response to a declared-vs-sniffed mismatch is not
   documented by any accepted capture, so adding a rejection would invent new observable
   behavior rather than preserve known behavior — exactly what the frozen-contract rule exists
   to prevent.
3. **The underlying risk is already contained server-side.** Uploaded bytes are not served
   back as-is: the stored representation is canonicalized through the bounded decode/re-encode
   image pipeline, and the served `Content-Type` derives from that canonical stored form, not
   from the client-declared header. A lying `Content-Type` therefore cannot smuggle unsniffed
   bytes to a downstream client. The route already performs magic-byte sniffing (R1.3 §1.3
   inventory row), which is the control that carries the actual risk.

Disposition: rejected; no wire change; this record satisfies the backlog requirement that
security-inventory follow-ups end in either a fix or an explicit accepted decision. PRD-SEC-003
remains `planned` with owner R5, whose review scope includes this disposition.

## 2. Tests and gate (R2-SEC increment)

- `cargo test -p melodee-server --test lastfm_scrobble` — 10 passed.
- `cargo test -p melodee-media --lib` ffprobe argv tests — 3 passed.
- `cargo test -p melodee-server --test native_podcasts_api` — 3 passed.
- `cargo test -p melodee-server --test admin_artwork_api` — 6 passed.
- `./scripts/check-smoke.sh` at the commit boundary (fmt, clippy `-D warnings`, workspace lib
  tests, doc/requirement validation).

Documentation impact:
- None for `/docs` — all four fixes are server-internal hardening with no user-visible behavior
  change; the R2-SEC-3 disposition preserves the documented frozen contract. Internal: this
  report and the evidence registry index.
