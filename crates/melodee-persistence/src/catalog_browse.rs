//! read-side operator queries over the catalog schema
//! (ADR-0117/0118): server-side catalog browsing (Artists, Releases, Tracks)
//! and the detail reads backing the admin Artist/Release/Track detail tabs.
//!
//! Every query here is a pure read over existing tables — no new tables, no
//! recomputation of resolver logic. Sub-objects are always bounded
//! (`MAX_CATALOG_DETAIL_ITEMS`) with the full section total carried alongside
//! so the API can report truncation honestly. Identity vocabulary follows the
//! core domain: `Artist`, `Release`, `Track` — never legacy naming.

use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::{CatalogStore, CatalogStoreError};

/// Maximum characters accepted in a catalog browsing `query` filter. The
/// bound keeps ILIKE pattern evaluation cheap on catalog-scale tables.
pub const MAX_CATALOG_BROWSE_QUERY_CHARS: usize = 200;

/// Maximum rows returned for any bounded detail sub-object (aliases,
/// releases, contributions, media assets, history, …). The full section
/// total accompanies every bounded list.
pub const MAX_CATALOG_DETAIL_ITEMS: u32 = 100;

/// Escapes a raw substring filter into a safe ILIKE `contains` pattern.
/// Returns `None` for blank input; `Err` when the input exceeds
/// [`MAX_CATALOG_BROWSE_QUERY_CHARS`].
pub fn contains_pattern(raw: Option<&str>) -> Result<Option<String>, CatalogBrowseError> {
    let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > MAX_CATALOG_BROWSE_QUERY_CHARS {
        return Err(CatalogBrowseError::QueryTooLong);
    }
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('%');
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('%');
    Ok(Some(escaped))
}

/// Catalog browsing filter validation failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CatalogBrowseError {
    #[error("catalog query filter exceeds the {MAX_CATALOG_BROWSE_QUERY_CHARS} character bound")]
    QueryTooLong,
}

/// Sort keys accepted by the Artist browsing listing. Each variant maps to a
/// constant `ORDER BY` fragment over indexed plain columns — the whitelist is
/// the injection boundary, so client input never reaches SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogArtistSort {
    NameAsc,
    NameDesc,
    CreatedAtAsc,
    CreatedAtDesc,
    UpdatedAtAsc,
    UpdatedAtDesc,
}

impl CatalogArtistSort {
    /// Parses a strict `key:asc|desc` sort expression. Keys are camelCase API
    /// names (`name`, `createdAt`, `updatedAt`); anything else is rejected.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (key, direction) = raw.split_once(':')?;
        match (key, direction) {
            ("name", "asc") => Some(Self::NameAsc),
            ("name", "desc") => Some(Self::NameDesc),
            ("createdAt", "asc") => Some(Self::CreatedAtAsc),
            ("createdAt", "desc") => Some(Self::CreatedAtDesc),
            ("updatedAt", "asc") => Some(Self::UpdatedAtAsc),
            ("updatedAt", "desc") => Some(Self::UpdatedAtDesc),
            _ => None,
        }
    }

    /// Constant SQL fragment; every variant ends with the unique `id`
    /// tiebreaker so paging is deterministic.
    const fn order_by(self) -> &'static str {
        match self {
            Self::NameAsc => "a.sort_name ASC, a.id",
            Self::NameDesc => "a.sort_name DESC, a.id",
            Self::CreatedAtAsc => "a.created_at ASC, a.id",
            Self::CreatedAtDesc => "a.created_at DESC, a.id",
            Self::UpdatedAtAsc => "a.updated_at ASC, a.id",
            Self::UpdatedAtDesc => "a.updated_at DESC, a.id",
        }
    }
}

/// Sort keys accepted by the Release browsing listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogReleaseSort {
    TitleAsc,
    TitleDesc,
    ReleaseDateAsc,
    ReleaseDateDesc,
    CreatedAtAsc,
    CreatedAtDesc,
    UpdatedAtAsc,
    UpdatedAtDesc,
}

impl CatalogReleaseSort {
    /// Parses a strict `key:asc|desc` sort expression. Keys are camelCase API
    /// names (`title`, `releaseDate`, `createdAt`, `updatedAt`).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (key, direction) = raw.split_once(':')?;
        match (key, direction) {
            ("title", "asc") => Some(Self::TitleAsc),
            ("title", "desc") => Some(Self::TitleDesc),
            ("releaseDate", "asc") => Some(Self::ReleaseDateAsc),
            ("releaseDate", "desc") => Some(Self::ReleaseDateDesc),
            ("createdAt", "asc") => Some(Self::CreatedAtAsc),
            ("createdAt", "desc") => Some(Self::CreatedAtDesc),
            ("updatedAt", "asc") => Some(Self::UpdatedAtAsc),
            ("updatedAt", "desc") => Some(Self::UpdatedAtDesc),
            _ => None,
        }
    }

    /// Constant SQL fragment with the unique `id` tiebreaker; release dates
    /// sort NULLS LAST so undated Releases never lead a page.
    const fn order_by(self) -> &'static str {
        match self {
            Self::TitleAsc => "r.sort_title ASC, r.id",
            Self::TitleDesc => "r.sort_title DESC, r.id",
            Self::ReleaseDateAsc => "r.release_date ASC NULLS LAST, r.id",
            Self::ReleaseDateDesc => "r.release_date DESC NULLS LAST, r.id",
            Self::CreatedAtAsc => "r.created_at ASC, r.id",
            Self::CreatedAtDesc => "r.created_at DESC, r.id",
            Self::UpdatedAtAsc => "r.updated_at ASC, r.id",
            Self::UpdatedAtDesc => "r.updated_at DESC, r.id",
        }
    }
}

/// Sort keys accepted by the Track browsing listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogTrackSort {
    TitleAsc,
    TitleDesc,
    DurationAsc,
    DurationDesc,
    CreatedAtAsc,
    CreatedAtDesc,
}

