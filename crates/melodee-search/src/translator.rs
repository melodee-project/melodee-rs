//! Translate a framework-free [`CatalogQueryPlan`] into a Tantivy query.
//!
//! This is the only place that knows about Tantivy types; everything upstream
//! (the MQL crate) stays framework-free (ADR-0122). Participation targets are
//! ID-backed: names are resolved to Artist internal IDs via `resolve`, then
//! matched against the multi-valued participant ID fields.

use std::ops::Bound;

use melodee_mql::{
    CatalogQueryPlan, CompareOp, MqlValue, PlanNode, PlanPredicate, PredicateTarget, date_days,
};
use tantivy::Term;
use tantivy::query::{
    AllQuery, BooleanQuery, EmptyQuery, Occur, RangeQuery, RegexQuery, TermQuery,
};
use tantivy::schema::{Field, IndexRecordOption};
use tantivy::tokenizer::TextAnalyzer;

use crate::SearchError;
use crate::schema::SearchSchema;

type BoxQuery = Box<dyn tantivy::query::Query>;

fn text_term(field: Field, token: &str) -> BoxQuery {
    let term = Term::from_field_text(field, token);
    Box::new(TermQuery::new(
        term,
        IndexRecordOption::WithFreqsAndPositions,
    ))
}

fn raw_term(field: Field, value: &str) -> BoxQuery {
    let term = Term::from_field_text(field, value);
    Box::new(TermQuery::new(term, IndexRecordOption::Basic))
}

pub fn analyze(tokenizer: &mut TextAnalyzer, value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stream = tokenizer.token_stream(value);
    while let Some(tok) = stream.next() {
        if !tok.text.is_empty() {
            out.push(tok.text.to_string());
        }
    }
    out
}

/// Build a query for a text value on one field: each analyzed token must be
/// present (legacy "contains all words" semantics), or a regex when applicable.
fn text_query(
    field: Field,
    value: &MqlValue,
    tokenizer: &mut TextAnalyzer,
) -> Result<BoxQuery, SearchError> {
    match value {
        MqlValue::Regex(pattern) => RegexQuery::from_pattern(pattern, field)
            .map(|query| Box::new(query) as BoxQuery)
            .map_err(|error| SearchError::InvalidQuery(format!("invalid regex: {error}"))),
        MqlValue::Text(t) => {
            let tokens = analyze(tokenizer, t);
            if tokens.is_empty() {
                return Ok(Box::new(EmptyQuery));
            }
            let sub: Vec<(Occur, BoxQuery)> = tokens
                .into_iter()
                .map(|tok| (Occur::Must, text_term(field, &tok)))
                .collect();
            Ok(Box::new(BooleanQuery::new(sub)))
        }
        other => {
            let s = match other {
                MqlValue::Number(n) => n.to_string(),
                MqlValue::Date(y, m, d) => format!("{y:04}-{m:02}-{d:02}"),
                _ => String::new(),
            };
            if s.is_empty() {
                Ok(Box::new(EmptyQuery))
            } else {
                Ok(text_term(field, &s))
            }
        }
    }
}

/// Constrain a subquery to a single document kind.
fn kind_scope(schema: &SearchSchema, kind: &str, sub: BoxQuery) -> BoxQuery {
    let kind_q = kind_query(schema, kind);
    Box::new(BooleanQuery::new(vec![
        (Occur::Must, kind_q),
        (Occur::Must, sub),
    ]))
}

fn kind_query(schema: &SearchSchema, kind: &str) -> BoxQuery {
    Box::new(TermQuery::new(
        Term::from_field_text(schema.kind, kind),
        IndexRecordOption::Basic,
    ))
}

