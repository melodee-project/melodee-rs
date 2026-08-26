//! Property-based tests for the MQL lexer and parser (ADR-0122).
//!
//! The grammar is a public, attacker-influenced input surface (search boxes),
//! so beyond the example-based suite the parser must hold two properties under
//! arbitrary input:
//!
//! 1. **Totality** — `parse` never panics and always returns `Ok` or a typed
//!    `MqlError` for any in-bounds input string.
//! 2. **Round-trip fidelity** — any AST within the documented value grammar,
//!    rendered in fully parenthesized concrete syntax, parses back to the same
//!    AST (spans excluded).
//!
//! Generated values are restricted to what the lexer can re-read as a single
//! token (quoted text, simple regex bodies, `[a-z_]` fields) because AST values
//! like `Number`/`Date` are only produced later by plan coercion, not by the
//! parser itself.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use melodee_mql::ast::{CompareOp, MqlAst, MqlTerm, MqlValue};
use melodee_mql::{MqlError, lexer, parse, parse_with_bounds};
use proptest::prelude::*;

fn interesting_chars() -> BoxedStrategy<char> {
    let letter = (0u8..26u8).prop_map(|i| char::from(b'a' + i)).boxed();
    prop_oneof![
        4 => Just('('),
        4 => Just(')'),
        3 => Just(':'),
        3 => Just('.'),
        3 => Just('-'),
        3 => Just('/'),
        2 => Just('"'),
        2 => Just('\''),
        2 => Just('\\'),
        3 => Just('<'),
        3 => Just('>'),
        3 => Just('='),
        3 => Just('!'),
        2 => Just('&'),
        2 => Just('|'),
        1 => Just(' '),
        4 => letter,
    ]
    .boxed()
}

fn arb_input() -> BoxedStrategy<String> {
    // Bias half the cases toward dense syntax-character soup, which is where
    // tokenizers usually break.
    prop_oneof![
        1 => proptest::collection::vec(any::<char>(), 0..64),
        1 => proptest::collection::vec(interesting_chars(), 0..64),
    ]
    .prop_map(|chars| chars.into_iter().collect())
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Integration tests have no lib.rs/main.rs for seed persistence.
        failure_persistence: None,
        ..ProptestConfig::with_cases(512)
    })]

    /// Totality: no in-bounds input string panics or hangs the parser.
    #[test]
    fn parse_never_panics_on_arbitrary_input(input in arb_input()) {
        let input = truncate_bytes(&input, 4 * 1024);
        let _ = parse(input);
    }

    /// Totality for the lexer in isolation, including its bound checks.
    #[test]
    fn lex_never_panics_on_arbitrary_input(input in arb_input()) {
        let input = truncate_bytes(&input, 4 * 1024);
        let _ = lexer::lex(input, 4 * 1024, 1024);
    }

    /// Any input longer than `max_input_len` fails closed with the input-length
    /// bound error, regardless of content.
    #[test]
    fn oversize_input_always_fails_closed(prefix in arb_input()) {
        let prefix = truncate_bytes(&prefix, 64);
        let oversized = format!("{prefix}{}", "a".repeat(4 * 1024 + 1));
        match parse(&oversized) {
            Err(MqlError::ExceedsBound { kind, .. }) if kind == "input length" => {}
            other => panic!("expected input-length bound error, got {other:?}"),
        }
    }

    /// Round-trip: a generated AST rendered in fully parenthesized syntax
    /// parses back to the identical AST (spans excluded).
    #[test]
    fn rendered_ast_round_trips(ast in arb_ast(0)) {
        let rendered = render(&ast);
        let reparsed = parse(&rendered).unwrap_or_else(|e| {
            panic!("rendered query {rendered:?} failed to parse: {e:?}")
        });
        prop_assert_eq!(strip_spans(ast), strip_spans(reparsed));
    }

    /// Parsing is deterministic: the same input yields the same AST or error.
    #[test]
    fn parse_is_deterministic(input in arb_input()) {
        let input = truncate_bytes(&input, 4 * 1024);
        prop_assert_eq!(parse(input).map_err(err_fingerprint), parse(input).map_err(err_fingerprint));
    }
}

// ---------------------------------------------------------------------------
// AST generation
// ---------------------------------------------------------------------------

fn arb_field() -> impl Strategy<Value = String> {
    // Fields must lex as a single Word that is not a boolean keyword and does
    // not start with '/' (regex literal) or '.' (range operator arm).
    "[a-z_][a-z0-9_]{0,7}".prop_filter("reserved field", |w| {
        !matches!(w.to_ascii_uppercase().as_str(), "AND" | "OR" | "NOT")
    })
}

fn arb_quoted_body() -> impl Strategy<Value = String> {
    // Any text is safe inside quotes once '\' and '"' are escaped; the lexer
    // resolves exactly the escape set the renderer emits.
    proptest::collection::vec(
        prop_oneof![
            8 => "[a-z0-9 ]".prop_map(|s| s.chars().next().unwrap_or_default().to_string()),
            1 => Just("\\\\".to_string()),
            1 => Just("\\\"".to_string()),
            1 => Just("\n".to_string()),
        ],
        0..12,
    )
    .prop_map(|parts| parts.concat())
}

fn arb_regex_body() -> impl Strategy<Value = String> {
    // Regex bodies must survive as one Word token: word characters only, and a
    // '/' would terminate the literal, so exclude it.
    "[a-z0-9_]{1,8}"
}

fn arb_value() -> impl Strategy<Value = MqlValue> {
    prop_oneof![
        3 => arb_quoted_body().prop_map(MqlValue::Text),
        1 => arb_regex_body().prop_map(MqlValue::Regex),
    ]
}

