# benchmarks

Phase 0 synthetic scale and benchmark baseline (FIRST-10-ISSUES.md Issue 9).

- `datasets/tier-{a,b,c}.json` — deterministic dataset generation results/timings
  (A: 1k/1.5k/15k; B: 35k/50k/560k; C: 100k/500k/5M artists/albums/songs).
- `results/tier-*-<date>.json` — endpoint timing matrices with full environment metadata.
- `results/rust-native-current-*.json` — current Rust native-API acceptance at the 560,000-Track
  reference catalog, including proposed-budget verdicts, exact scope exclusions, and remediation
  evidence from any failed candidate run.
- `SUMMARY.md` — accumulated human-readable rows.

Regenerate: `python3 scripts/generate_benchmark_dataset.py --tier B` then
`python3 scripts/run_benchmarks.py --tier B`. Datasets land in dedicated disposable
databases (`melodee_bench_{tier}`) — never in the capture fixture DB.

Evidence: the committed tier datasets, result matrices, and `SUMMARY.md`. The original
scale-baseline narrative report (findings, budgets, hazards) is a withdrawn evidence report.

The legacy tier files are behavior-oracle baselines, not current Rust acceptance. Generate the Rust
current-catalog result with the ignored release-mode test command recorded in its JSON report; do
not copy legacy timings into a Rust release decision.