fn target_scope(schema: &SearchSchema, target: PredicateTarget) -> BoxQuery {
    let kinds: &[&str] = match target {
        PredicateTarget::Name | PredicateTarget::Alias | PredicateTarget::Type => &["artist"],
        PredicateTarget::ReleaseTitle => &["release"],
        PredicateTarget::TrackTitle | PredicateTarget::Duration => &["track"],
        PredicateTarget::Title
        | PredicateTarget::Genre
        | PredicateTarget::Tag
        | PredicateTarget::Year
        | PredicateTarget::Date
        | PredicateTarget::PrimaryArtist
        | PredicateTarget::FeaturedArtist
        | PredicateTarget::Performer
        | PredicateTarget::Other => &["release", "track"],
        PredicateTarget::Status => &["artist", "release"],
        PredicateTarget::Default
        | PredicateTarget::Artist
        | PredicateTarget::Id
        | PredicateTarget::PublicId => &["artist", "release", "track"],
    };
    let queries = kinds
        .iter()
        .map(|kind| kind_query(schema, kind))
        .collect::<Vec<_>>();
    if queries.len() == 1 {
        queries
            .into_iter()
            .next()
            .unwrap_or_else(|| Box::new(EmptyQuery))
    } else {
        Box::new(BooleanQuery::union(queries))
    }
}

fn range_bounds_i64(op: CompareOp, value: &MqlValue) -> (Bound<i64>, Bound<i64>) {
    let v = scalar_i64(value);
    match op {
        CompareOp::Eq | CompareOp::NotEq => (Bound::Included(v), Bound::Included(v)),
        CompareOp::Lt => (Bound::Unbounded, Bound::Excluded(v)),
        CompareOp::Lte => (Bound::Unbounded, Bound::Included(v)),
        CompareOp::Gt => (Bound::Excluded(v), Bound::Unbounded),
        CompareOp::Gte => (Bound::Included(v), Bound::Unbounded),
    }
}

fn range_value_bounds_i64(
    low: &Option<Box<MqlValue>>,
    high: &Option<Box<MqlValue>>,
) -> (Bound<i64>, Bound<i64>) {
    (
        match low {
            Some(v) => Bound::Included(scalar_i64(v)),
            None => Bound::Unbounded,
        },
        match high {
            Some(v) => Bound::Included(scalar_i64(v)),
            None => Bound::Unbounded,
        },
    )
}

fn scalar_i64(value: &MqlValue) -> i64 {
    match value {
        MqlValue::Number(number) => *number,
        MqlValue::Date(year, month, day) => date_days(*year, *month, *day).unwrap_or_default(),
        _ => 0,
    }
}

fn range_query(field: Field, lower: Bound<i64>, upper: Bound<i64>) -> BoxQuery {
    let to_term = |b: Bound<i64>| match b {
        Bound::Included(v) => Bound::Included(Term::from_field_i64(field, v)),
        Bound::Excluded(v) => Bound::Excluded(Term::from_field_i64(field, v)),
        Bound::Unbounded => Bound::Unbounded,
    };
    Box::new(RangeQuery::new(to_term(lower), to_term(upper)))
}

fn date_range_query(field: Field, lower: Bound<i64>, upper: Bound<i64>) -> BoxQuery {
    let to_term = |bound: Bound<i64>| {
        let date = |days: i64| tantivy::DateTime::from_timestamp_secs(days.saturating_mul(86_400));
        match bound {
            Bound::Included(value) => Bound::Included(Term::from_field_date(field, date(value))),
            Bound::Excluded(value) => Bound::Excluded(Term::from_field_date(field, date(value))),
            Bound::Unbounded => Bound::Unbounded,
        }
    };
    Box::new(RangeQuery::new(to_term(lower), to_term(upper)))
}