fn arb_range_value() -> impl Strategy<Value = MqlValue> {
    (arb_quoted_body(), arb_quoted_body()).prop_map(|(low, high)| {
        MqlValue::Range(
            Some(Box::new(MqlValue::Text(low))),
            Some(Box::new(MqlValue::Text(high))),
        )
    })
}

fn arb_term() -> BoxedStrategy<MqlAst> {
    (any::<bool>(), proptest::option::of(arb_field()))
        .prop_flat_map(|(negate, field)| {
            // Bare terms have no operator slot in the concrete syntax, so they
            // are always `Eq`; field terms can carry any comparison operator.
            // Ranges require a field context to re-parse unambiguously.
            let (op, value) = match field.as_ref() {
                None => (Just(CompareOp::Eq).boxed(), arb_value().boxed()),
                Some(_) => (
                    prop_oneof![
                        6 => Just(CompareOp::Eq),
                        1 => Just(CompareOp::NotEq),
                        1 => Just(CompareOp::Lt),
                        1 => Just(CompareOp::Lte),
                        1 => Just(CompareOp::Gt),
                        1 => Just(CompareOp::Gte),
                    ]
                    .boxed(),
                    prop_oneof![
                        3 => arb_value(),
                        1 => arb_range_value(),
                    ]
                    .boxed(),
                ),
            };
            (Just(negate), Just(field), op, value)
        })
        .prop_map(|(negate, field, op, value)| {
            MqlAst::Term(Box::new(MqlTerm {
                field,
                op,
                value,
                negate,
                span: (0, 0),
            }))
        })
        .boxed()
}

fn arb_ast(depth: usize) -> BoxedStrategy<MqlAst> {
    if depth >= 5 {
        arb_term().boxed()
    } else {
        prop_oneof![
            2 => arb_term(),
            3 => (arb_ast(depth + 1), arb_ast(depth + 1))
                .prop_map(|(a, b)| MqlAst::And(Box::new(a), Box::new(b))),
            3 => (arb_ast(depth + 1), arb_ast(depth + 1))
                .prop_map(|(a, b)| MqlAst::Or(Box::new(a), Box::new(b))),
            2 => arb_ast(depth + 1).prop_map(|a| MqlAst::Not(Box::new(a))),
        ]
        .boxed()
    }
}

// ---------------------------------------------------------------------------
// Rendering (fully parenthesized, always re-parseable)
// ---------------------------------------------------------------------------

fn render(ast: &MqlAst) -> String {
    match ast {
        MqlAst::Term(t) => render_term(t),
        MqlAst::And(a, b) => format!("({}) and ({})", render(a), render(b)),
        MqlAst::Or(a, b) => format!("({}) or ({})", render(a), render(b)),
        MqlAst::Not(inner) => format!("not ({})", render(inner)),
    }
}

fn render_term(t: &MqlTerm) -> String {
    let neg = if t.negate { "-" } else { "" };
    let op = match t.op {
        CompareOp::Eq => ":",
        other => other.as_str(),
    };
    match (&t.field, &t.value) {
        (None, MqlValue::Text(s)) => format!("{neg}{}", quote(s)),
        (None, MqlValue::Regex(body)) => format!("{neg}/{body}/"),
        (None, MqlValue::Range(_, _)) => "justtext".to_string(),
        (None, other) => format!("{neg}{other:?}"),
        (Some(f), value) => format!("{neg}{f}{op}{}", render_value(value)),
    }
}

fn render_value(value: &MqlValue) -> String {
    match value {
        MqlValue::Text(s) => quote(s),
        MqlValue::Regex(body) => format!("/{body}/"),
        MqlValue::Range(low, high) => {
            let low = low.as_ref().map(|v| render_value(v)).unwrap_or_default();
            let high = high.as_ref().map(|v| render_value(v)).unwrap_or_default();
            format!("{low}..{high}")
        }
        other => format!("{other:?}"),
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Comparison helpers
// ---------------------------------------------------------------------------

fn strip_spans(ast: MqlAst) -> MqlAst {
    fn go(node: MqlAst) -> MqlAst {
        match node {
            MqlAst::Term(mut t) => {
                t.span = (0, 0);
                MqlAst::Term(t)
            }
            MqlAst::And(a, b) => MqlAst::And(Box::new(go(*a)), Box::new(go(*b))),
            MqlAst::Or(a, b) => MqlAst::Or(Box::new(go(*a)), Box::new(go(*b))),
            MqlAst::Not(a) => MqlAst::Not(Box::new(go(*a))),
        }
    }
    go(ast)
}

fn err_fingerprint(err: MqlError) -> String {
    // Spans depend on input offsets, which are identical for both calls, so a
    // direct format is sufficient for the determinism check.
    format!("{err:?}")
}

/// Byte-bounded truncation that respects UTF-8 char boundaries.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod bounds_shape {
    use super::*;

    #[test]
    fn property_bounds_match_adr_0122_defaults() {
        let bounds = melodee_mql::Bounds::default();
        assert_eq!(bounds.max_input_len, 4 * 1024);
        assert_eq!(bounds.max_tokens, 1024);
        // `parse_with_bounds` must also be totality-safe with tiny bounds.
        let tiny = melodee_mql::Bounds {
            max_input_len: 8,
            max_tokens: 4,
            max_ast_depth: 2,
            max_clauses: 2,
            max_regex_len: 8,
            parse_timeout_ms: 1,
        };
        let _ = parse_with_bounds("((a:b)) or (c:d)", &tiny);
    }
}
