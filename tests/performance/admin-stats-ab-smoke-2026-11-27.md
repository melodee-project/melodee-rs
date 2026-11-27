# Admin statistics route A/B smoke, 2026-11-27

A small invocation-owned fixture containing 35 Artists, 50 Releases and 150
Tracks exercised the authenticated in-process admin statistics route in ABBA
order. All four runs completed with nine samples each. The entire normalized
response serialized identically across all samples and variants, including key
and array ordering and nulls; the sole excluded field was `generatedAt`, whose
RFC3339 timestamp was validated before removal. This was not byte equality of
the original time-varying responses. The
[sanitized record](admin-stats-ab-smoke-2026-11-27.json) retains the normalized
response digest and fixture counts, but no raw response or credential.

This is a harness and compatibility smoke, not representative performance
evidence. It used a synthetic PostgreSQL catalog and an in-process router,
without TCP traffic or concurrent ingestion. Fixture publication references
are synthetic; no genuine import receipts were created. The first attempt
seeded successfully but exited with a generic ValueError without preserving
its exact reason. A subsequent reviewed collector change recognizes the exact
libtest first-sample prefix and reports only allowlisted static failure reasons;
the repeated smoke passed. Do not infer a measured improvement from this result.

The [baseline](admin-stats-ab-baseline-build-2026-11-27.json) and
[candidate](admin-stats-ab-candidate-build-2026-11-27.json) release-test
proofs record snapshots based on dirty intermediate HEAD
`37f29889fb05d1825faecb8b597387f0eb971b5e`; the snapshots' 577 source
fingerprints differ only in `apps/melodee-server/src/admin_stats.rs`. The
candidate used invocation-owned copy-on-write dependency artifacts from the
baseline, and `melodee-server` was explicitly invalidated and recompiled in
the candidate target. Its final test-binary hash differs from the baseline.
The initial candidate build incorrectly reused the baseline binary; that proof
was discarded, and was never used for the smoke. The
[matching migration CLI build](admin-stats-ab-cli-build-2026-11-27.json) was
built against the candidate snapshot, and the fixture verifies the CLI proof
before and after migration. The CLI build emitted 51 warnings from the
provisional ingestion snapshot; this is not final quality-gate evidence.

A representative-scale A/B run and a final integrated-source measurement
remain outstanding. Matching source hashes do not attest dependency/toolchain
binaries or exclude transient edits, and the sample sizes in the smoke are too
small for useful percentile estimates.
