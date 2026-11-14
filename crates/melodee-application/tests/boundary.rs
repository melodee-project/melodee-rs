//! SPEC §7 / ADR-0195: application services orchestrate stores, never SQL or HTTP adapters.
#![allow(clippy::expect_used)]

use std::path::Path;

fn inspect_sources(directory: &Path) {
    for entry in std::fs::read_dir(directory).expect("application source directory") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            inspect_sources(&path);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(&path).expect("read source");
            for forbidden in [
                "sqlx::query",
                "sqlx::raw_sql",
                "QueryBuilder",
                "melodee_server::",
                "axum::",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "application boundary violation in {}: {forbidden}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn application_services_keep_sql_in_persistence_and_http_in_adapters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    inspect_sources(&root.join("src"));
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("application manifest");
    assert!(!manifest.contains("melodee-server"));
    assert!(!manifest.contains("axum"));
    let cli = std::fs::read_to_string(root.join("../../apps/melodee-cli/Cargo.toml"))
        .expect("CLI manifest");
    assert!(
        !cli.contains("melodee-server"),
        "CLI must consume shared use cases directly"
    );
    assert!(cli.contains("melodee-application"));
}
