//! MQL parser/validator tests (acceptance A20, A21, A22).
//!
#![allow(clippy::expect_used, clippy::panic)]

use melodee_mql::{
    Bounds, CatalogQueryPlan, CompareOp, MqlAst, MqlError, MqlValue, PlanNode, PlanPredicate,
    PredicateTarget, parse, parse_and_plan,
};

fn ast(input: &str) -> MqlAst {
    parse(input).expect("parse ok")
}

#[test]
fn precedence_and_binds_tighter_than_or() {
    // a AND b OR c  ==  (a AND b) OR c
    let a = ast("a AND b OR c");
    match a {
        MqlAst::Or(l, _) => match *l {
            MqlAst::And(_, _) => {}
            other => panic!("left of OR should be AND, got {other:?}"),
        },
        other => panic!("expected OR root, got {other:?}"),
    }
}

#[test]
fn parentheses_override_precedence() {
    let a = ast("(a OR b) AND c");
    match a {
        MqlAst::And(l, _) => match *l {
            MqlAst::Or(_, _) => {}
            other => panic!("left of AND should be OR group, got {other:?}"),
        },
        other => panic!("expected AND root, got {other:?}"),
    }
}

#[test]
fn implicit_and_between_bare_terms() {
    let a = ast("foo bar");
    assert!(matches!(a, MqlAst::And(_, _)));
}

#[test]
fn nested_parentheses_depth() {
    let a = ast("((a OR b) AND (c OR d))");
    assert!(a.depth() >= 3);
    assert_eq!(a.leaf_count(), 4);
}

