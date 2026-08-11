//! `SearchStore`: one owned Tantivy generation with revision-fenced writes,
//! bounded queries, autocomplete, and name->ID resolution (ADR-0121).

use std::collections::BTreeMap;
use std::path::Path;

use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, Occur, RegexQuery, TermQuery};
use tantivy::schema::{IndexRecordOption, Value};
use tantivy::tokenizer::TextAnalyzer;
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, Order, Term};

use melodee_mql::CatalogQueryPlan;

use crate::documents::{ArtistDoc, ReleaseDoc, TrackDoc};
use crate::error::SearchError;
use crate::schema::{self, SearchSchema, TEXT_TOKENIZER};
use crate::translator;

pub const MAX_SEARCH_RESULTS: usize = 1_000;
pub const MAX_SEARCH_OFFSET: usize = 10_000;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SearchHit {
    pub kind: String,
    pub internal_id: String,
    pub public_id: String,
    pub display: String,
    pub score: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GenerationStats {
    pub artists: usize,
    pub releases: usize,
    pub tracks: usize,
    pub total: usize,
    pub schema_version: u32,
}

pub struct SearchStore {
    schema: SearchSchema,
    writer: Option<IndexWriter>,
    reader: IndexReader,
    tokenizer: TextAnalyzer,
    pending_revisions: BTreeMap<String, i64>,
}

impl SearchStore {
    /// Open an existing generation directory.
    pub fn open(dir: &Path) -> Result<SearchStore, SearchError> {
        let index = Index::open_in_dir(dir).map_err(|e| SearchError::IndexOpen(e.to_string()))?;
        Self::from_index(index, true)
    }

    /// Open an existing generation without acquiring Tantivy's exclusive
    /// writer lock. Query and diagnostic paths must use this entry point so
    /// they remain available while the projection worker is committing.
    pub fn open_readonly(dir: &Path) -> Result<SearchStore, SearchError> {
        let index = Index::open_in_dir(dir).map_err(|e| SearchError::IndexOpen(e.to_string()))?;
        Self::from_index(index, false)
    }

    /// Create a new generation directory (use only for a fresh generation).
    pub fn create(dir: &Path) -> Result<SearchStore, SearchError> {
        let schema = schema::build_schema();
        let index = Index::create_in_dir(dir, schema.schema.clone())
            .map_err(|e| SearchError::IndexOpen(e.to_string()))?;
        Self::from_index(index, true)
    }

    fn from_index(index: Index, writable: bool) -> Result<SearchStore, SearchError> {
        schema::register_tokenizers(&index);
        let schema = schema::build_schema();
        if index.schema() != schema.schema {
            return Err(SearchError::IndexOpen(format!(
                "index schema does not match Melodee schema version {}",
                schema::SCHEMA_VERSION
            )));
        }
        let writer = if writable {
            Some(
                index
                    .writer(50_000_000)
                    .map_err(|e| SearchError::IndexOpen(e.to_string()))?,
            )
        } else {
            None
        };
        let reader = index
            .reader()
            .map_err(|e| SearchError::IndexOpen(e.to_string()))?;
        let tokenizer = index
            .tokenizers()
            .get(TEXT_TOKENIZER)
            .ok_or_else(|| SearchError::IndexOpen("missing tokenizer".into()))?;
        Ok(SearchStore {
            schema,
            writer,
            reader,
            tokenizer,
            pending_revisions: BTreeMap::new(),
        })
    }

    pub fn schema(&self) -> &SearchSchema {
        &self.schema
    }

    fn doc_id_term(&self, kind: &str, internal_id: &str) -> Term {
        Term::from_field_text(self.schema.doc_id, &format!("{kind}:{internal_id}"))
    }

    /// Read the current revision of a document, if present.
    fn existing_revision(&self, kind: &str, internal_id: &str) -> Result<Option<i64>, SearchError> {
        let document_id = format!("{kind}:{internal_id}");
        if let Some(revision) = self.pending_revisions.get(&document_id) {
            return Ok(Some(*revision));
        }
        let searcher = self.reader.searcher();
        let term = self.doc_id_term(kind, internal_id);
        let tq = TermQuery::new(term, IndexRecordOption::Basic);
        let top = searcher
            .search(&tq, &TopDocs::with_limit(1).order_by_score())
            .map_err(SearchError::Tantivy)?;
        let Some((_, address)) = top.into_iter().next() else {
            return Ok(None);
        };
        let document = searcher
            .doc::<tantivy::schema::TantivyDocument>(address)
            .map_err(SearchError::Tantivy)?;
        Ok(document
            .get_first(self.schema.revision)
            .and_then(|value| value.as_i64()))
    }

    fn remove_doc(&mut self, kind: &str, internal_id: &str) -> Result<(), SearchError> {
        let term = self.doc_id_term(kind, internal_id);
        self.writer_mut()?.delete_term(term);
        Ok(())
    }

    fn writer_mut(&mut self) -> Result<&mut IndexWriter, SearchError> {
        self.writer.as_mut().ok_or_else(|| {
            SearchError::IndexOpen("search generation was opened read-only".to_string())
        })
    }

    /// Upsert an Artist document, skipping if the new revision is not strictly
    /// newer than the existing one (revision fencing; A4/A5).
    pub fn upsert_artist(&mut self, doc: &ArtistDoc) -> Result<bool, SearchError> {
        if let Some(existing) = self.existing_revision("artist", &doc.internal_id)? {
            if doc.revision <= existing {
                return Ok(false);
            }
            self.remove_doc("artist", &doc.internal_id)?;
        }
        let document = crate::documents::artist_document(&self.schema, doc);
        self.writer_mut()?
            .add_document(document)
            .map_err(SearchError::Tantivy)?;
        self.pending_revisions
            .insert(format!("artist:{}", doc.internal_id), doc.revision);
        Ok(true)
    }

    pub fn upsert_release(&mut self, doc: &ReleaseDoc) -> Result<bool, SearchError> {
        if let Some(existing) = self.existing_revision("release", &doc.internal_id)? {
            if doc.revision <= existing {
                return Ok(false);
            }
            self.remove_doc("release", &doc.internal_id)?;
        }
        let document = crate::documents::release_document(&self.schema, doc);
        self.writer_mut()?
            .add_document(document)
            .map_err(SearchError::Tantivy)?;
        self.pending_revisions
            .insert(format!("release:{}", doc.internal_id), doc.revision);
        Ok(true)
    }

    pub fn upsert_track(&mut self, doc: &TrackDoc) -> Result<bool, SearchError> {
        if let Some(existing) = self.existing_revision("track", &doc.internal_id)? {
            if doc.revision <= existing {
                return Ok(false);
            }
            self.remove_doc("track", &doc.internal_id)?;
        }
        let document = crate::documents::track_document(&self.schema, doc);
        self.writer_mut()?
            .add_document(document)
            .map_err(SearchError::Tantivy)?;
        self.pending_revisions
            .insert(format!("track:{}", doc.internal_id), doc.revision);
        Ok(true)
    }

    /// Tombstone a document (projection-only removal; A5).
    pub fn delete(
        &mut self,
        kind: &str,
        internal_id: &str,
        revision: i64,
    ) -> Result<bool, SearchError> {
        if !matches!(kind, "artist" | "release" | "track") {
            return Err(SearchError::InvalidQuery(format!(
                "unsupported document kind '{kind}'"
            )));
        }
        if revision <= 0 {
            return Err(SearchError::InvalidQuery(
                "document revision must be positive".to_string(),
            ));
        }
        if self
            .existing_revision(kind, internal_id)?
            .is_some_and(|existing| revision <= existing)
        {
            return Ok(false);
        }
        self.remove_doc(kind, internal_id)?;
        let document =
            crate::documents::tombstone_document(&self.schema, kind, internal_id, revision);
        self.writer_mut()?
            .add_document(document)
            .map_err(SearchError::Tantivy)?;
        self.pending_revisions
            .insert(format!("{kind}:{internal_id}"), revision);
        Ok(true)
    }

    /// Make buffered writes durable and refresh readers.
    pub fn commit(&mut self) -> Result<(), SearchError> {
        self.writer_mut()?.commit().map_err(SearchError::Tantivy)?;
        self.reader.reload().map_err(SearchError::Tantivy)?;
        self.pending_revisions.clear();
        Ok(())
    }

    /// Resolve a value (Artist name or internal/public ID) to candidate Artist
    /// internal IDs from the artist index.
    pub fn resolve_artist_ids(&self, value: &str) -> Result<Vec<String>, SearchError> {
        let mut tok = self.tokenizer.clone();
        let searcher = self.reader.searcher();
        let mut ids = Vec::new();
        // Always try exact internal/public ID term match first (works for real
        // UUIDs and for literal internal IDs passed by callers).
        {
            let q_int = TermQuery::new(
                Term::from_field_text(self.schema.a_internal_id, value),
                IndexRecordOption::Basic,
            );
            let q_pub = TermQuery::new(
                Term::from_field_text(self.schema.a_public_id, value),
                IndexRecordOption::Basic,
            );
            let queries: Vec<Box<dyn tantivy::query::Query>> =
                vec![Box::new(q_int), Box::new(q_pub)];
            for q in &queries {
                let top = searcher
                    .search(&**q, &TopDocs::with_limit(1).order_by_score())
                    .map_err(SearchError::Tantivy)?;
                if let Some((_, address)) = top.into_iter().next() {
                    let document = searcher
                        .doc::<tantivy::schema::TantivyDocument>(address)
                        .map_err(SearchError::Tantivy)?;
                    if let Some(id) = document
                        .get_first(self.schema.a_internal_id)
                        .and_then(|field| field.as_str())
                    {
                        ids.push(id.to_string());
                    }
                }
            }
            if !ids.is_empty() {
                return Ok(ids);
            }
        }
        // Name/alias matches require every analyzed token, so a multi-word
        // observation cannot resolve through only one coincidental token.
        let tokens = translator::analyze(&mut tok, value);
        if tokens.is_empty() {
            return Ok(ids);
        }
        let token_queries = tokens
            .iter()
            .map(|token| {
                let name = Box::new(TermQuery::new(
                    Term::from_field_text(self.schema.a_display_name, token),
                    IndexRecordOption::WithFreqsAndPositions,
                ));
                let alias = Box::new(TermQuery::new(
                    Term::from_field_text(self.schema.a_aliases, token),
                    IndexRecordOption::WithFreqsAndPositions,
                ));
                (
                    Occur::Must,
                    Box::new(BooleanQuery::union(vec![name, alias]))
                        as Box<dyn tantivy::query::Query>,
                )
            })
            .collect();
        let query = BooleanQuery::new(token_queries);
        let top = searcher
            .search(
                &query,
                &TopDocs::with_limit(MAX_SEARCH_RESULTS).order_by_score(),
            )
            .map_err(SearchError::Tantivy)?;
        for (_, address) in top {
            let document = searcher
                .doc::<tantivy::schema::TantivyDocument>(address)
                .map_err(SearchError::Tantivy)?;
            if let Some(id) = document
                .get_first(self.schema.a_internal_id)
                .and_then(|field| field.as_str())
            {
                ids.push(id.to_string());
            }
        }
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// Run a validated plan and return bounded, deterministically ordered hits.
    pub fn search(
        &self,
        plan: &CatalogQueryPlan,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<SearchHit>, SearchError> {
        if !(1..=MAX_SEARCH_RESULTS).contains(&limit) {
            return Err(SearchError::InvalidQuery(format!(
                "result limit must be between 1 and {MAX_SEARCH_RESULTS}"
            )));
        }
        if offset > MAX_SEARCH_OFFSET {
            return Err(SearchError::InvalidQuery(format!(
                "result offset must not exceed {MAX_SEARCH_OFFSET}"
            )));
        }
        let mut tok = self.tokenizer.clone();
        let resolve = |v: &str| self.resolve_artist_ids(v);
        let query = translator::plan_to_query(&self.schema, plan, &mut tok, &resolve)?;
        let searcher = self.reader.searcher();
        let top = searcher
            .search(
                &query,
                &TopDocs::with_limit(limit)
                    .and_offset(offset)
                    .order_by_string_fast_field("doc_id", Order::Asc),
            )
            .map_err(SearchError::Tantivy)?;
        top.into_iter()
            .map(|(_, address)| self.hit(&searcher, address, 0.0))
            .collect::<Result<Vec<_>, _>>()
            .map(|hits| hits.into_iter().flatten().collect())
    }

    fn hit(
        &self,
        searcher: &tantivy::Searcher,
        address: DocAddress,
        score: f32,
    ) -> Result<Option<SearchHit>, SearchError> {
        let document = searcher
            .doc::<tantivy::schema::TantivyDocument>(address)
            .map_err(SearchError::Tantivy)?;
        let Some(kind) = document
            .get_first(self.schema.kind)
            .and_then(|value| value.as_str())
            .map(str::to_string)
        else {
            return Ok(None);
        };
        let field_text = |field: tantivy::schema::Field| {
            document
                .get_first(field)
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string()
        };
        Ok(match kind.as_str() {
            "artist" => Some(SearchHit {
                kind,
                internal_id: field_text(self.schema.a_internal_id),
                public_id: field_text(self.schema.a_public_id),
                display: field_text(self.schema.a_display_name),
                score,
            }),
            "release" => Some(SearchHit {
                kind,
                internal_id: field_text(self.schema.r_internal_id),
                public_id: field_text(self.schema.r_public_id),
                display: field_text(self.schema.r_title),
                score,
            }),
            "track" => Some(SearchHit {
                kind,
                internal_id: field_text(self.schema.t_internal_id),
                public_id: field_text(self.schema.t_public_id),
                display: field_text(self.schema.t_title),
                score,
            }),
            _ => None,
        })
    }

    /// Autocomplete: return distinct display values matching a prefix for a kind.
    pub fn autocomplete(
        &self,
        kind: &str,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<String>, SearchError> {
        if prefix.trim().is_empty() || !(1..=200).contains(&limit) {
            return Err(SearchError::InvalidQuery(
                "autocomplete requires a nonblank prefix and a limit from 1 to 200".to_string(),
            ));
        }
        let mut tok = self.tokenizer.clone();
        let searcher = self.reader.searcher();
        let field = match kind {
            "artist" => self.schema.a_display_name,
            "release" => self.schema.r_title,
            "track" => self.schema.t_title,
            _ => return Ok(Vec::new()),
        };
        let tokens = translator::analyze(&mut tok, prefix);
        let last = tokens.last().cloned().unwrap_or_default();
        if last.is_empty() {
            return Ok(Vec::new());
        }
        let query = RegexQuery::from_pattern(&format!("{}.*", regex_escape_literal(&last)), field)
            .map_err(|error| SearchError::InvalidQuery(format!("invalid prefix: {error}")))?;
        let top = searcher
            .search(&query, &TopDocs::with_limit(limit * 8).order_by_score())
            .map_err(SearchError::Tantivy)?;
        let mut values = Vec::new();
        for (_, addr) in top {
            let document = searcher
                .doc::<tantivy::schema::TantivyDocument>(addr)
                .map_err(SearchError::Tantivy)?;
            if let Some(value) = document.get_first(field).and_then(|field| field.as_str())
                && !values.iter().any(|existing| existing == value)
            {
                values.push(value.to_string());
            }
            if values.len() >= limit {
                break;
            }
        }
        values.sort_by_key(|value| value.to_lowercase());
        values.truncate(limit);
        Ok(values)
    }

    /// Total document counts per kind.
    pub fn stats(&self) -> Result<GenerationStats, SearchError> {
        let searcher = self.reader.searcher();
        let count_kind = |k: &str| -> Result<usize, SearchError> {
            let tq = TermQuery::new(
                Term::from_field_text(self.schema.kind, k),
                IndexRecordOption::Basic,
            );
            searcher.search(&tq, &Count).map_err(SearchError::Tantivy)
        };
        let artists = count_kind("artist")?;
        let releases = count_kind("release")?;
        let tracks = count_kind("track")?;
        Ok(GenerationStats {
            artists,
            releases,
            tracks,
            total: artists + releases + tracks,
            schema_version: schema::SCHEMA_VERSION,
        })
    }
}

fn regex_escape_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}
