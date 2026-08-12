//! MQL validation and the framework-free `CatalogQueryPlan`.
//!
//! The plan is the boundary contract consumed by the search translator
//! (ADR-0122). It contains no Tantivy/SQLx/Axum/filesystem types.

use serde::Serialize;
use time::{Date, Month};

use crate::ast::{CompareOp, MqlAst, MqlTerm, MqlValue};
use crate::errors::{MqlError, Span};

/// Hard resource bounds. Defaults match ADR-0122.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub max_input_len: usize,
    pub max_tokens: usize,
    pub max_ast_depth: usize,
    pub max_clauses: usize,
    pub max_regex_len: usize,
    pub default_limit: usize,
    pub max_limit: usize,
    pub parse_timeout_ms: u64,
}

impl Default for Bounds {
    fn default() -> Self {
        Bounds {
            max_input_len: 4 * 1024,
            max_tokens: 1024,
            max_ast_depth: 32,
            max_clauses: 256,
            max_regex_len: 256,
            default_limit: 200,
            max_limit: 1000,
            parse_timeout_ms: 100,
        }
    }
}

/// Structured search targets (field/role concepts from ADR-0122).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PredicateTarget {
    Default,
    Artist,
    PrimaryArtist,
    FeaturedArtist,
    Performer,
    Other,
    Alias,
    Title,
    Name,
    ReleaseTitle,
    TrackTitle,
    Genre,
    Tag,
    Year,
    Duration,
    Date,
    Type,
    Status,
    Id,
    PublicId,
}

impl PredicateTarget {
    pub fn from_field(field: &str, span: Span) -> Result<PredicateTarget, MqlError> {
        let f = field.to_ascii_lowercase();
        let t = match f.as_str() {
            "artist" => PredicateTarget::Artist,
            "primaryartist" => PredicateTarget::PrimaryArtist,
            "featuredartist" => PredicateTarget::FeaturedArtist,
            "performer" => PredicateTarget::Performer,
            "other" => PredicateTarget::Other,
            "alias" => PredicateTarget::Alias,
            "title" => PredicateTarget::Title,
            "name" => PredicateTarget::Name,
            "release" | "releasetitle" => PredicateTarget::ReleaseTitle,
            "track" | "tracktitle" => PredicateTarget::TrackTitle,
            "genre" => PredicateTarget::Genre,
            "tag" => PredicateTarget::Tag,
            "year" => PredicateTarget::Year,
            "duration" => PredicateTarget::Duration,
            "date" => PredicateTarget::Date,
            "type" => PredicateTarget::Type,
            "status" => PredicateTarget::Status,
            "id" => PredicateTarget::Id,
            "publicid" => PredicateTarget::PublicId,
            other => {
                return Err(MqlError::UnknownField {
                    field: other.to_string(),
                    span,
                });
            }
        };
        Ok(t)
    }

    /// Whether this target expects a numeric/date value rather than text.
    pub fn is_numeric(self) -> bool {
        matches!(self, PredicateTarget::Year | PredicateTarget::Duration)
    }