impl CatalogTrackSort {
    /// Parses a strict `key:asc|desc` sort expression. Keys are camelCase API
    /// names (`title`, `durationMs`, `createdAt`).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (key, direction) = raw.split_once(':')?;
        match (key, direction) {
            ("title", "asc") => Some(Self::TitleAsc),
            ("title", "desc") => Some(Self::TitleDesc),
            ("durationMs", "asc") => Some(Self::DurationAsc),
            ("durationMs", "desc") => Some(Self::DurationDesc),
            ("createdAt", "asc") => Some(Self::CreatedAtAsc),
            ("createdAt", "desc") => Some(Self::CreatedAtDesc),
            _ => None,
        }
    }

    /// Constant SQL fragment with the unique `id` tiebreaker.
    const fn order_by(self) -> &'static str {
        match self {
            Self::TitleAsc => "t.sort_title ASC, t.id",
            Self::TitleDesc => "t.sort_title DESC, t.id",
            Self::DurationAsc => "t.duration_ms ASC NULLS LAST, t.id",
            Self::DurationDesc => "t.duration_ms DESC NULLS LAST, t.id",
            Self::CreatedAtAsc => "t.created_at ASC, t.id",
            Self::CreatedAtDesc => "t.created_at DESC, t.id",
        }
    }
}

/// One Artist row in the browsing listing.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CatalogArtistListRow {
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub sort_name: String,
    pub artist_type: String,
    pub disambiguation: Option<String>,
    pub status: String,
    pub release_count: i64,
    pub track_count: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// One Release row in the browsing listing.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CatalogReleaseListRow {
    pub public_api_id: Uuid,
    pub title: String,
    pub display_artist: Option<String>,
    pub release_date: Option<Date>,
    pub release_date_text: Option<String>,
    pub status: String,
    pub workflow_state: Option<String>,
    pub policy_disposition: Option<String>,
    pub policy_evidence_summary: Option<String>,
    pub track_count: i64,
    pub duration_ms: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// One Track row in the browsing listing.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CatalogTrackListRow {
    pub public_api_id: Uuid,
    pub title: String,
    pub display_artist: Option<String>,
    pub ordinal: i32,
    pub disc_number: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub release_public_api_id: Uuid,
    pub release_title: String,
    pub created_at: OffsetDateTime,
}

/// A bounded detail sub-object plus the full row count for the section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedSection<T> {
    pub items: Vec<T>,
    pub total: i64,
}

/// Typed Artist alias as recorded by the resolver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistAliasRow {
    pub alias: String,
    pub alias_type: String,
    pub locale: Option<String>,
    pub evidence_source: String,
    pub confidence: i32,
    pub is_locked: bool,
    pub created_at: OffsetDateTime,
}

/// Provider identity durably linked to an Artist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistExternalIdRow {
    pub provider: String,
    pub external_id: String,
    pub confidence: i32,
    pub evidence_source: String,
    pub verified_at: Option<OffsetDateTime>,
    pub is_locked: bool,
}

/// One Release an Artist is billed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistReleaseRow {
    pub public_api_id: Uuid,
    pub title: String,
    pub role: String,
    pub presentation_order: i32,
    pub display_value: String,
    pub release_date: Option<Date>,
    pub track_count: i64,
}

/// One Contribution (performance or non-performance credit) by an Artist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistContributionRow {
    pub role: String,
    pub role_detail: Option<String>,
    pub is_performance: bool,
    pub evidence_source: String,
    pub release_public_api_id: Uuid,
    pub release_title: String,
    pub track_public_api_id: Option<Uuid>,
    pub track_title: Option<String>,
}

/// One recorded Artist-to-Artist relationship (either direction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistRelationshipRow {
    pub source_public_api_id: Uuid,
    pub source_name: String,
    pub target_public_api_id: Uuid,
    pub target_name: String,
    pub relationship_type: String,
    pub role: Option<String>,
    pub start_date: Option<Date>,
    pub end_date: Option<Date>,
    pub confidence: i32,
    pub evidence_source: String,
    pub is_locked: bool,
}

/// One group membership (member-of or has-member, depending on direction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistMembershipRow {
    pub member_public_api_id: Uuid,
    pub member_name: String,
    pub group_public_api_id: Uuid,
    pub group_name: String,
    pub role: String,
    pub start_date: Option<Date>,
    pub end_date: Option<Date>,
    pub confidence: i32,
    pub evidence_source: String,
    pub is_locked: bool,
}

/// One catalog audit event for an entity (append-only evidence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogAuditEventRow {
    pub event_id: Uuid,
    pub event_type: String,
    pub actor: String,
    pub reason: String,
    pub payload: serde_json::Value,
    pub created_at: OffsetDateTime,
}

/// Full operator detail image of one catalog Artist.
#[derive(Debug, Clone)]
pub struct ArtistCatalogDetail {
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub sort_name: String,
    pub artist_type: String,
    pub disambiguation: Option<String>,
    pub status: String,
    pub begin_date: Option<Date>,
    pub end_date: Option<Date>,
    /// Public id of the merge target when `status` is `merged`.
    pub merged_into_public_api_id: Option<Uuid>,
    pub revision: i64,
    pub release_count: i64,
    pub billed_track_count: i64,
    pub performance_track_count: i64,
    pub all_credit_track_count: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub aliases: BoundedSection<ArtistAliasRow>,
    pub external_ids: BoundedSection<ArtistExternalIdRow>,
    pub releases: BoundedSection<ArtistReleaseRow>,
    pub contributions: BoundedSection<ArtistContributionRow>,
    /// Track credits backing the Artist statistics, newest Release first.
    pub track_credits: BoundedSection<ArtistTrackCreditRow>,
    pub relationships: BoundedSection<ArtistRelationshipRow>,
    pub memberships: BoundedSection<ArtistMembershipRow>,
    pub history: BoundedSection<CatalogAuditEventRow>,
}

