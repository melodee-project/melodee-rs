# ADR-0108: Use external FFmpeg, ffprobe, and Chromaprint tools

- **Date:** 2026-08-07
- **Status:** Proposed
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
