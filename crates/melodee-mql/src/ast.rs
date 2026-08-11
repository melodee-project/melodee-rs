//! Pure MQL AST. No Tantivy/SQLx/Axum/filesystem types (ADR-0122).

use serde::Serialize;

use crate::errors::Span;

/// Comparison operators supported by MQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CompareOp {
    Eq,
    NotEq,
    Lt,
    Lte,
    Gt,
    Gte,
}

impl CompareOp {
    pub fn as_str(self) -> &'static str {
        match self {
            CompareOp::Eq => "=",
            CompareOp::NotEq => "!=",
            CompareOp::Lte => "<=",
            CompareOp::Lt => "<",
            CompareOp::Gte => ">=",
            CompareOp::Gt => ">",
        }
    }
}

/// A leaf value. Regex values are delimited by `/.../`; everything else is
/// parsed by the validator into text/number/date as the field requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MqlValue {
    Text(String),
    Regex(String),
    Number(i64),
    /// ISO `YYYY-MM-DD` date value.
    Date(i32, u32, u32),
    /// Inclusive range; a `None` side means open-ended.
    Range(Option<Box<MqlValue>>, Option<Box<MqlValue>>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MqlTerm {
    /// `None` means a bare term targeting the default field expansion.
    pub field: Option<String>,
    pub op: CompareOp,
    pub value: MqlValue,
    pub negate: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum MqlAst {
    Term(Box<MqlTerm>),
    And(Box<MqlAst>, Box<MqlAst>),
    Or(Box<MqlAst>, Box<MqlAst>),
    Not(Box<MqlAst>),
}

impl MqlAst {
    /// Count leaf term nodes (used for the clause/leaf bound).
    pub fn leaf_count(&self) -> usize {
        match self {
            MqlAst::Term(_) => 1,
            MqlAst::Not(inner) => inner.leaf_count(),
            MqlAst::And(a, b) | MqlAst::Or(a, b) => a.leaf_count() + b.leaf_count(),
        }
    }

    /// Maximum nesting depth (used for the AST-depth bound).
    pub fn depth(&self) -> usize {
        match self {
            MqlAst::Term(_) => 1,
            MqlAst::Not(inner) => 1 + inner.depth(),
            MqlAst::And(a, b) | MqlAst::Or(a, b) => 1 + a.depth().max(b.depth()),
        }
    }
}
