//! Fast executable guard for the benchmark contract.
//!
//! The recorded 10-million-Track acceptance remains a manual release-scale
//! run. This small corpus belongs in the default gate so a broken generator,
//! incorrect result assertion, media read, or catastrophic latency regression
//! cannot silently leave the evidence binary green only on paper.

#![allow(clippy::expect_used)]

#[test]
fn benchmark_binary_enforces_counts_correctness_and_interactive_budget() {
    let directory = tempfile::tempdir().expect("benchmark temporary root");
    let root = directory.path().join("search");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_phase6_benchmark"))
        .args([
            "--tracks",
            "10000",
            "--root",
            root.to_str().expect("UTF-8 benchmark root"),
        ])
        .output()
        .expect("run benchmark binary");
    assert!(
        output.status.success(),
        "benchmark failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse benchmark report");
    assert_eq!(report["tracks"], 10_000);
    assert_eq!(report["total_documents"], 95_000);
    assert_eq!(report["corpus_identity"], "ordinal-v1");
    assert_eq!(report["source_media_reads"], 0);
    assert_eq!(report["canonical_media_reads"], 0);
    assert_eq!(report["target_met"], true);
    assert!(
        report["aggregate_warm_p95_ms"]
            .as_f64()
            .is_some_and(|p95| p95 < 50.0)
    );
}