fn leaf_to_query(
    schema: &SearchSchema,
    pred: &PlanPredicate,
    tokenizer: &mut TextAnalyzer,
    resolve: &impl Fn(&str) -> Result<Vec<String>, SearchError>,
) -> Result<BoxQuery, SearchError> {
    let mut subqueries: Vec<BoxQuery> = Vec::new();

    match pred.target {
        PredicateTarget::Default => {
            subqueries.push(kind_scope(
                schema,
                "artist",
                text_query(schema.a_display_name, &pred.value, tokenizer)?,
            ));
            subqueries.push(kind_scope(
                schema,
                "artist",
                text_query(schema.a_aliases, &pred.value, tokenizer)?,
            ));
            subqueries.push(kind_scope(
                schema,
                "release",
                text_query(schema.r_title, &pred.value, tokenizer)?,
            ));
            subqueries.push(kind_scope(
                schema,
                "release",
                text_query(schema.r_artist_display, &pred.value, tokenizer)?,
            ));
            subqueries.push(kind_scope(
                schema,
                "track",
                text_query(schema.t_title, &pred.value, tokenizer)?,
            ));
        }
        PredicateTarget::Title => {
            subqueries.push(kind_scope(
                schema,
                "release",
                text_query(schema.r_title, &pred.value, tokenizer)?,
            ));
            subqueries.push(kind_scope(
                schema,
                "track",
                text_query(schema.t_title, &pred.value, tokenizer)?,
            ));
        }
        PredicateTarget::Name => {
            subqueries.push(kind_scope(
                schema,
                "artist",
                text_query(schema.a_display_name, &pred.value, tokenizer)?,
            ));
            subqueries.push(kind_scope(
                schema,
                "artist",
                text_query(schema.a_aliases, &pred.value, tokenizer)?,
            ));
        }
        PredicateTarget::ReleaseTitle => {
            subqueries.push(kind_scope(
                schema,
                "release",
                text_query(schema.r_title, &pred.value, tokenizer)?,
            ));
        }
        PredicateTarget::TrackTitle => {
            subqueries.push(kind_scope(
                schema,
                "track",
                text_query(schema.t_title, &pred.value, tokenizer)?,
            ));
        }
        PredicateTarget::Alias => {
            subqueries.push(kind_scope(
                schema,
                "artist",
                text_query(schema.a_aliases, &pred.value, tokenizer)?,
            ));
        }
        PredicateTarget::Id => {
            if let MqlValue::Text(t) = &pred.value {
                subqueries.push(kind_scope(
                    schema,
                    "artist",
                    raw_term(schema.a_internal_id, t),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_internal_id, t),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "track",
                    raw_term(schema.t_internal_id, t),
                ));
            }
        }
        PredicateTarget::PublicId => {
            if let MqlValue::Text(t) = &pred.value {
                subqueries.push(kind_scope(
                    schema,
                    "artist",
                    raw_term(schema.a_public_id, t),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_public_id, t),
                ));
                subqueries.push(kind_scope(schema, "track", raw_term(schema.t_public_id, t)));
            }
        }
        PredicateTarget::Artist => {
            subqueries.push(kind_scope(
                schema,
                "artist",
                text_query(schema.a_display_name, &pred.value, tokenizer)?,
            ));
            subqueries.push(kind_scope(
                schema,
                "artist",
                text_query(schema.a_aliases, &pred.value, tokenizer)?,
            ));
            for id in resolve(&value_as_text(&pred.value))? {
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_artist_ids, &id),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "track",
                    raw_term(schema.t_participant_artist_ids, &id),
                ));
            }
        }
        PredicateTarget::PrimaryArtist => {
            for id in resolve(&value_as_text(&pred.value))? {
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_artist_ids, &id),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "track",
                    raw_term(schema.t_primary_artist_ids, &id),
                ));
            }
        }
        PredicateTarget::FeaturedArtist => {
            for id in resolve(&value_as_text(&pred.value))? {
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_artist_ids, &id),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "track",
                    raw_term(schema.t_featured_artist_ids, &id),
                ));
            }
        }
        PredicateTarget::Performer => {
            for id in resolve(&value_as_text(&pred.value))? {
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_artist_ids, &id),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "track",
                    raw_term(schema.t_performer_artist_ids, &id),
                ));
            }
        }
        PredicateTarget::Other => {
            for id in resolve(&value_as_text(&pred.value))? {
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_artist_ids, &id),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "track",
                    raw_term(schema.t_other_artist_ids, &id),
                ));
            }
        }
        PredicateTarget::Genre | PredicateTarget::Tag => {
            if let MqlValue::Text(t) = &pred.value {
                subqueries.push(kind_scope(
                    schema,
                    "release",
                    raw_term(schema.r_genre_tags, t),
                ));
                subqueries.push(kind_scope(
                    schema,
                    "track",
                    raw_term(schema.t_genre_tags, t),
                ));
            }
        }
        PredicateTarget::Year => {
            let (lo, hi) = match &pred.value {
                MqlValue::Range(l, h) => range_value_bounds_i64(l, h),
                v => range_bounds_i64(pred.op, v),
            };
            subqueries.push(kind_scope(
                schema,
                "release",
                range_query(schema.r_release_year, lo, hi),
            ));
            subqueries.push(kind_scope(
                schema,
                "track",
                range_query(schema.t_release_year, lo, hi),
            ));
        }
        PredicateTarget::Duration => {
            let (lo, hi) = match &pred.value {
                MqlValue::Range(l, h) => range_value_bounds_i64(l, h),
                v => range_bounds_i64(pred.op, v),
            };
            subqueries.push(kind_scope(
                schema,
                "track",
                range_query(schema.t_duration_ms, lo, hi),
            ));
        }
        PredicateTarget::Date => {
            let (lo, hi) = match &pred.value {
                MqlValue::Range(l, h) => range_value_bounds_i64(l, h),
                v => range_bounds_i64(pred.op, v),
            };
            subqueries.push(kind_scope(
                schema,
                "release",
                date_range_query(schema.r_release_date, lo, hi),
            ));
            subqueries.push(kind_scope(
                schema,
                "track",
                date_range_query(schema.t_release_date, lo, hi),
            ));
        }
        PredicateTarget::Type => {
            if let MqlValue::Text(t) = &pred.value {
                subqueries.push(kind_scope(schema, "artist", raw_term(schema.a_type, t)));
            }
        }
        PredicateTarget::Status => {
            if let MqlValue::Text(t) = &pred.value {
                subqueries.push(kind_scope(schema, "artist", raw_term(schema.a_status, t)));
                subqueries.push(kind_scope(schema, "release", raw_term(schema.r_status, t)));
            }
        }
    }

    let combined: BoxQuery = match subqueries.len() {
        0 => Box::new(EmptyQuery),
        1 => subqueries.pop().ok_or_else(|| {
            SearchError::InvalidQuery("query branch disappeared during translation".to_string())
        })?,
        _ => Box::new(BooleanQuery::union(subqueries)),
    };

    if pred.negate || pred.op == CompareOp::NotEq {
        Ok(Box::new(BooleanQuery::new(vec![
            (Occur::Must, target_scope(schema, pred.target)),
            (Occur::MustNot, combined),
        ])))
    } else {
        Ok(combined)
    }
}