/// One Track credit for an Artist, sourced from the same
/// `artist_track_associations` rows the Artist statistics aggregate over.
///
/// The statistics ("track credits", "appearances") are counts over these
/// associations, so this section is what makes those counts drillable: an
/// operator can see which Track, on which Release, in which role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistTrackCreditRow {
    pub track_public_api_id: Uuid,
    pub track_title: String,
    pub release_public_api_id: Uuid,
    pub release_title: String,
    /// Billing role from `track_artists` (`primary`, `featured`, …) when the
    /// credit came from tag billing syntax.
    pub role: Option<String>,
    /// True when the credit is a performance credit (the "appearances" count).
    pub is_performance: bool,
    pub association_type: String,
    pub display_value: Option<String>,
    pub join_phrase: Option<String>,
    pub presentation_order: Option<i32>,
}

/// One Track row inside the Release detail, with playable media format facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseTrackRow {
    pub public_api_id: Uuid,
    /// Whether the Track's mandatory playable MediaAsset row resolves. The
    /// schema makes the playable link mandatory, so this is
    /// defensive: a `false` row must never be offered a preview/stream URL.
    pub has_playable: bool,
    pub ordinal: i32,
    pub title: String,
    pub display_artist: Option<String>,
    pub disc_number: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub source_path: Option<String>,
    pub container: Option<String>,
    pub codec: Option<String>,
    pub byte_size: Option<i64>,
    pub mime_type: Option<String>,
}

/// One billed Artist on a Release or Track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BilledArtistRow {
    pub public_api_id: Uuid,
    pub canonical_name: String,
    pub status: String,
    pub role: String,
    pub presentation_order: i32,
    pub display_value: String,
    pub join_phrase: String,
}

/// One recorded Contribution within a Release (or on one Track).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContributionRow {
    pub artist_public_api_id: Uuid,
    pub artist_name: String,
    pub role: String,
    pub role_detail: Option<String>,
    pub is_performance: bool,
    pub evidence_source: String,
    pub display_value: Option<String>,
    pub presentation_order: i32,
    pub track_public_api_id: Option<Uuid>,
    pub track_title: Option<String>,
}

/// One MediaAsset referenced by a Release or Track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAssetRow {
    pub id: Uuid,
    pub canonical_relative_path: String,
    pub byte_size: i64,
    pub mime_type: String,
    pub container: Option<String>,
    pub codec: Option<String>,
    pub sample_rate_hz: Option<i32>,
    pub channels: Option<i32>,
    pub bitrate_bps: Option<i64>,
    pub duration_ms: Option<i64>,
}

/// Immutable catalog import receipt: the provenance linkage from a Release
/// back to its manifest and work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseProvenanceRow {
    pub work_item_id: Uuid,
    pub manifest_id: Uuid,
    pub manifest_schema_version: i32,
    pub semantic_manifest_hash: String,
    pub publication_plan_hash: String,
    pub sealed_manifest_hash: String,
    pub resolver_version: String,
    pub imported_at: OffsetDateTime,
}

/// Full operator detail image of one catalog Release. Classification evidence
/// (traits) is loaded separately through `PolicyStore` and attached
/// by the caller.
#[derive(Debug, Clone)]
pub struct ReleaseCatalogDetail {
    /// Internal identity for further store lookups (e.g. policy
    /// evidence); never exposed on the wire.
    pub internal_id: Uuid,
    pub public_api_id: Uuid,
    pub title: String,
    pub sort_title: String,
    pub display_artist: Option<String>,
    pub release_date: Option<Date>,
    pub release_date_text: Option<String>,
    pub status: String,
    pub workflow_state: Option<String>,
    pub policy_disposition: Option<String>,
    pub policy_evidence_summary: Option<String>,
    pub revision: i64,
    pub track_count: i64,
    pub duration_ms: i64,
    pub media_bytes: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub tracks: BoundedSection<ReleaseTrackRow>,
    pub artists: Vec<BilledArtistRow>,
    pub contributors: BoundedSection<ContributionRow>,
    pub media_assets: BoundedSection<MediaAssetRow>,
    pub provenance: Option<ReleaseProvenanceRow>,
    pub history: BoundedSection<CatalogAuditEventRow>,
}

/// Full operator detail image of one catalog Track.
#[derive(Debug, Clone)]
pub struct TrackCatalogDetail {
    pub public_api_id: Uuid,
    pub title: String,
    pub sort_title: String,
    pub display_artist: Option<String>,
    pub ordinal: i32,
    pub disc_number: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub source_path: Option<String>,
    pub audio_identity_id: Uuid,
    pub revision: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub release_public_api_id: Uuid,
    pub release_title: String,
    pub artists: Vec<BilledArtistRow>,
    pub contributors: BoundedSection<ContributionRow>,
    /// (purpose, asset) pairs; exactly one row carries purpose `playable`.
    pub media_assets: Vec<(String, MediaAssetRow)>,
}

/// Reads the section total carried by every bounded detail row (the window
/// count) and trims to the section bound.
fn bounded<T>(rows: Vec<(T, i64)>) -> BoundedSection<T> {
    let total = rows.first().map_or(0, |(_, total)| *total);
    BoundedSection {
        items: rows.into_iter().map(|(item, _)| item).collect(),
        total,
    }
}

/// Row decoder for bounded sections: pairs each mapped item with the
/// `total_count` window column.
fn section_rows<T>(
    rows: Vec<sqlx::postgres::PgRow>,
    map: impl Fn(&sqlx::postgres::PgRow) -> T,
) -> BoundedSection<T> {
    use sqlx::Row as _;
    let pairs = rows
        .iter()
        .map(|row| {
            let total: i64 = row.get("total_count");
            (map(row), total)
        })
        .collect();
    bounded(pairs)
}

