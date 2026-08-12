//! Executable frozen-legacy MQL compatibility corpus (ADR-0122).

#![allow(clippy::expect_used, clippy::panic)]

use melodee_mql::{
    CatalogQueryPlan, CompareOp, MqlError, MqlValue, PlanNode, PredicateTarget, parse_and_plan,
};
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
    expected_plan: Option<String>,
    preserves_legacy: bool,
    legacy_bug_not_preserved: Option<String>,
}

#[test]
fn frozen_legacy_capabilities_and_rejected_bugs_are_explicit() {
    let corpus: Corpus = serde_json::from_str(include_str!(
        "../../../compatibility/mql/phase6-corpus.json"
    ))
    .expect("valid compatibility corpus");
    assert_eq!(corpus.schema_version, 2);
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
            "valid" => {
                let plan = result.unwrap_or_else(|error| panic!("{}: {error:?}", case.id));
                assert_eq!(
                    normalize_plan(&plan),
                    case.expected_plan
                        .as_deref()
                        .unwrap_or_else(|| panic!("{} lacks expectedPlan", case.id)),
                    "{}",
                    case.id
                );
            }
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
        if case.outcome != "valid" {
            assert!(case.expected_plan.is_none(), "{}", case.id);
        }
        if case.legacy_bug_not_preserved.is_some() {
            assert!(!case.preserves_legacy);
            rejected_bugs += 1;
        }
    }
    assert_eq!(rejected_bugs, 4);
}

fn normalize_plan(plan: &CatalogQueryPlan) -> String {
    normalize_node(&plan.root)
}

fn normalize_node(node: &PlanNode) -> String {
    match node {
        PlanNode::Leaf(predicate) => format!(
            "{}{}:{}:{}",
            if predicate.negate { "negate:" } else { "" },
            target_name(predicate.target),
            op_name(predicate.op),
            value_name(&predicate.value)
        ),
        PlanNode::And(left, right) => {
            format!("and({},{})", normalize_node(left), normalize_node(right))
        }
        PlanNode::Or(left, right) => {
            format!("or({},{})", normalize_node(left), normalize_node(right))
        }
        PlanNode::Not(inner) => format!("not({})", normalize_node(inner)),
    }
}

fn target_name(target: PredicateTarget) -> &'static str {
    match target {
        PredicateTarget::Default => "default",
        PredicateTarget::Artist => "artist",
        PredicateTarget::PrimaryArtist => "primary_artist",
        PredicateTarget::FeaturedArtist => "featured_artist",
        PredicateTarget::Performer => "performer",
        PredicateTarget::Other => "other",
        PredicateTarget::Alias => "alias",
        PredicateTarget::Title => "title",
        PredicateTarget::Name => "name",
        PredicateTarget::ReleaseTitle => "release_title",
        PredicateTarget::TrackTitle => "track_title",
        PredicateTarget::Genre => "genre",
        PredicateTarget::Tag => "tag",
        PredicateTarget::Year => "year",
        PredicateTarget::Duration => "duration",
        PredicateTarget::Date => "date",
        PredicateTarget::Type => "type",
        PredicateTarget::Status => "status",
        PredicateTarget::Id => "id",
        PredicateTarget::PublicId => "public_id",
    }
}

fn op_name(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Eq => "eq",
        CompareOp::NotEq => "not_eq",
        CompareOp::Lt => "lt",
        CompareOp::Lte => "lte",
        CompareOp::Gt => "gt",
        CompareOp::Gte => "gte",
    }
}

fn value_name(value: &MqlValue) -> String {
    match value {
        MqlValue::Text(value) => format!("text({value})"),
        MqlValue::Regex(value) => format!("regex({value})"),
        MqlValue::Number(value) => format!("number({value})"),
        MqlValue::Date(year, month, day) => format!("date({year:04}-{month:02}-{day:02})"),
        MqlValue::Range(low, high) => format!(
            "range({},{})",
            low.as_deref()
                .map(value_name)
                .unwrap_or_else(|| "open".to_string()),
            high.as_deref()
                .map(value_name)
                .unwrap_or_else(|| "open".to_string())
        ),
    }
}
