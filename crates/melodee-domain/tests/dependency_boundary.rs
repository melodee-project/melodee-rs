//! Compile-time boundary guard for PRD-ARCH-001 / ADR-0104: the domain
//! crate stays free of web, SQL, filesystem, observability, and framework
//! dependencies, and never depends on sibling workspace crates. Boundaries
//! are enforced by the dependency graph plus this test, not by convention.
//! The allow-list below is the point: adding any dependency to
//! `melodee-domain` requires deliberately editing this test.
#![allow(clippy::expect_used, clippy::panic)]

use std::path::Path;

/// The complete set of dependencies the domain crate is allowed to carry.
/// Pure value/identity/manifest types only: hashing, (de)serialization,
/// typed errors, time, and UUIDs.
const ALLOWED_DEPENDENCIES: &[&str] =
    &["blake3", "serde", "serde_json", "thiserror", "time", "uuid"];

/// Categories that must never appear, named explicitly so a violation
/// message explains the boundary instead of only failing the allow-list:
/// web frameworks and HTTP clients, SQL/data stores, the search engine,
/// async runtimes, and observability/framework plumbing.
const FORBIDDEN_DEPENDENCIES: &[&str] = &[
    "axum",
    "axum-extra",
    "tower",
    "tower-http",
    "hyper",
    "reqwest",
    "actix-web",
    "warp",
    "sqlx",
    "sea-orm",
    "diesel",
    "tantivy",
    "tokio",
    "tracing",
    "tracing-subscriber",
    "config",
];

fn dependency_keys(manifest: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let section = trimmed.trim_matches(['[', ']']);
            in_dependencies = section == "dependencies"
                || section == "dev-dependencies"
                || section == "build-dependencies"
                || (section.starts_with("target.") && section.ends_with(".dependencies"));
            continue;
        }
        if in_dependencies
            && !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && let Some((key, _)) = trimmed.split_once('=')
        {
            // `name.workspace = true` and dotted table keys reduce to the
            // crate name before the first dot.
            let name = key.trim().split('.').next().unwrap_or_default();
            keys.push(name.trim_matches('"').to_owned());
        }
    }
    keys
}

#[test]
fn domain_dependencies_stay_inside_the_allow_list() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read domain manifest");
    let keys = dependency_keys(&manifest);
    assert!(
        !keys.is_empty(),
        "manifest dependency sections must parse non-empty"
    );
    for key in &keys {
        assert!(
            !FORBIDDEN_DEPENDENCIES.contains(&key.as_str()),
            "domain boundary violation: `{key}` is a web/SQL/framework dependency"
        );
        assert!(
            ALLOWED_DEPENDENCIES.contains(&key.as_str()),
            "domain boundary violation: `{key}` is not on the pinned allow-list \
             (PRD-ARCH-001/ADR-0104); extending it is a deliberate architecture \
             decision recorded here"
        );
    }
}

#[test]
fn domain_never_depends_on_sibling_workspace_crates() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path).expect("read domain manifest");
    let keys = dependency_keys(&manifest);
    for key in &keys {
        assert!(
            !key.starts_with("melodee-"),
            "domain must not depend on sibling crate `{key}`"
        );
    }
    let dependencies_section = manifest
        .split("[dependencies]")
        .nth(1)
        .and_then(|rest| rest.split('[').next())
        .expect("dependencies section");
    assert!(
        !dependencies_section.contains("path ="),
        "domain dependencies must be crates.io/workspace-pinned, never path-linked siblings"
    );
}
