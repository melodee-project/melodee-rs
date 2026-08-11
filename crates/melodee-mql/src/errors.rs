//! Typed MQL errors with source spans.
//!
//! Every error carries byte-offset spans into the original input so the CLI and
//! API can point users at the exact offending token. No Tantivy/SQLx/Axum
//! types appear in this crate (ADR-0122).

/// Half-open byte-offset span `(start, end)` into the MQL input string.
pub type Span = (usize, usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MqlError {
    /// Input was empty after trimming.
    EmptyInput,
    /// Lexer/parser syntax error.
    Syntax { message: String, span: Span },
    /// A `field:` reference names an unknown field.
    UnknownField { field: String, span: Span },
    /// A role concept (e.g. `primaryArtist`) is unknown.
    UnknownRole { role: String, span: Span },
    /// Operator/value type mismatch (e.g. `<` on a text field).
    TypeMismatch {
        field: String,
        expected: String,
        span: Span,
    },
    /// Inverted range or otherwise invalid range expression.
    RangeError { message: String, span: Span },
    /// Invalid regex (rejected by the regex engine).
    RegexError { message: String, span: Span },
    /// A hard resource bound was exceeded.
    ExceedsBound {
        kind: String,
        limit: usize,
        span: Option<Span>,
    },
}

impl std::fmt::Display for MqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MqlError::EmptyInput => write!(f, "MQL input is empty"),
            MqlError::Syntax { message, span } => {
                write!(f, "syntax error at {span:?}: {message}")
            }
            MqlError::UnknownField { field, span } => {
                write!(f, "unknown field '{field}' at {span:?}")
            }
            MqlError::UnknownRole { role, span } => {
                write!(f, "unknown role '{role}' at {span:?}")
            }
            MqlError::TypeMismatch {
                field,
                expected,
                span,
            } => {
                write!(f, "field '{field}' expects {expected} at {span:?}")
            }
            MqlError::RangeError { message, span } => {
                write!(f, "invalid range at {span:?}: {message}")
            }
            MqlError::RegexError { message, span } => {
                write!(f, "invalid regex at {span:?}: {message}")
            }
            MqlError::ExceedsBound { kind, limit, span } => match span {
                Some(s) => write!(f, "{kind} exceeds limit {limit} at {s:?}"),
                None => write!(f, "{kind} exceeds limit {limit}"),
            },
        }
    }
}

impl std::error::Error for MqlError {}

impl MqlError {
    /// Render a caret line pointing at `span` within `input`, for CLI output.
    pub fn render(input: &str, span: Span) -> String {
        let line = input.to_string();
        let (start, end) = span;
        let end = end.max(start + 1).min(line.len());
        let caret = " ".repeat(start) + &"^".repeat(end.saturating_sub(start));
        format!("{line}\n{caret}")
    }
}
