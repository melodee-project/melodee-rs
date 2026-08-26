# benchmarks

Phase 0 synthetic scale and benchmark baseline (FIRST-10-ISSUES.md Issue 9).

- `datasets/tier-{a,b,c}.json` — deterministic dataset generation results/timings
  (A: 1k/1.5k/15k; B: 35k/50k/560k; C: 100k/500k/5M artists/albums/songs).
- `results/tier-*-<date>.json` — endpoint timing matrices with full environment metadata.
- `SUMMARY.md` — accumulated human-readable rows.

Regenerate: `python3 scripts/generate_benchmark_dataset.py --tier B` then
`python3 scripts/run_benchmarks.py --tier B`. Datasets land in dedicated disposable
databases (`melodee_bench_{tier}`) — never in the capture fixture DB.

Evidence report: historical artifact removed (findings, budgets, hazards).
