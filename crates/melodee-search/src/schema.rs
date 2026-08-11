//! Tantivy schema for the Melodee search projection (ADR-0121).
//!
//! Three document kinds share one index, distinguished by the `kind` field:
//! `artist`, `release`, `track`. Participation is ID-backed: Track/Release docs
//! carry multi-valued Artist-internal-ID fields so participation queries are
//! immediate and never reconstructed from display strings.

use tantivy::schema::{
    DateOptions, Field, IndexRecordOption, NumericOptions, Schema, TextFieldIndexing, TextOptions,
};
use tantivy::tokenizer::{
    AsciiFoldingFilter, LowerCaser, RemoveLongFilter, SimpleTokenizer, TextAnalyzer,
};

/// Schema version. Bump when field semantics change incompatibly.
pub const SCHEMA_VERSION: u32 = 1;

/// Tokenizer for tokenized text fields (Unicode-aware + lowercase + ASCII fold).
pub const TEXT_TOKENIZER: &str = "melodee";
/// Tokenizer that keeps the whole value as one token (exact ID/term match).
pub const RAW_TOKENIZER: &str = "raw";

#[derive(Clone)]
pub struct SearchSchema {
    pub schema: Schema,
    pub kind: Field,
    pub doc_id: Field,
    pub revision: Field,
    pub updated_at: Field,

    pub a_internal_id: Field,
    pub a_public_id: Field,
    pub a_display_name: Field,
    pub a_sort_name: Field,
    pub a_aliases: Field,
    pub a_disambiguation: Field,
    pub a_type: Field,
    pub a_status: Field,
    pub a_begin_year: Field,
    pub a_end_year: Field,
    pub a_revision: Field,

    pub r_internal_id: Field,
    pub r_public_id: Field,
    pub r_title: Field,
    pub r_sort_title: Field,
    pub r_artist_ids: Field,
    pub r_artist_display: Field,
    pub r_genre_tags: Field,
    pub r_release_year: Field,
    pub r_release_date: Field,
    pub r_type: Field,
    pub r_status: Field,
    pub r_revision: Field,

    pub t_internal_id: Field,
    pub t_public_id: Field,
    pub t_release_id: Field,
    pub t_title: Field,
    pub t_sort_title: Field,
    pub t_primary_artist_ids: Field,
    pub t_featured_artist_ids: Field,
    pub t_performer_artist_ids: Field,
    pub t_other_artist_ids: Field,
    pub t_participant_artist_ids: Field,
    pub t_display_credits: Field,
    pub t_genre_tags: Field,
    pub t_track_number: Field,
    pub t_duration_ms: Field,
    pub t_release_year: Field,
    pub t_release_date: Field,
    pub t_revision: Field,
}

fn text_opts(stored: bool) -> TextOptions {
    let mut opts = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(TEXT_TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );
    if stored {
        opts = opts.set_stored();
    }
    opts
}

fn raw_opts(stored: bool) -> TextOptions {
    let mut opts = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(RAW_TOKENIZER)
            .set_index_option(IndexRecordOption::Basic),
    );
    if stored {
        opts = opts.set_stored();
    }
    opts
}

fn raw_fast_opts(stored: bool) -> TextOptions {
    raw_opts(stored).set_fast(None)
}

fn int_opts(stored: bool) -> NumericOptions {
    let mut opts = NumericOptions::default().set_indexed();
    if stored {
        opts = opts.set_stored();
    }
    opts
}

fn date_opts() -> DateOptions {
    DateOptions::default().set_indexed().set_fast().set_stored()
}

