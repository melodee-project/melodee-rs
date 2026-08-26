# media

Phase 0 media-processing behavioral baseline (FIRST-10-ISSUES.md Issue 8).

- `baseline.json` — machine-readable aggregate (environment, seeded conversion/imaging/
  validation/transcoding settings, case index).
- `cases/<slug>.json` — per-case input characteristics, extraction outcome (`mcli tags show`
  via the production AtlMetaTag pipeline), pipeline outcome (staging tree, melodee.json
  status/reasons/messages, artwork, naming, leftovers), and validator outcome.
- Generator: `python3 scripts/generate_media_corpus.py` (deterministic ffmpeg-synthesized
  corpus into /tmp/media-baseline; fully redistributable, no copyrighted media).
- Capture: `python3 scripts/capture_media_baseline.py`.
- Evidence report: historical artifact removed (includes the two critical
  legacy defects F1 metadata-container gate and F2 same-size destructive dedupe).
