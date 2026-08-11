//! Melodee MQL — pure query language for search (ADR-0122).
//!
//! Pipeline: `input -> lexer -> parser -> MqlAst -> validate -> CatalogQueryPlan`.
//! The AST and plan contain no Tantivy/SQLx/Axum/filesystem types; the
//! `melodee-search` crate translates the plan into a Tantivy query behind its
//! own boundary.

#![forbid(unsafe_code)]

pub mod ast;
pub mod errors;
pub mod lexer;
pub mod parser;
pub mod plan;

pub use ast::{CompareOp, MqlAst, MqlTerm, MqlValue};
pub use errors::{MqlError, Span};
pub use plan::{
    Bounds, CatalogQueryPlan, PlanNode, PlanPredicate, PredicateTarget, date_days, parse_date,
};

/// Parse MQL text into an [`MqlAst`] using the default bounds.
pub fn parse(input: &str) -> Result<MqlAst, MqlError> {
    parse_with_bounds(input, &Bounds::default())
}

/// Parse MQL text into an [`MqlAst`] with explicit bounds.
pub fn parse_with_bounds(input: &str, bounds: &Bounds) -> Result<MqlAst, MqlError> {
    let tokens = lexer::lex(input, bounds.max_input_len, bounds.max_tokens)?;
    parser::Parser::new(tokens, bounds.parse_timeout_ms).parse(bounds)
}

/// Parse and validate MQL into a framework-free [`CatalogQueryPlan`].
pub fn parse_and_plan(input: &str) -> Result<CatalogQueryPlan, MqlError> {
    parse_and_plan_with_bounds(input, &Bounds::default())
}

/// Parse and validate MQL into a [`CatalogQueryPlan`] with explicit bounds.
pub fn parse_and_plan_with_bounds(
    input: &str,
    bounds: &Bounds,
) -> Result<CatalogQueryPlan, MqlError> {
    let ast = parse_with_bounds(input, bounds)?;
    plan::validate(&ast, bounds)
}

/// Produce a stable JSON rendering of the plan for `explain` tooling.
pub fn explain(input: &str) -> Result<String, MqlError> {
    let plan = parse_and_plan(input)?;
    serde_json::to_string_pretty(&plan).map_err(|e| MqlError::Syntax {
        message: format!("serialize plan: {e}"),
        span: (0, 0),
    })
}