pub fn build_schema() -> SearchSchema {
    let mut builder = Schema::builder();

    let kind = builder.add_text_field("kind", raw_opts(true));
    let doc_id = builder.add_text_field("doc_id", raw_fast_opts(true));
    let revision = builder.add_i64_field("revision", int_opts(true));
    let updated_at = builder.add_date_field("updated_at", date_opts());

    let a_internal_id = builder.add_text_field("a_internal_id", raw_opts(true));
    let a_public_id = builder.add_text_field("a_public_id", raw_opts(true));
    let a_display_name = builder.add_text_field("a_display_name", text_opts(true));
    let a_sort_name = builder.add_text_field("a_sort_name", raw_opts(true));
    let a_aliases = builder.add_text_field("a_aliases", text_opts(true));
    let a_disambiguation = builder.add_text_field("a_disambiguation", text_opts(true));
    let a_type = builder.add_text_field("a_type", raw_opts(true));
    let a_status = builder.add_text_field("a_status", raw_opts(true));
    let a_begin_year = builder.add_i64_field("a_begin_year", int_opts(false));
    let a_end_year = builder.add_i64_field("a_end_year", int_opts(false));
    let a_revision = builder.add_i64_field("a_revision", int_opts(true));

    let r_internal_id = builder.add_text_field("r_internal_id", raw_opts(true));
    let r_public_id = builder.add_text_field("r_public_id", raw_opts(true));
    let r_title = builder.add_text_field("r_title", text_opts(true));
    let r_sort_title = builder.add_text_field("r_sort_title", raw_opts(true));
    let r_artist_ids = builder.add_text_field("r_artist_ids", raw_opts(true));
    let r_artist_display = builder.add_text_field("r_artist_display", text_opts(true));
    let r_genre_tags = builder.add_text_field("r_genre_tags", raw_opts(true));
    let r_release_year = builder.add_i64_field("r_release_year", int_opts(true));
    let r_release_date = builder.add_i64_field("r_release_date", int_opts(true));
    let r_type = builder.add_text_field("r_type", raw_opts(true));
    let r_status = builder.add_text_field("r_status", raw_opts(true));
    let r_revision = builder.add_i64_field("r_revision", int_opts(true));

    let t_internal_id = builder.add_text_field("t_internal_id", raw_opts(true));
    let t_public_id = builder.add_text_field("t_public_id", raw_opts(true));
    let t_release_id = builder.add_text_field("t_release_id", raw_opts(true));
    let t_title = builder.add_text_field("t_title", text_opts(true));
    let t_sort_title = builder.add_text_field("t_sort_title", raw_opts(true));
    let t_primary_artist_ids = builder.add_text_field("t_primary_artist_ids", raw_opts(true));
    let t_featured_artist_ids = builder.add_text_field("t_featured_artist_ids", raw_opts(true));
    let t_performer_artist_ids = builder.add_text_field("t_performer_artist_ids", raw_opts(true));
    let t_other_artist_ids = builder.add_text_field("t_other_artist_ids", raw_opts(true));
    let t_participant_artist_ids =
        builder.add_text_field("t_participant_artist_ids", raw_opts(true));
    let t_display_credits = builder.add_text_field("t_display_credits", text_opts(true));
    let t_genre_tags = builder.add_text_field("t_genre_tags", raw_opts(true));
    let t_track_number = builder.add_i64_field("t_track_number", int_opts(true));
    let t_duration_ms = builder.add_i64_field("t_duration_ms", int_opts(true));
    let t_release_year = builder.add_i64_field("t_release_year", int_opts(true));
    let t_release_date = builder.add_i64_field("t_release_date", int_opts(true));
    let t_revision = builder.add_i64_field("t_revision", int_opts(true));

    let schema = builder.build();

    SearchSchema {
        schema,
        kind,
        doc_id,
        revision,
        updated_at,
        a_internal_id,
        a_public_id,
        a_display_name,
        a_sort_name,
        a_aliases,
        a_disambiguation,
        a_type,
        a_status,
        a_begin_year,
        a_end_year,
        a_revision,
        r_internal_id,
        r_public_id,
        r_title,
        r_sort_title,
        r_artist_ids,
        r_artist_display,
        r_genre_tags,
        r_release_year,
        r_release_date,
        r_type,
        r_status,
        r_revision,
        t_internal_id,
        t_public_id,
        t_release_id,
        t_title,
        t_sort_title,
        t_primary_artist_ids,
        t_featured_artist_ids,
        t_performer_artist_ids,
        t_other_artist_ids,
        t_participant_artist_ids,
        t_display_credits,
        t_genre_tags,
        t_track_number,
        t_duration_ms,
        t_release_year,
        t_release_date,
        t_revision,
    }
}

/// Build the tokenizer used for tokenized text fields and register both
/// tokenizers on the index.
pub fn register_tokenizers(index: &tantivy::Index) {
    let tokenizer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .filter(RemoveLongFilter::limit(64))
        .build();
    let manager = index.tokenizers();
    manager.register(TEXT_TOKENIZER, tokenizer);
    manager.register(RAW_TOKENIZER, tantivy::tokenizer::RawTokenizer::default());
}