/// Shared ILIKE filter fragments (constants; client values stay bound).
const ARTIST_QUERY_FILTER: &str = "($1::text IS NULL OR a.canonical_name ILIKE $1 ESCAPE '\\'
       OR a.sort_name ILIKE $1 ESCAPE '\\'
       OR EXISTS (SELECT 1 FROM artist_aliases alias
                  WHERE alias.artist_id = a.id
                    AND alias.alias ILIKE $1 ESCAPE '\\'))";
const RELEASE_QUERY_FILTER: &str = "($1::text IS NULL OR r.title ILIKE $1 ESCAPE '\\')";
const RELEASE_ARTIST_FILTER: &str =
    "($2::uuid IS NULL OR EXISTS (SELECT 1 FROM release_artists billing
                                   JOIN artists billed ON billed.id = billing.artist_id
                                   WHERE billing.release_id = r.id
                                     AND billed.public_api_id = $2))";
const TRACK_QUERY_FILTER: &str = "($1::text IS NULL OR t.title ILIKE $1 ESCAPE '\\')";
const TRACK_RELEASE_FILTER: &str = "($2::uuid IS NULL OR r.public_api_id = $2)";

impl CatalogStore {
    /// Lists catalog Artists, paged and whitelist-sorted, with an optional
    /// substring filter over the canonical/sort name and recorded aliases,
    /// plus the total row count for the filter.
    pub async fn list_catalog_artists(
        &self,
        pool: &PgPool,
        search: Option<&str>,
        limit: u32,
        offset: u32,
        sort: CatalogArtistSort,
    ) -> Result<(Vec<CatalogArtistListRow>, i64), CatalogStoreError> {
        let total = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*)::bigint FROM artists a
              WHERE a.status IN ('provisional','verified') AND {ARTIST_QUERY_FILTER}"
        )))
        .bind(search)
        .fetch_one(pool)
        .await?;
        // Injection audit: `order_by` returns a constant `&'static str`
        // fragment selected from the CatalogArtistSort whitelist; only bind
        // parameters carry client-influenced values.
        let rows = sqlx::query_as::<_, CatalogArtistListRow>(sqlx::AssertSqlSafe(format!(
            "SELECT a.public_api_id, a.canonical_name, a.sort_name, a.artist_type,
                    a.disambiguation, a.status,
                    COALESCE(aggregate.release_count, 0) AS release_count,
                    COALESCE(aggregate.all_credit_track_count, 0) AS track_count,
                    a.created_at, a.updated_at
               FROM artists a
               LEFT JOIN artist_catalog_aggregates aggregate ON aggregate.artist_id = a.id
              WHERE a.status IN ('provisional','verified') AND {ARTIST_QUERY_FILTER}
              ORDER BY {}
              LIMIT $2 OFFSET $3",
            sort.order_by()
        )))
        .bind(search)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok((rows, total))
    }

    /// Lists catalog Releases, paged and whitelist-sorted, with an optional
    /// title substring filter and an optional billed-Artist public id filter.
    pub async fn list_catalog_releases(
        &self,
        pool: &PgPool,
        search: Option<&str>,
        artist_public_id: Option<Uuid>,
        limit: u32,
        offset: u32,
        sort: CatalogReleaseSort,
    ) -> Result<(Vec<CatalogReleaseListRow>, i64), CatalogStoreError> {
        let total = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*)::bigint FROM releases r
              WHERE r.status <> 'deleted'
                AND {RELEASE_QUERY_FILTER} AND {RELEASE_ARTIST_FILTER}"
        )))
        .bind(search)
        .bind(artist_public_id)
        .fetch_one(pool)
        .await?;
        let rows = sqlx::query_as::<_, CatalogReleaseListRow>(sqlx::AssertSqlSafe(format!(
            "SELECT r.public_api_id, r.title, r.display_artist, r.release_date,
                    r.release_date_text, r.status,
                    workflow.state AS workflow_state,
                    decision.disposition AS policy_disposition,
                    decision.evidence_summary AS policy_evidence_summary,
                    COALESCE(aggregate.track_count, 0) AS track_count,
                    COALESCE(aggregate.duration_ms, 0) AS duration_ms,
                    r.created_at, r.updated_at
               FROM releases r
               LEFT JOIN release_catalog_aggregates aggregate ON aggregate.release_id = r.id
               LEFT JOIN catalog_imports import ON import.release_id = r.id
               LEFT JOIN release_work_items workflow ON workflow.id = import.work_item_id
               LEFT JOIN policy_decisions decision ON decision.release_id = r.id
              WHERE r.status <> 'deleted'
                AND {RELEASE_QUERY_FILTER} AND {RELEASE_ARTIST_FILTER}
              ORDER BY {}
              LIMIT $3 OFFSET $4",
            sort.order_by()
        )))
        .bind(search)
        .bind(artist_public_id)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok((rows, total))
    }

    /// Lists catalog Tracks, paged and whitelist-sorted, with an optional
    /// title substring filter and an optional Release public id filter.
    pub async fn list_catalog_tracks(
        &self,
        pool: &PgPool,
        search: Option<&str>,
        release_public_id: Option<Uuid>,
        limit: u32,
        offset: u32,
        sort: CatalogTrackSort,
    ) -> Result<(Vec<CatalogTrackListRow>, i64), CatalogStoreError> {
        let total = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*)::bigint FROM tracks t JOIN releases r ON r.id = t.release_id
              WHERE r.status <> 'deleted'
                AND {TRACK_QUERY_FILTER} AND {TRACK_RELEASE_FILTER}"
        )))
        .bind(search)
        .bind(release_public_id)
        .fetch_one(pool)
        .await?;
        let rows = sqlx::query_as::<_, CatalogTrackListRow>(sqlx::AssertSqlSafe(format!(
            "SELECT t.public_api_id, t.title, t.display_artist, t.ordinal,
                    t.disc_number, t.track_number, t.duration_ms,
                    r.public_api_id AS release_public_api_id, r.title AS release_title,
                    t.created_at
               FROM tracks t
               JOIN releases r ON r.id = t.release_id
              WHERE r.status <> 'deleted'
                AND {TRACK_QUERY_FILTER} AND {TRACK_RELEASE_FILTER}
              ORDER BY {}
              LIMIT $3 OFFSET $4",
            sort.order_by()
        )))
        .bind(search)
        .bind(release_public_id)
        .bind(i64::from(limit))
        .bind(i64::from(offset))
        .fetch_all(pool)
        .await?;
        Ok((rows, total))
    }

    /// Loads the full operator detail image of one catalog Artist by public
    /// id: identity facts, catalog aggregate stats, and every bounded
    /// sub-object the Artist detail tabs render. `None` for unknown ids.
    pub async fn artist_catalog_detail(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<ArtistCatalogDetail>, CatalogStoreError> {
        let artist = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                Option<String>,
                String,
                Option<Date>,
                Option<Date>,
                Option<Uuid>,
                i64,
                OffsetDateTime,
                OffsetDateTime,
            ),
        >(
            "SELECT a.id, a.canonical_name, a.sort_name, a.artist_type,
                    a.disambiguation, a.status, a.begin_date, a.end_date,
                    merged.public_api_id AS merged_into_public_api_id,
                    a.revision, a.created_at, a.updated_at
               FROM artists a
               LEFT JOIN artists merged ON merged.id = a.merged_into_artist_id
              WHERE a.public_api_id = $1",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?;
        let Some((
            id,
            canonical_name,
            sort_name,
            artist_type,
            disambiguation,
            status,
            begin_date,
            end_date,
            merged_into_public_api_id,
            revision,
            created_at,
            updated_at,
        )) = artist
        else {
            return Ok(None);
        };

        let stats = sqlx::query_as::<_, (i64, i64, i64, i64)>(
            "SELECT release_count, billed_track_count, performance_track_count,
                    all_credit_track_count
               FROM artist_catalog_aggregates WHERE artist_id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .unwrap_or((0, 0, 0, 0));

        let limit = i64::from(MAX_CATALOG_DETAIL_ITEMS);
        let aliases = section_rows(
            sqlx::query(
                "SELECT alias, alias_type, locale, evidence_source, confidence,
                        is_locked, created_at, COUNT(*) OVER () AS total_count
                   FROM artist_aliases
                  WHERE artist_id = $1
                  ORDER BY created_at, id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ArtistAliasRow {
                    alias: row.get("alias"),
                    alias_type: row.get("alias_type"),
                    locale: row.get("locale"),
                    evidence_source: row.get("evidence_source"),
                    confidence: row.get("confidence"),
                    is_locked: row.get("is_locked"),
                    created_at: row.get("created_at"),
                }
            },
        );
        let external_ids = section_rows(
            sqlx::query(
                "SELECT provider, external_id, confidence, evidence_source,
                        verified_at, is_locked, COUNT(*) OVER () AS total_count
                   FROM artist_external_identities
                  WHERE artist_id = $1
                  ORDER BY provider, external_id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ArtistExternalIdRow {
                    provider: row.get("provider"),
                    external_id: row.get("external_id"),
                    confidence: row.get("confidence"),
                    evidence_source: row.get("evidence_source"),
                    verified_at: row.get("verified_at"),
                    is_locked: row.get("is_locked"),
                }
            },
        );
        let releases = section_rows(
            sqlx::query(
                "SELECT r.public_api_id, r.title, billing.role,
                        billing.presentation_order, billing.display_value,
                        r.release_date,
                        COALESCE(aggregate.track_count, 0) AS track_count,
                        COUNT(*) OVER () AS total_count
                   FROM release_artists billing
                   JOIN releases r ON r.id = billing.release_id
                   LEFT JOIN release_catalog_aggregates aggregate
                          ON aggregate.release_id = r.id
                  WHERE billing.artist_id = $1
                  ORDER BY r.release_date DESC NULLS LAST,
                           billing.presentation_order, billing.id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ArtistReleaseRow {
                    public_api_id: row.get("public_api_id"),
                    title: row.get("title"),
                    role: row.get("role"),
                    presentation_order: row.get("presentation_order"),
                    display_value: row.get("display_value"),
                    release_date: row.get("release_date"),
                    track_count: row.get("track_count"),
                }
            },
        );
        let contributions = section_rows(
            sqlx::query(
                "SELECT c.role, c.role_detail, c.is_performance, c.evidence_source,
                        r.public_api_id AS release_public_api_id, r.title AS release_title,
                        t.public_api_id AS track_public_api_id, t.title AS track_title,
                        COUNT(*) OVER () AS total_count
                   FROM contributions c
                   JOIN releases r ON r.id = c.release_id
                   LEFT JOIN tracks t ON t.id = c.track_id
                  WHERE c.artist_id = $1
                  ORDER BY c.created_at DESC, c.id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ArtistContributionRow {
                    role: row.get("role"),
                    role_detail: row.get("role_detail"),
                    is_performance: row.get("is_performance"),
                    evidence_source: row.get("evidence_source"),
                    release_public_api_id: row.get("release_public_api_id"),
                    release_title: row.get("release_title"),
                    track_public_api_id: row.get("track_public_api_id"),
                    track_title: row.get("track_title"),
                }
            },
        );
        let relationships = section_rows(
            sqlx::query(
                "SELECT source.public_api_id AS source_public_api_id,
                        source.canonical_name AS source_name,
                        target.public_api_id AS target_public_api_id,
                        target.canonical_name AS target_name,
                        relation.relationship_type, relation.role,
                        relation.start_date, relation.end_date,
                        relation.confidence, relation.evidence_source,
                        relation.is_locked,
                        COUNT(*) OVER () AS total_count
                   FROM artist_relationships relation
                   JOIN artists source ON source.id = relation.source_artist_id
                   JOIN artists target ON target.id = relation.target_artist_id
                  WHERE relation.source_artist_id = $1
                     OR relation.target_artist_id = $1
                  ORDER BY relation.relationship_type, relation.created_at, relation.id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ArtistRelationshipRow {
                    source_public_api_id: row.get("source_public_api_id"),
                    source_name: row.get("source_name"),
                    target_public_api_id: row.get("target_public_api_id"),
                    target_name: row.get("target_name"),
                    relationship_type: row.get("relationship_type"),
                    role: row.get("role"),
                    start_date: row.get("start_date"),
                    end_date: row.get("end_date"),
                    confidence: row.get("confidence"),
                    evidence_source: row.get("evidence_source"),
                    is_locked: row.get("is_locked"),
                }
            },
        );
        let memberships = section_rows(
            sqlx::query(
                "SELECT member.public_api_id AS member_public_api_id,
                        member.canonical_name AS member_name,
                        grp.public_api_id AS group_public_api_id,
                        grp.canonical_name AS group_name,
                        membership.role, membership.start_date, membership.end_date,
                        membership.confidence, membership.evidence_source,
                        membership.is_locked,
                        COUNT(*) OVER () AS total_count
                   FROM group_memberships membership
                   JOIN artists member ON member.id = membership.member_artist_id
                   JOIN artists grp ON grp.id = membership.group_artist_id
                  WHERE membership.member_artist_id = $1
                     OR membership.group_artist_id = $1
                  ORDER BY membership.start_date ASC NULLS LAST, membership.id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ArtistMembershipRow {
                    member_public_api_id: row.get("member_public_api_id"),
                    member_name: row.get("member_name"),
                    group_public_api_id: row.get("group_public_api_id"),
                    group_name: row.get("group_name"),
                    role: row.get("role"),
                    start_date: row.get("start_date"),
                    end_date: row.get("end_date"),
                    confidence: row.get("confidence"),
                    evidence_source: row.get("evidence_source"),
                    is_locked: row.get("is_locked"),
                }
            },
        );
        // Drill-down for the statistics: the counts come from
        // `artist_track_associations`, so the rows do too.
        let track_credits = section_rows(
            sqlx::query(
                "SELECT track.public_api_id AS track_public_api_id,
                        track.title AS track_title,
                        release.public_api_id AS release_public_api_id,
                        release.title AS release_title,
                        billing.role, billing.display_value, billing.join_phrase,
                        billing.presentation_order,
                        association.is_performance, association.association_type,
                        COUNT(*) OVER () AS total_count
                   FROM artist_track_associations association
                   JOIN tracks track ON track.id = association.track_id
                   JOIN releases release ON release.id = track.release_id
                   LEFT JOIN track_artists billing
                     ON billing.track_id = association.track_id
                    AND billing.artist_id = association.artist_id
                  WHERE association.artist_id = $1
                  ORDER BY release.title, track.disc_number ASC NULLS FIRST,
                           track.track_number ASC NULLS FIRST, track.ordinal, track.id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ArtistTrackCreditRow {
                    track_public_api_id: row.get("track_public_api_id"),
                    track_title: row.get("track_title"),
                    release_public_api_id: row.get("release_public_api_id"),
                    release_title: row.get("release_title"),
                    role: row.try_get("role").ok(),
                    is_performance: row.get("is_performance"),
                    association_type: row.get("association_type"),
                    display_value: row.try_get("display_value").ok(),
                    join_phrase: row.try_get("join_phrase").ok(),
                    presentation_order: row.try_get("presentation_order").ok(),
                }
            },
        );
        let history = audit_history(pool, "artist", id, limit).await?;

        Ok(Some(ArtistCatalogDetail {
            public_api_id,
            canonical_name,
            sort_name,
            artist_type,
            disambiguation,
            status,
            begin_date,
            end_date,
            merged_into_public_api_id,
            revision,
            release_count: stats.0,
            billed_track_count: stats.1,
            performance_track_count: stats.2,
            all_credit_track_count: stats.3,
            created_at,
            updated_at,
            aliases,
            external_ids,
            releases,
            contributions,
            track_credits,
            relationships,
            memberships,
            history,
        }))
    }

    /// Loads the full operator detail image of one catalog Release by public
    /// id: overview facts, aggregate stats, ordered Tracks with media format
    /// facts, billed Artists, contributors, media assets, provenance, and
    /// audit history. `None` for unknown ids.
    pub async fn release_catalog_detail(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<ReleaseCatalogDetail>, CatalogStoreError> {
        let release = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                Option<String>,
                Option<Date>,
                Option<String>,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
                i64,
                OffsetDateTime,
                OffsetDateTime,
            ),
        >(
            "SELECT release.id, release.title, release.sort_title,
                    release.display_artist, release.release_date,
                    release.release_date_text, release.status,
                    workflow.state AS workflow_state,
                    decision.disposition AS policy_disposition,
                    decision.evidence_summary AS policy_evidence_summary,
                    release.revision, release.created_at, release.updated_at
               FROM releases release
               LEFT JOIN catalog_imports import ON import.release_id = release.id
               LEFT JOIN release_work_items workflow ON workflow.id = import.work_item_id
               LEFT JOIN policy_decisions decision ON decision.release_id = release.id
              WHERE release.public_api_id = $1",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?;
        let Some((
            id,
            title,
            sort_title,
            display_artist,
            release_date,
            release_date_text,
            status,
            workflow_state,
            policy_disposition,
            policy_evidence_summary,
            revision,
            created_at,
            updated_at,
        )) = release
        else {
            return Ok(None);
        };

        let stats = sqlx::query_as::<_, (i64, i64, i64)>(
            "SELECT track_count, duration_ms, media_bytes
               FROM release_catalog_aggregates WHERE release_id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .unwrap_or((0, 0, 0));

        let limit = i64::from(MAX_CATALOG_DETAIL_ITEMS);
        let tracks = section_rows(
            sqlx::query(
                "SELECT t.public_api_id, asset.id IS NOT NULL AS has_playable,
                        t.ordinal, t.title, t.display_artist,
                        t.disc_number, t.track_number, t.duration_ms, t.source_path,
                        asset.container, asset.codec, asset.byte_size, asset.mime_type,
                        COUNT(*) OVER () AS total_count
                   FROM tracks t
                   LEFT JOIN media_assets asset ON asset.id = t.playable_media_asset_id
                  WHERE t.release_id = $1
                  ORDER BY t.ordinal
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            |row| {
                use sqlx::Row as _;
                ReleaseTrackRow {
                    public_api_id: row.get("public_api_id"),
                    has_playable: row.get("has_playable"),
                    ordinal: row.get("ordinal"),
                    title: row.get("title"),
                    display_artist: row.get("display_artist"),
                    disc_number: row.get("disc_number"),
                    track_number: row.get("track_number"),
                    duration_ms: row.get("duration_ms"),
                    source_path: row.get("source_path"),
                    container: row.get("container"),
                    codec: row.get("codec"),
                    byte_size: row.get("byte_size"),
                    mime_type: row.get("mime_type"),
                }
            },
        );
        let artists = sqlx::query(
            "SELECT a.public_api_id, a.canonical_name, a.status, billing.role,
                    billing.presentation_order, billing.display_value,
                    billing.join_phrase
               FROM release_artists billing
               JOIN artists a ON a.id = billing.artist_id
              WHERE billing.release_id = $1
              ORDER BY billing.presentation_order, billing.id",
        )
        .bind(id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(billed_artist_row)
        .collect();
        let contributors = section_rows(
            sqlx::query(
                "SELECT a.public_api_id AS artist_public_api_id,
                        a.canonical_name AS artist_name,
                        c.role, c.role_detail, c.is_performance, c.evidence_source,
                        c.display_value, c.presentation_order,
                        t.public_api_id AS track_public_api_id, t.title AS track_title,
                        COUNT(*) OVER () AS total_count
                   FROM contributions c
                   JOIN artists a ON a.id = c.artist_id
                   LEFT JOIN tracks t ON t.id = c.track_id
                  WHERE c.release_id = $1
                  ORDER BY c.presentation_order, c.created_at, c.id
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            contribution_row,
        );
        // The DISTINCT sub-select deduplicates assets shared across Tracks
        // before the window count, so the section total is the distinct
        // asset count rather than the link count.
        let media_assets = section_rows(
            sqlx::query(
                "SELECT id, canonical_relative_path, byte_size, mime_type,
                        container, codec, sample_rate_hz, channels, bitrate_bps,
                        duration_ms, COUNT(*) OVER () AS total_count
                   FROM (
                       SELECT DISTINCT asset.id, asset.canonical_relative_path,
                              asset.byte_size, asset.mime_type, asset.container,
                              asset.codec, asset.sample_rate_hz, asset.channels,
                              asset.bitrate_bps, asset.duration_ms
                         FROM media_assets asset
                         JOIN track_media_assets link ON link.media_asset_id = asset.id
                         JOIN tracks t ON t.id = link.track_id
                        WHERE t.release_id = $1
                   ) distinct_assets
                  ORDER BY canonical_relative_path
                  LIMIT $2",
            )
            .bind(id)
            .bind(limit)
            .fetch_all(pool)
            .await?,
            media_asset_row,
        );
        let provenance = sqlx::query_as::<
            _,
            (
                Uuid,
                Uuid,
                i32,
                String,
                String,
                String,
                String,
                OffsetDateTime,
            ),
        >(
            "SELECT work_item_id, manifest_id, manifest_schema_version,
                    semantic_manifest_hash, publication_plan_hash,
                    sealed_manifest_hash, resolver_version, imported_at
               FROM catalog_imports WHERE release_id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(
            |(
                work_item_id,
                manifest_id,
                manifest_schema_version,
                semantic_manifest_hash,
                publication_plan_hash,
                sealed_manifest_hash,
                resolver_version,
                imported_at,
            )| {
                ReleaseProvenanceRow {
                    work_item_id,
                    manifest_id,
                    manifest_schema_version,
                    semantic_manifest_hash,
                    publication_plan_hash,
                    sealed_manifest_hash,
                    resolver_version,
                    imported_at,
                }
            },
        );
        let history = audit_history(pool, "release", id, limit).await?;

        Ok(Some(ReleaseCatalogDetail {
            internal_id: id,
            public_api_id,
            title,
            sort_title,
            display_artist,
            release_date,
            release_date_text,
            status,
            workflow_state,
            policy_disposition,
            policy_evidence_summary,
            revision,
            track_count: stats.0,
            duration_ms: stats.1,
            media_bytes: stats.2,
            created_at,
            updated_at,
            tracks,
            artists,
            contributors,
            media_assets,
            provenance,
            history,
        }))
    }

    /// Loads the full operator detail image of one catalog Track by public
    /// id: identity facts, Release linkage, billed Artists, contributors, and
    /// every linked MediaAsset with its purpose. `None` for unknown ids.
    pub async fn track_catalog_detail(
        &self,
        pool: &PgPool,
        public_api_id: Uuid,
    ) -> Result<Option<TrackCatalogDetail>, CatalogStoreError> {
        let track = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                Option<String>,
                i32,
                Option<i32>,
                Option<i32>,
                Option<i64>,
                Option<String>,
                Uuid,
                i64,
                OffsetDateTime,
                OffsetDateTime,
                Uuid,
                String,
            ),
        >(
            "SELECT t.id, t.title, t.sort_title, t.display_artist, t.ordinal,
                    t.disc_number, t.track_number, t.duration_ms, t.source_path,
                    t.audio_identity_id, t.revision, t.created_at, t.updated_at,
                    r.public_api_id AS release_public_api_id, r.title AS release_title
               FROM tracks t
               JOIN releases r ON r.id = t.release_id
              WHERE t.public_api_id = $1",
        )
        .bind(public_api_id)
        .fetch_optional(pool)
        .await?;
        let Some((
            id,
            title,
            sort_title,
            display_artist,
            ordinal,
            disc_number,
            track_number,
            duration_ms,
            source_path,
            audio_identity_id,
            revision,
            created_at,
            updated_at,
            release_public_api_id,
            release_title,
        )) = track
        else {
            return Ok(None);
        };

        let artists = sqlx::query(
            "SELECT a.public_api_id, a.canonical_name, a.status, billing.role,
                    billing.presentation_order, billing.display_value,
                    billing.join_phrase
               FROM track_artists billing
               JOIN artists a ON a.id = billing.artist_id
              WHERE billing.track_id = $1
              ORDER BY billing.presentation_order, billing.id",
        )
        .bind(id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(billed_artist_row)
        .collect();
        let contributors = section_rows(
            sqlx::query(
                "SELECT a.public_api_id AS artist_public_api_id,
                        a.canonical_name AS artist_name,
                        c.role, c.role_detail, c.is_performance, c.evidence_source,
                        c.display_value, c.presentation_order,
                        CAST(NULL AS uuid) AS track_public_api_id,
                        CAST(NULL AS text) AS track_title,
                        COUNT(*) OVER () AS total_count
                   FROM contributions c
                   JOIN artists a ON a.id = c.artist_id
                  WHERE c.track_id = $1
                  ORDER BY c.presentation_order, c.created_at, c.id
                  LIMIT $2",
            )
            .bind(id)
            .bind(i64::from(MAX_CATALOG_DETAIL_ITEMS))
            .fetch_all(pool)
            .await?,
            contribution_row,
        );
        let media_assets = sqlx::query(
            "SELECT link.purpose, asset.id, asset.canonical_relative_path,
                    asset.byte_size, asset.mime_type, asset.container,
                    asset.codec, asset.sample_rate_hz, asset.channels,
                    asset.bitrate_bps, asset.duration_ms
               FROM track_media_assets link
               JOIN media_assets asset ON asset.id = link.media_asset_id
              WHERE link.track_id = $1
              ORDER BY link.ordinal, asset.canonical_relative_path",
        )
        .bind(id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|row| {
            use sqlx::Row as _;
            let purpose: String = row.get("purpose");
            (purpose, media_asset_row(row))
        })
        .collect();

        Ok(Some(TrackCatalogDetail {
            public_api_id,
            title,
            sort_title,
            display_artist,
            ordinal,
            disc_number,
            track_number,
            duration_ms,
            source_path,
            audio_identity_id,
            revision,
            created_at,
            updated_at,
            release_public_api_id,
            release_title,
            artists,
            contributors,
            media_assets,
        }))
    }
}

/// Decodes one billed-Artist join row (shared by Release and Track detail).
fn billed_artist_row(row: &sqlx::postgres::PgRow) -> BilledArtistRow {
    use sqlx::Row as _;
    BilledArtistRow {
        public_api_id: row.get("public_api_id"),
        canonical_name: row.get("canonical_name"),
        status: row.get("status"),
        role: row.get("role"),
        presentation_order: row.get("presentation_order"),
        display_value: row.get("display_value"),
        join_phrase: row.get("join_phrase"),
    }
}

/// Decodes one Contribution join row (shared by Release and Track detail).
fn contribution_row(row: &sqlx::postgres::PgRow) -> ContributionRow {
    use sqlx::Row as _;
    ContributionRow {
        artist_public_api_id: row.get("artist_public_api_id"),
        artist_name: row.get("artist_name"),
        role: row.get("role"),
        role_detail: row.get("role_detail"),
        is_performance: row.get("is_performance"),
        evidence_source: row.get("evidence_source"),
        display_value: row.get("display_value"),
        presentation_order: row.get("presentation_order"),
        track_public_api_id: row.get("track_public_api_id"),
        track_title: row.get("track_title"),
    }
}

/// Decodes one MediaAsset row (shared by Release and Track detail).
fn media_asset_row(row: &sqlx::postgres::PgRow) -> MediaAssetRow {
    use sqlx::Row as _;
    MediaAssetRow {
        id: row.get("id"),
        canonical_relative_path: row.get("canonical_relative_path"),
        byte_size: row.get("byte_size"),
        mime_type: row.get("mime_type"),
        container: row.get("container"),
        codec: row.get("codec"),
        sample_rate_hz: row.get("sample_rate_hz"),
        channels: row.get("channels"),
        bitrate_bps: row.get("bitrate_bps"),
        duration_ms: row.get("duration_ms"),
    }
}

/// Bounded append-only audit history for one catalog entity, newest first.
async fn audit_history(
    pool: &PgPool,
    entity_type: &str,
    entity_id: Uuid,
    limit: i64,
) -> Result<BoundedSection<CatalogAuditEventRow>, CatalogStoreError> {
    Ok(section_rows(
        sqlx::query(
            "SELECT event_id, event_type, actor, reason, payload, created_at,
                    COUNT(*) OVER () AS total_count
               FROM catalog_audit_events
              WHERE entity_type = $1 AND entity_id = $2
              ORDER BY created_at DESC, id DESC
              LIMIT $3",
        )
        .bind(entity_type)
        .bind(entity_id)
        .bind(limit)
        .fetch_all(pool)
        .await?,
        |row| {
            use sqlx::Row as _;
            CatalogAuditEventRow {
                event_id: row.get("event_id"),
                event_type: row.get("event_type"),
                actor: row.get("actor"),
                reason: row.get("reason"),
                payload: row.get("payload"),
                created_at: row.get("created_at"),
            }
        },
    ))
}
