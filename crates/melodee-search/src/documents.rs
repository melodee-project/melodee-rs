//! Catalog row -> Tantivy `Document` mapping (ADR-0121).
//!
//! These structs mirror the catalog columns the search index needs. The
//! snapshot/rebuild queries (in `outbox.rs`) populate them; document building is
//! pure and testable without a database.

use tantivy::schema::TantivyDocument;

use crate::schema::SearchSchema;

#[derive(Debug, Clone)]
pub struct ArtistDoc {
    pub internal_id: String,
    pub public_id: String,
    pub display_name: String,
    pub sort_name: String,
    pub aliases: Vec<String>,
    pub disambiguation: Option<String>,
    pub artist_type: String,
    pub status: String,
    pub begin_year: Option<i64>,
    pub end_year: Option<i64>,
    pub revision: i64,
    pub updated_at: time::OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct ReleaseDoc {
    pub internal_id: String,
    pub public_id: String,
    pub title: String,
    pub sort_title: String,
    pub artist_ids: Vec<String>,
    pub artist_display: String,
    pub genre_tags: Vec<String>,
    pub release_year: Option<i64>,
    pub release_date_days: Option<i64>,
    pub release_type: String,
    pub status: String,
    pub revision: i64,
    pub updated_at: time::OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct TrackDoc {
    pub internal_id: String,
    pub public_id: String,
    pub release_id: String,
    pub title: String,
    pub sort_title: String,
    pub primary_artist_ids: Vec<String>,
    pub featured_artist_ids: Vec<String>,
    pub performer_artist_ids: Vec<String>,
    pub other_artist_ids: Vec<String>,
    pub participant_artist_ids: Vec<String>,
    pub display_credits: String,
    pub genre_tags: Vec<String>,
    pub track_number: Option<i64>,
    pub duration_ms: Option<i64>,
    pub release_year: Option<i64>,
    pub release_date_days: Option<i64>,
    pub revision: i64,
    pub updated_at: time::OffsetDateTime,
}

impl From<melodee_persistence::SearchArtistRecord> for ArtistDoc {
    fn from(record: melodee_persistence::SearchArtistRecord) -> Self {
        Self {
            internal_id: record.internal_id.to_string(),
            public_id: record.public_id.to_string(),
            display_name: record.display_name,
            sort_name: record.sort_name,
            aliases: record.aliases,
            disambiguation: record.disambiguation,
            artist_type: record.artist_type,
            status: record.status,
            begin_year: record.begin_year,
            end_year: record.end_year,
            revision: record.revision,
            updated_at: record.updated_at,
        }
    }
}

impl From<melodee_persistence::SearchReleaseRecord> for ReleaseDoc {
    fn from(record: melodee_persistence::SearchReleaseRecord) -> Self {
        Self {
            internal_id: record.internal_id.to_string(),
            public_id: record.public_id.to_string(),
            title: record.title,
            sort_title: record.sort_title,
            artist_ids: record
                .artist_ids
                .into_iter()
                .map(|id| id.to_string())
                .collect(),
            artist_display: record.artist_display,
            genre_tags: record.genre_tags,
            release_year: record.release_year,
            release_date_days: record.release_date_days,
            release_type: record.release_type,
            status: record.status,
            revision: record.revision,
            updated_at: record.updated_at,
        }
    }
}

impl From<melodee_persistence::SearchTrackRecord> for TrackDoc {
    fn from(record: melodee_persistence::SearchTrackRecord) -> Self {
        let convert =
            |values: Vec<uuid::Uuid>| values.into_iter().map(|value| value.to_string()).collect();
        Self {
            internal_id: record.internal_id.to_string(),
            public_id: record.public_id.to_string(),
            release_id: record.release_id.to_string(),
            title: record.title,
            sort_title: record.sort_title,
            primary_artist_ids: convert(record.primary_artist_ids),
            featured_artist_ids: convert(record.featured_artist_ids),
            performer_artist_ids: convert(record.performer_artist_ids),
            other_artist_ids: convert(record.other_artist_ids),
            participant_artist_ids: convert(record.participant_artist_ids),
            display_credits: record.display_credits,
            genre_tags: record.genre_tags,
            track_number: record.track_number,
            duration_ms: record.duration_ms,
            release_year: record.release_year,
            release_date_days: record.release_date_days,
            revision: record.revision,
            updated_at: record.updated_at,
        }
    }
}

fn push_text(doc: &mut TantivyDocument, field: tantivy::schema::Field, values: &[String]) {
    for v in values {
        if !v.is_empty() {
            doc.add_text(field, v);
        }
    }
}

fn add_updated_at(
    document: &mut TantivyDocument,
    schema: &SearchSchema,
    updated_at: time::OffsetDateTime,
) {
    document.add_date(
        schema.updated_at,
        tantivy::DateTime::from_timestamp_secs(updated_at.unix_timestamp()),
    );
}

pub fn artist_document(schema: &SearchSchema, a: &ArtistDoc) -> TantivyDocument {
    let mut doc = TantivyDocument::new();
    doc.add_text(schema.kind, "artist");
    doc.add_text(schema.doc_id, format!("artist:{}", a.internal_id));
    doc.add_i64(schema.revision, a.revision);
    add_updated_at(&mut doc, schema, a.updated_at);
    doc.add_text(schema.a_internal_id, &a.internal_id);
    doc.add_text(schema.a_public_id, &a.public_id);
    doc.add_text(schema.a_display_name, &a.display_name);
    doc.add_text(schema.a_sort_name, &a.sort_name);
    push_text(&mut doc, schema.a_aliases, &a.aliases);
    if let Some(disambiguation) = &a.disambiguation {
        doc.add_text(schema.a_disambiguation, disambiguation);
    }
    doc.add_text(schema.a_type, &a.artist_type);
    doc.add_text(schema.a_status, &a.status);
    if let Some(y) = a.begin_year {
        doc.add_i64(schema.a_begin_year, y);
    }
    if let Some(y) = a.end_year {
        doc.add_i64(schema.a_end_year, y);
    }
    doc.add_i64(schema.a_revision, a.revision);
    doc
}

pub fn release_document(schema: &SearchSchema, r: &ReleaseDoc) -> TantivyDocument {
    let mut doc = TantivyDocument::new();
    doc.add_text(schema.kind, "release");
    doc.add_text(schema.doc_id, format!("release:{}", r.internal_id));
    doc.add_i64(schema.revision, r.revision);
    add_updated_at(&mut doc, schema, r.updated_at);
    doc.add_text(schema.r_internal_id, &r.internal_id);
    doc.add_text(schema.r_public_id, &r.public_id);
    doc.add_text(schema.r_title, &r.title);
    doc.add_text(schema.r_sort_title, &r.sort_title);
    push_text(&mut doc, schema.r_artist_ids, &r.artist_ids);
    doc.add_text(schema.r_artist_display, &r.artist_display);
    push_text(&mut doc, schema.r_genre_tags, &r.genre_tags);
    if let Some(y) = r.release_year {
        doc.add_i64(schema.r_release_year, y);
    }
    if let Some(d) = r.release_date_days {
        doc.add_i64(schema.r_release_date, d);
    }
    doc.add_text(schema.r_type, &r.release_type);
    doc.add_text(schema.r_status, &r.status);
    doc.add_i64(schema.r_revision, r.revision);
    doc
}

pub fn track_document(schema: &SearchSchema, t: &TrackDoc) -> TantivyDocument {
    let mut doc = TantivyDocument::new();
    doc.add_text(schema.kind, "track");
    doc.add_text(schema.doc_id, format!("track:{}", t.internal_id));
    doc.add_i64(schema.revision, t.revision);
    add_updated_at(&mut doc, schema, t.updated_at);
    doc.add_text(schema.t_internal_id, &t.internal_id);
    doc.add_text(schema.t_public_id, &t.public_id);
    doc.add_text(schema.t_release_id, &t.release_id);
    doc.add_text(schema.t_title, &t.title);
    doc.add_text(schema.t_sort_title, &t.sort_title);
    push_text(&mut doc, schema.t_primary_artist_ids, &t.primary_artist_ids);
    push_text(
        &mut doc,
        schema.t_featured_artist_ids,
        &t.featured_artist_ids,
    );
    push_text(
        &mut doc,
        schema.t_performer_artist_ids,
        &t.performer_artist_ids,
    );
    push_text(&mut doc, schema.t_other_artist_ids, &t.other_artist_ids);
    push_text(
        &mut doc,
        schema.t_participant_artist_ids,
        &t.participant_artist_ids,
    );
    doc.add_text(schema.t_display_credits, &t.display_credits);
    push_text(&mut doc, schema.t_genre_tags, &t.genre_tags);
    if let Some(n) = t.track_number {
        doc.add_i64(schema.t_track_number, n);
    }
    if let Some(d) = t.duration_ms {
        doc.add_i64(schema.t_duration_ms, d);
    }
    if let Some(y) = t.release_year {
        doc.add_i64(schema.t_release_year, y);
    }
    if let Some(d) = t.release_date_days {
        doc.add_i64(schema.t_release_date, d);
    }
    doc.add_i64(schema.t_revision, t.revision);
    doc
}

/// A revision-bearing tombstone keeps a stale outbox replay from resurrecting
/// an entity after its searchable document has been removed.
pub fn tombstone_document(
    schema: &SearchSchema,
    kind: &str,
    internal_id: &str,
    revision: i64,
) -> TantivyDocument {
    let mut document = TantivyDocument::new();
    document.add_text(schema.kind, "tombstone");
    document.add_text(schema.doc_id, format!("{kind}:{internal_id}"));
    document.add_i64(schema.revision, revision);
    add_updated_at(&mut document, schema, time::OffsetDateTime::UNIX_EPOCH);
    document
}