fn negate_query(query: BoxQuery) -> BoxQuery {
    Box::new(BooleanQuery::new(vec![
        (Occur::Must, Box::new(AllQuery)),
        (Occur::MustNot, query),
    ]))
}

fn value_as_text(v: &MqlValue) -> String {
    match v {
        MqlValue::Text(t) => t.clone(),
        MqlValue::Regex(t) => t.clone(),
        MqlValue::Number(n) => n.to_string(),
        MqlValue::Date(y, m, d) => format!("{y:04}-{m:02}-{d:02}"),
        MqlValue::Range(_, _) => String::new(),
    }
}

fn node_to_query(
    schema: &SearchSchema,
    node: &PlanNode,
    tokenizer: &mut TextAnalyzer,
    resolve: &impl Fn(&str) -> Result<Vec<String>, SearchError>,
) -> Result<BoxQuery, SearchError> {
    match node {
        PlanNode::Leaf(p) => leaf_to_query(schema, p, tokenizer, resolve),
        PlanNode::Not(inner) => Ok(negate_query(node_to_query(
            schema, inner, tokenizer, resolve,
        )?)),
        PlanNode::And(a, b) => Ok(Box::new(BooleanQuery::new(vec![
            (Occur::Must, node_to_query(schema, a, tokenizer, resolve)?),
            (Occur::Must, node_to_query(schema, b, tokenizer, resolve)?),
        ]))),
        PlanNode::Or(a, b) => Ok(Box::new(BooleanQuery::union(vec![
            node_to_query(schema, a, tokenizer, resolve)?,
            node_to_query(schema, b, tokenizer, resolve)?,
        ]))),
    }
}

/// Translate a plan into a Tantivy query.
pub fn plan_to_query(
    schema: &SearchSchema,
    plan: &CatalogQueryPlan,
    tokenizer: &mut TextAnalyzer,
    resolve: &impl Fn(&str) -> Result<Vec<String>, SearchError>,
) -> Result<BoxQuery, SearchError> {
    node_to_query(schema, &plan.root, tokenizer, resolve)
}