#[test]
fn quoted_string_escapes() {
    let plan = parse_and_plan(r#"title:"foo \"bar\" baz""#).expect("plan");
    let node = first_leaf(&plan);
    assert_eq!(node.value, MqlValue::Text("foo \"bar\" baz".to_string()));
    assert_eq!(node.target, PredicateTarget::Title);
}

#[test]
fn numeric_comparison_coerced() {
    let plan = parse_and_plan("year>=2000").expect("plan");
    let node = first_leaf(&plan);
    assert_eq!(node.target, PredicateTarget::Year);
    assert_eq!(node.op, CompareOp::Gte);
    assert_eq!(node.value, MqlValue::Number(2000));
}

#[test]
fn negative_numeric_values_are_not_confused_with_clause_negation() {
    let plan = parse_and_plan("year:-1 AND duration>=-250").expect("signed numeric plan");
    let PlanNode::And(left, right) = plan.root else {
        panic!("expected two signed numeric predicates");
    };
    let PlanNode::Leaf(year) = *left else {
        panic!("expected year predicate");
    };
    let PlanNode::Leaf(duration) = *right else {
        panic!("expected duration predicate");
    };
    assert_eq!(year.value, MqlValue::Number(-1));
    assert_eq!(duration.value, MqlValue::Number(-250));
}

#[test]
fn inclusive_range_parsed() {
    let plan = parse_and_plan("year:2000..2010").expect("plan");
    let node = first_leaf(&plan);
    assert_eq!(
        node.value,
        MqlValue::Range(
            Some(Box::new(MqlValue::Number(2000))),
            Some(Box::new(MqlValue::Number(2010)))
        )
    );
}

#[test]
fn open_start_range_parsed() {
    let plan = parse_and_plan("year:..2010").expect("plan");
    let node = first_leaf(&plan);
    assert_eq!(
        node.value,
        MqlValue::Range(None, Some(Box::new(MqlValue::Number(2010))))
    );
}

#[test]
fn regex_value_parsed() {
    let plan = parse_and_plan("title:/foo.*bar/").expect("plan");
    let node = first_leaf(&plan);
    assert_eq!(node.value, MqlValue::Regex("foo.*bar".to_string()));
}

#[test]
fn empty_regex_is_rejected() {
    assert!(matches!(
        parse_and_plan("title://").expect_err("empty regex"),
        MqlError::RegexError { .. }
    ));
}

#[test]
fn regex_bound_counts_characters_not_utf8_bytes() {
    let expression = format!("title:/{}/", "界".repeat(200));
    parse_and_plan(&expression).expect("200 characters is below the 256-character bound");
}

#[test]
fn negation_prefix() {
    let plan = parse_and_plan("-title:x").expect("plan");
    let node = first_leaf(&plan);
    assert!(node.negate);
    assert_eq!(node.target, PredicateTarget::Title);
}

#[test]
fn not_keyword() {
    let plan = parse_and_plan("NOT artist:x").expect("plan");
    match plan.root {
        PlanNode::Not(_) => {}
        other => panic!("expected Not, got {other:?}"),
    }
}

#[test]
fn unknown_field_is_error() {
    let err = parse_and_plan("nope:x").expect_err("should fail");
    assert!(matches!(err, MqlError::UnknownField { .. }));
}

#[test]
fn mismatched_paren_reports_span() {
    let err = parse("(a OR b").expect_err("should fail");
    match err {
        MqlError::Syntax { span, .. } => assert_eq!(span, (7, 7)),
        other => panic!("expected Syntax with span, got {other:?}"),
    }
}

#[test]
fn leading_boolean_operators_are_rejected_at_their_token() {
    for (input, expected_span) in [("AND title:x", (0, 3)), ("OR title:x", (0, 2))] {
        let error = parse(input).expect_err("leading operator must fail");
        assert!(matches!(
            error,
            MqlError::Syntax { span, .. } if span == expected_span
        ));
    }
}

#[test]
fn dangling_boolean_operators_are_rejected_at_their_token() {
    for (input, expected_span) in [("title:x AND", (8, 11)), ("title:x OR", (8, 10))] {
        let error = parse(input).expect_err("dangling operator must fail");
        assert!(matches!(
            error,
            MqlError::Syntax { span, .. } if span == expected_span
        ));
    }
}

#[test]
fn grouped_negation_error_has_the_group_span() {
    let error = parse("-(a OR b)").expect_err("dash cannot negate a group");
    assert!(matches!(error, MqlError::Syntax { span: (0, 9), .. }));
}

#[test]
fn empty_regex_reports_regex_error_at_the_regex_token() {
    for (input, expected_span) in [("title://", (6, 8)), ("//", (0, 2))] {
        let error = parse(input).expect_err("empty regex");
        assert!(matches!(
            error,
            MqlError::RegexError { span, .. } if span == expected_span
        ));
    }
}

#[test]
fn unterminated_quote_reports_span() {
    let err = parse("title:\"unterminated").expect_err("should fail");
    assert!(matches!(err, MqlError::Syntax { span: (6, 19), .. }));
}

#[test]
fn unary_and_group_eof_errors_point_at_the_input_end() {
    for (input, expected_span) in [("NOT", (3, 3)), ("-", (1, 1)), ("(", (1, 1))] {
        let error = parse(input).expect_err("incomplete expression must fail");
        assert!(matches!(
            error,
            MqlError::Syntax { span, .. } if span == expected_span
        ));
    }
}

#[test]
fn type_mismatch_numeric_field_gets_text() {
    let err = parse_and_plan("year:abc").expect_err("should fail");
    assert!(matches!(err, MqlError::TypeMismatch { .. }));
}

#[test]
fn empty_input_is_error() {
    assert!(matches!(
        parse("   ").expect_err("empty"),
        MqlError::EmptyInput
    ));
}

#[test]
fn input_too_long_exceeds_bound() {
    let big = "a ".repeat(5000);
    let bounds = Bounds::default();
    let err = melodee_mql::parse_with_bounds(&big, &bounds).expect_err("should fail");
    assert!(matches!(
        err,
        MqlError::ExceedsBound {
            kind,
            ..
        } if kind == "input length"
    ));
}

#[test]
fn too_many_clauses_exceeds_bound() {
    let mut q = String::new();
    for i in 0..300 {
        q.push_str(&format!("title:{i} "));
    }
    let err = parse_and_plan(&q).expect_err("should fail");
    assert!(matches!(
        err,
        MqlError::ExceedsBound {
            kind,
            ..
        } if kind == "clause count"
    ));
}

#[test]
fn regex_too_long_exceeds_bound() {
    let long = "x".repeat(500);
    let q = format!("title:/{long}/");
    let err = parse_and_plan(&q).expect_err("should fail");
    assert!(matches!(
        err,
        MqlError::ExceedsBound {
            kind,
            ..
        } if kind == "regex length"
    ));
}

#[test]
fn token_depth_and_deadline_bounds_are_independently_enforced() {
    let token_bounds = Bounds {
        max_tokens: 2,
        ..Bounds::default()
    };
    assert!(matches!(
        melodee_mql::parse_with_bounds("a b c", &token_bounds).expect_err("token bound"),
        MqlError::ExceedsBound { kind, limit: 2, .. } if kind == "token count"
    ));

    let depth_bounds = Bounds {
        max_ast_depth: 2,
        ..Bounds::default()
    };
    assert!(matches!(
        melodee_mql::parse_with_bounds("(((a)))", &depth_bounds).expect_err("depth bound"),
        MqlError::ExceedsBound { kind, limit: 2, .. } if kind == "AST depth"
    ));

    let deadline_bounds = Bounds {
        parse_timeout_ms: 0,
        ..Bounds::default()
    };
    assert!(matches!(
        melodee_mql::parse_with_bounds("a", &deadline_bounds).expect_err("deadline bound"),
        MqlError::ExceedsBound { kind, limit: 0, .. } if kind == "parse timeout"
    ));
}

#[test]
fn parse_is_deterministic() {
    let a = parse_and_plan("artist:x AND year>=2000").expect("plan");
    let b = parse_and_plan("artist:x AND year>=2000").expect("plan");
    assert_eq!(a, b);
}

#[test]
fn plan_serializes_stably() {
    let a = melodee_mql::explain("artist:x AND year>=2000").expect("explain");
    let b = melodee_mql::explain("artist:x AND year>=2000").expect("explain");
    assert_eq!(a, b);
}

#[test]
fn legacy_compat_implicit_and_field_search() {
    // Legacy: `colin hay` searches the default field set; `artist:...` is cross-participation.
    let plan = parse_and_plan("colin hay").expect("plan");
    let node = first_leaf(&plan);
    assert_eq!(node.target, PredicateTarget::Default);
    let plan = parse_and_plan("artist:\"colin hay\"").expect("plan");
    assert_eq!(first_leaf(&plan).target, PredicateTarget::Artist);
}

#[test]
fn legacy_compat_role_fields() {
    for (field, target) in [
        ("primaryArtist:x", PredicateTarget::PrimaryArtist),
        ("featuredArtist:x", PredicateTarget::FeaturedArtist),
        ("performer:x", PredicateTarget::Performer),
        ("other:x", PredicateTarget::Other),
    ] {
        let plan = parse_and_plan(field).expect("plan");
        assert_eq!(first_leaf(&plan).target, target, "field {field}");
    }
}

#[test]
fn unicode_terms_are_span_safe() {
    let plan = parse_and_plan("artist:Björk").expect("unicode plan");
    assert_eq!(first_leaf(&plan).value, MqlValue::Text("Björk".to_string()));
}

#[test]
fn unknown_escape_is_rejected_at_exact_span() {
    let error = parse(r#"title:"bad\q""#).expect_err("unknown escape");
    assert!(matches!(error, MqlError::Syntax { span: (10, 12), .. }));
}

#[test]
fn invalid_calendar_date_and_inverted_range_fail_closed() {
    assert!(matches!(
        parse_and_plan("date:2024-02-31").expect_err("invalid date"),
        MqlError::TypeMismatch { .. }
    ));
    assert!(matches!(
        parse_and_plan("year:2020..2000").expect_err("inverted range"),
        MqlError::RangeError { .. }
    ));
}

#[test]
fn ordering_comparison_on_text_is_rejected() {
    assert!(matches!(
        parse_and_plan("title>alpha").expect_err("text ordering"),
        MqlError::TypeMismatch { .. }
    ));
}

#[test]
fn documented_release_and_track_field_aliases_are_accepted() {
    assert!(parse_and_plan("release:Blue").is_ok());
    assert!(parse_and_plan("track:Train").is_ok());
}

/// Helper: descend to the first leaf predicate in a plan.
fn first_leaf(plan: &CatalogQueryPlan) -> &PlanPredicate {
    fn walk(node: &PlanNode) -> &PlanPredicate {
        match node {
            PlanNode::Leaf(p) => p,
            PlanNode::Not(inner) => walk(inner),
            PlanNode::And(a, _) | PlanNode::Or(a, _) => walk(a),
        }
    }
    walk(&plan.root)
}
