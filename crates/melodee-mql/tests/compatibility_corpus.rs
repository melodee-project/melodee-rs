//! Executable frozen-legacy MQL compatibility corpus (ADR-0122).

#![allow(clippy::expect_used, clippy::panic)]

use melodee_mql::{MqlError, parse_and_plan};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Corpus {
    schema_version: u32,
    authority: Authority,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Authority {
    legacy_commit: String,
    sources: Vec<String>,
    governing_decision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    id: String,
    expression: String,
    outcome: String,
    preserves_legacy: bool,
    legacy_bug_not_preserved: Option<String>,
}

#[test]
fn frozen_legacy_capabilities_and_rejected_bugs_are_explicit() {
    let corpus: Corpus = serde_json::from_str(include_str!(
        "../../../compatibility/mql/phase6-corpus.json"
    ))
    .expect("valid compatibility corpus");
    assert_eq!(corpus.schema_version, 1);
    assert_eq!(
        corpus.authority.legacy_commit,
        "4028b58dbd3302ed431e2c5b32d9dde5634db218"
    );
    assert!(corpus.authority.sources.len() >= 4);
    assert_eq!(
        corpus.authority.governing_decision,
        "design/adr/ADR-0122-mql-syntax-and-semantics.md"
    );
    let mut ids = std::collections::BTreeSet::new();
    let mut rejected_bugs = 0;
    for case in corpus.cases {
        assert!(ids.insert(case.id.clone()), "duplicate case id {}", case.id);
        let expression = if case.expression == "__OVERLONG__" {
            "x".repeat(4_097)
        } else {
            case.expression
        };
        let result = parse_and_plan(&expression);
        match case.outcome.as_str() {
            "valid" => assert!(result.is_ok(), "{}: {result:?}", case.id),
            "syntax" => assert!(
                matches!(result, Err(MqlError::Syntax { .. })),
                "{}",
                case.id
            ),
            "exceeds_bound" => assert!(
                matches!(result, Err(MqlError::ExceedsBound { .. })),
                "{}",
                case.id
            ),
            "type_mismatch" => assert!(
                matches!(result, Err(MqlError::TypeMismatch { .. })),
                "{}",
                case.id
            ),
            other => panic!("unknown expected outcome {other}"),
        }
        if case.legacy_bug_not_preserved.is_some() {
            assert!(!case.preserves_legacy);
            rejected_bugs += 1;
        }
    }
    assert_eq!(rejected_bugs, 4);
}