    pub fn is_date(self) -> bool {
        self == PredicateTarget::Date
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanPredicate {
    pub target: PredicateTarget,
    pub op: CompareOp,
    pub value: MqlValue,
    pub negate: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PlanNode {
    Leaf(PlanPredicate),
    And(Box<PlanNode>, Box<PlanNode>),
    Or(Box<PlanNode>, Box<PlanNode>),
    Not(Box<PlanNode>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogQueryPlan {
    pub root: PlanNode,
    pub limit: usize,
    pub offset: usize,
}

fn coerce_numeric(v: &MqlValue, field: &str, span: Span) -> Result<i64, MqlError> {
    match v {
        MqlValue::Number(n) => Ok(*n),
        MqlValue::Text(t) => t.trim().parse::<i64>().map_err(|_| MqlError::TypeMismatch {
            field: field.to_string(),
            expected: "a number".to_string(),
            span,
        }),
        _ => Err(MqlError::TypeMismatch {
            field: field.to_string(),
            expected: "a number".to_string(),
            span,
        }),
    }
}

fn coerce_date(v: &MqlValue, field: &str, span: Span) -> Result<MqlValue, MqlError> {
    match v {
        MqlValue::Date(y, m, d) => Ok(MqlValue::Date(*y, *m, *d)),
        MqlValue::Text(t) => {
            let (y, m, d) = parse_date(t).ok_or_else(|| MqlError::TypeMismatch {
                field: field.to_string(),
                expected: "a YYYY-MM-DD date".to_string(),
                span,
            })?;
            Ok(MqlValue::Date(y, m, d))
        }
        _ => Err(MqlError::TypeMismatch {
            field: field.to_string(),
            expected: "a YYYY-MM-DD date".to_string(),
            span,
        }),
    }
}

/// Parse `YYYY-MM-DD` into `(year, month, day)`.
pub fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let mut parts = s.split('-');
    let y = parts.next()?.parse::<i32>().ok()?;
    let m = parts.next()?.parse::<u32>().ok()?;
    let d = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let month = Month::try_from(u8::try_from(m).ok()?).ok()?;
    Date::from_calendar_date(y, month, u8::try_from(d).ok()?).ok()?;
    Some((y, m, d))
}

/// Convert a validated calendar date to whole days since the Unix epoch.
#[must_use]
pub fn date_days(year: i32, month: u32, day: u32) -> Option<i64> {
    const UNIX_EPOCH_JULIAN_DAY: i32 = 2_440_588;
    let month = Month::try_from(u8::try_from(month).ok()?).ok()?;
    let date = Date::from_calendar_date(year, month, u8::try_from(day).ok()?).ok()?;
    Some(i64::from(date.to_julian_day() - UNIX_EPOCH_JULIAN_DAY))
}

fn comparable_value(value: &MqlValue) -> Option<i64> {
    match value {
        MqlValue::Number(number) => Some(*number),
        MqlValue::Date(year, month, day) => date_days(*year, *month, *day),
        _ => None,
    }
}

fn validate_value(
    target: PredicateTarget,
    value: &mut MqlValue,
    field_label: &str,
    span: Span,
    bounds: &Bounds,
) -> Result<(), MqlError> {
    match value {
        MqlValue::Regex(s) => {
            if s.is_empty() {
                return Err(MqlError::RegexError {
                    message: "regex must not be empty".to_string(),
                    span,
                });
            }
            if s.chars().count() > bounds.max_regex_len {
                return Err(MqlError::ExceedsBound {
                    kind: "regex length".to_string(),
                    limit: bounds.max_regex_len,
                    span: Some(span),
                });
            }
        }
        MqlValue::Range(low, high) => {
            if target.is_numeric() {
                if let Some(l) = low {
                    **l = MqlValue::Number(coerce_numeric(l, field_label, span)?);
                }
                if let Some(h) = high {
                    **h = MqlValue::Number(coerce_numeric(h, field_label, span)?);
                }
            } else if target.is_date() {
                if let Some(l) = low {
                    **l = coerce_date(l, field_label, span)?;
                }
                if let Some(h) = high {
                    **h = coerce_date(h, field_label, span)?;
                }
            } else {
                return Err(MqlError::TypeMismatch {
                    field: field_label.to_string(),
                    expected: "numeric or date for range".to_string(),
                    span,
                });
            }
            if low.is_none() && high.is_none() {
                return Err(MqlError::RangeError {
                    message: "both range bounds are open".to_string(),
                    span,
                });
            }
            if let (Some(low), Some(high)) = (low.as_deref(), high.as_deref())
                && comparable_value(low) > comparable_value(high)
            {
                return Err(MqlError::RangeError {
                    message: "lower bound exceeds upper bound".to_string(),
                    span,
                });
            }
        }
        _ => {
            if target.is_numeric() {
                *value = MqlValue::Number(coerce_numeric(value, field_label, span)?);
            } else if target.is_date() {
                *value = coerce_date(value, field_label, span)?;
            }
        }
    }
    Ok(())
}

/// Validate an `MqlAst` into a `CatalogQueryPlan`, enforcing bounds and
/// field/type rules.
pub fn validate(ast: &MqlAst, bounds: &Bounds) -> Result<CatalogQueryPlan, MqlError> {
    let mut leaf_count = 0usize;
    let root = validate_node(ast, bounds, &mut leaf_count)?;
    if leaf_count > bounds.max_clauses {
        return Err(MqlError::ExceedsBound {
            kind: "clause count".to_string(),
            limit: bounds.max_clauses,
            span: None,
        });
    }
    Ok(CatalogQueryPlan {
        root,
        limit: bounds.default_limit,
        offset: 0,
    })
}

fn validate_node(
    ast: &MqlAst,
    bounds: &Bounds,
    leaf_count: &mut usize,
) -> Result<PlanNode, MqlError> {
    match ast {
        MqlAst::Term(t) => Ok(PlanNode::Leaf(validate_term(t, bounds, leaf_count)?)),
        MqlAst::And(a, b) => Ok(PlanNode::And(
            Box::new(validate_node(a, bounds, leaf_count)?),
            Box::new(validate_node(b, bounds, leaf_count)?),
        )),
        MqlAst::Or(a, b) => Ok(PlanNode::Or(
            Box::new(validate_node(a, bounds, leaf_count)?),
            Box::new(validate_node(b, bounds, leaf_count)?),
        )),
        MqlAst::Not(a) => Ok(PlanNode::Not(Box::new(validate_node(
            a, bounds, leaf_count,
        )?))),
    }
}

fn validate_term(
    term: &MqlTerm,
    bounds: &Bounds,
    leaf_count: &mut usize,
) -> Result<PlanPredicate, MqlError> {
    *leaf_count += 1;
    let target = match &term.field {
        None => PredicateTarget::Default,
        Some(f) => PredicateTarget::from_field(f, term.span)?,
    };
    if !target.is_numeric()
        && !target.is_date()
        && !matches!(term.op, CompareOp::Eq | CompareOp::NotEq)
    {
        return Err(MqlError::TypeMismatch {
            field: term.field.clone().unwrap_or_else(|| "default".to_string()),
            expected: "':'/'=' or '!=' for text fields".to_string(),
            span: term.span,
        });
    }
    let mut value = term.value.clone();
    let label = term.field.clone().unwrap_or_else(|| "default".to_string());
    validate_value(target, &mut value, &label, term.span, bounds)?;
    Ok(PlanPredicate {
        target,
        op: term.op,
        value,
        negate: term.negate,
        span: term.span,
    })
}
