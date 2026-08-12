//! CLI search configuration safety regressions.

#![allow(clippy::expect_used)]

use std::process::Command;

#[test]
fn search_status_rejects_overlap_with_an_inbound_source_without_leaking_paths() {
    let directory = tempfile::TempDir::new().expect("temporary root");
    let source = directory.path().join("inbound");
    std::fs::create_dir(&source).expect("inbound source");
    let search = source.join("search");

    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(["search", "status"])
        .env("MELODEE__SEARCH__ROOT", &search)
        .env("MELODEE__INGEST__SOURCE_ROOT", &source)
        .env_remove("MELODEE__INGEST__SOURCES_JSON")
        .env_remove("MELODEE__STORAGE__ROOT")
        .output()
        .expect("run search status");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("search root ownership or configuration is invalid"));
    assert!(!stderr.contains(&source.display().to_string()));
    assert!(!stderr.contains(&search.display().to_string()));
}
