# ADR-0108: Use external FFmpeg, ffprobe, and Chromaprint tools

- **Date:** 2026-08-07
- **Status:** Accepted
- **Decision owners:** Unassigned
- **Supersedes:** None
- **Superseded by:** None
- **Related requirements:** PRD-MEDIA-003, PRD-FP-003

## Context

Melodee already normalizes diverse inputs to MP3 and needs broad codec/container support plus audio
fingerprints. Replacing mature media tools with native Rust codecs is unnecessary risk.

## Decision

Proposed: invoke FFmpeg/ffprobe/fpcalc as structured child processes behind typed interfaces. Use
Rust-native metadata parsing when beneficial, with ffprobe fallback. Capture versions and arguments in
manifests.

## Consequences

- Container packages and license notices must be explicit.
- Process management, cancellation, resource limits, and sanitized diagnostics are required.

## Phase 0 ratification (2026-08-09)

- **Status change:** Proposed -> Accepted (Issue 10 ratification authority).
- **PRD support:** PRD-MEDIA-003: "The rewrite should coordinate FFmpeg/ffprobe rather than
  implement audio codecs in Rust." PRD-FP-003: "Chromaprint/fpcalc is the preferred initial
  FOSS fingerprint mechanism, subject to ADR approval and performance testing."
- **Phase 0 evidence:** the media baseline measured the legacy ffmpeg ingest conversion path
  end-to-end (MP3 384k VBR-q4 48kHz; `compatibility/media/baseline.json`) and confirmed the
  seeded streaming-transcode templates are malformed/unconsumed — the Rust implementation owns
  the real transcoding path behind typed interfaces.
- **Technology revalidation (2026-08-09):** ffmpeg 8.1.2 (installed, active upstream);
  acoustid/chromaprint active (pushed 2026-07-28), MIT for its own source per upstream
  LICENSE.md (GitHub's license detection shows a false NOASSERTION; FFmpeg parts remain LGPL).
  NOTE: the local Fedora ffmpeg build is GPL-enabled (`--enable-gpl`, libfdk-aac) — container
  packaging must deliberately select the build and ship license notices, as this ADR's
  consequences section already requires.
