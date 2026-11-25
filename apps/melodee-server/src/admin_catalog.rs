//! catalog browsing + detail API (PRD-API-ADMIN-002,
//! PRD-UI-005/006/010): server-side paginated catalog browsing for Artists,
//! Releases, and Tracks, plus the detail reads backing the admin
//! Artist/Release/Track detail tabs (identity, aliases, stats, releases,
//! appearances, relationships, images, provenance, classification, history).
//!
//! All routes are `AdminPrincipal`-gated reads over the catalog
//! schema. The surface uses Rust-native domain vocabulary (`Artist`,
//! `Release`, `Track`) — never legacy compatibility naming. Public ids are
//! the operator-safe lookup keys; unknown ids return 404. Image references
//! are ids consumable by the existing frozen `/images/{id}/{size}` native
//! endpoint; artwork bytes stay behind that contract.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, Path, Query, State},
    http::{HeaderMap, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
    routing::put,
};
use image::ImageFormat;
use melodee_persistence::{
    ArtistCatalogDetail, ArtistMetadataPatch, BilledArtistRow, BoundedSection, CatalogArtistSort,
    CatalogAuditEventRow, CatalogBrowseError, CatalogImageAudit, CatalogImageEntity,
    CatalogImageError, CatalogImageStore, CatalogReleaseSort, CatalogStore, CatalogStoreError,
    CatalogTrackSort, ContributionRow, MediaAssetRow, PolicyStore, ProviderIdentityCandidateStore,
    ReleaseCatalogDetail, ReleaseExternalIdentityRow, ReleaseMergeEventRow, ReleaseMetadataPatch,
    StoredReleaseClassification, TrackCatalogDetail, TrackMetadataPatch, contains_pattern,
};
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};
use crate::native::NativeStreamUrlSigner;
use crate::native::images::{ImageUploadValidationError, prepare_uploaded_image};
use crate::native_stream::ClientAddressResolver;

/// Maximum rows returned by catalog browsing listings.
const MAX_LIST_LIMIT: u32 = 500;
/// Deep offsets cause PostgreSQL to discard an unbounded prefix. Catalog
/// filters/search remain available for navigation beyond this defensive cap.
const MAX_LIST_OFFSET: u32 = 1_000_000;
/// Exact-count catalog reads are cancel-safe, but still need a request budget
/// so a pathological plan cannot retain a pool connection indefinitely.
const LIST_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// ADR-0143 §1/§3: the upload pipeline bound for catalog artwork overrides
/// (mirrors `MAX_IMAGE_INPUT_BYTES`). The route body limit admits one MiB of
/// slack above it so an oversized-but-parseable body reaches
/// `prepare_uploaded_image` and earns the contract `400 invalid_image`
/// instead of the limiter's bare `413`.
const MAX_ARTWORK_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
const ARTWORK_BODY_LIMIT_BYTES: usize = MAX_ARTWORK_UPLOAD_BYTES + 1024 * 1024;

/// Shared state for the catalog surface: the common admin state (auth
/// gate, pool) plus the read-only stream URL signer used to offer audio
/// previews on cataloged Tracks (PRD-UI-007). The signer is the same
/// cryptographic boundary the native adapter uses; this surface never sees
/// the signing key, and the produced URLs are served by the frozen native
/// `/song/stream/{track}/{user}/{token}` route.
#[derive(Clone)]
pub struct AdminCatalogState {
    pub admin: AdminAccessState,
    pub stream_url_signer: Arc<dyn NativeStreamUrlSigner>,
    /// Resolves the token client binding with the same trusted-proxy rules
    /// as the native stream route, so issued URLs validate for the browser
    /// that received them.
    pub client_addresses: ClientAddressResolver,
}

impl AdminCatalogState {
    #[must_use]
    pub fn new(
        admin: AdminAccessState,
        stream_url_signer: Arc<dyn NativeStreamUrlSigner>,
        client_addresses: ClientAddressResolver,
    ) -> Self {
        Self {
            admin,
            stream_url_signer,
            client_addresses,
        }
    }
}

/// The catalog surface shares the administrator gate; extraction delegates to
/// the canonical `AdminAccessState` implementation so 401/403 behavior is
/// identical on every admin route.
impl FromRequestParts<AdminCatalogState> for AdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminCatalogState,
    ) -> Result<Self, Self::Rejection> {
        <Self as FromRequestParts<AdminAccessState>>::from_request_parts(parts, &state.admin).await
    }
}

/// Administrator or editor identity for catalog list/detail reads. Editors
/// can inspect the catalog and hard-delete entries; all catalog mutations
/// remain administrator-only.
#[derive(Debug, Clone, Copy)]
pub struct CatalogReader {
    pub public_api_id: Uuid,
}

impl FromRequestParts<AdminCatalogState> for CatalogReader {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminCatalogState,
    ) -> Result<Self, Self::Rejection> {
        let user = crate::admin_access::authenticated_operator_user(parts, &state.admin).await?;
        if !user.is_admin && !user.is_editor {
            return Err(catalog_error(
                StatusCode::FORBIDDEN,
                "forbidden",
                "catalog access requires an administrator or editor",
            ));
        }
        Ok(Self {
            public_api_id: user.public_api_id,
        })
    }
}

const fn default_list_limit() -> u32 {
    50
}

fn validate_list_offset(offset: u32) -> Result<(), &'static str> {
    (offset <= MAX_LIST_OFFSET)
        .then_some(())
        .ok_or("offset exceeds the maximum supported catalog window")
}

fn invalid_list_offset(message: &'static str) -> Response {
    catalog_error(StatusCode::BAD_REQUEST, "invalid_offset", message)
}

fn list_query_timeout() -> Response {
    catalog_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "query_timeout",
        "catalog query exceeded its execution budget",
    )
}

/// Stable admin API error envelope (shared conventions).
fn catalog_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            message: message.into(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

fn store_failure(context: &'static str, error: &impl std::fmt::Display) -> Response {
    tracing::error!(%error, context, "admin catalog store failure");
    catalog_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", context)
}

/// Maps a `CatalogStoreError` to the admin API envelope. `InvalidRequest` is a
/// caller-supplied validation failure (400 `invalid_request`); anything else is an
/// unexpected store fault (500 `store_error`).
fn metadata_update_failure(kind: &'static str, error: CatalogStoreError) -> Response {
    match error {
        CatalogStoreError::InvalidRequest => catalog_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("invalid {kind} metadata update payload"),
        ),
        other => {
            tracing::error!(%other, context = kind, "admin catalog metadata update failure");
            catalog_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_error",
                format!("{kind} metadata update failed"),
            )
        }
    }
}

fn not_found(kind: &'static str, id: Uuid) -> Response {
    catalog_error(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("{kind} not found: {id}"),
    )
}

/// Validates and escapes the optional `query` substring filter.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
fn query_pattern(raw: Option<&str>) -> Result<Option<String>, Response> {
    contains_pattern(raw).map_err(|error| match error {
        CatalogBrowseError::QueryTooLong => catalog_error(
            StatusCode::BAD_REQUEST,
            "invalid_query",
            "query exceeds the maximum length",
        ),
    })
}

/// Image id consumable by the existing frozen `/images/{id}/{size}` native
/// endpoint. The `kind` prefix is that endpoint's captured contract
/// (`album`/`artist`/`song`); the admin surface never invents its own.
fn image_id(kind: &'static str, public_api_id: Uuid) -> String {
    format!("{kind}_{public_api_id}")
}

fn header_str<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// Resolves the stream-token client binding with the same trusted-proxy
/// rules the native stream route applies when the URL is fetched, so issued
/// URLs validate for the browser that received them.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
fn client_binding(
    state: &AdminCatalogState,
    peer: SocketAddr,
    headers: &HeaderMap,
) -> Result<String, Response> {
    state
        .client_addresses
        .resolve(
            peer,
            header_str(headers, "forwarded"),
            header_str(headers, "x-forwarded-for"),
        )
        .map(|address| address.to_string())
        .map_err(|_| {
            catalog_error(
                StatusCode::BAD_REQUEST,
                "invalid_client_binding",
                "client address could not be resolved",
            )
        })
}

/// Calendar dates go on the wire as extended ISO `YYYY-MM-DD` strings; the
/// workspace `time` serde default would emit an opaque `[year, ordinal]`
/// array instead.
fn date_string(date: Option<Date>) -> Option<String> {
    date.map(|date| {
        format!(
            "{:04}-{:02}-{:02}",
            date.year(),
            u8::from(date.month()),
            date.day()
        )
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistListParams {
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Whitelisted sort `key:asc|desc`; keys `name`, `createdAt`, `updatedAt`.
    pub sort: Option<String>,
    /// Substring filter over the canonical/sort name and recorded aliases.
    pub query: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseListParams {
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Whitelisted sort `key:asc|desc`; keys `title`, `releaseDate`,
    /// `createdAt`, `updatedAt`.
    pub sort: Option<String>,
    /// Substring filter over the Release title.
    pub query: Option<String>,
    /// Restricts to Releases the given Artist (public id) is billed on.
    pub artist_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackListParams {
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Whitelisted sort `key:asc|desc`; keys `title`, `durationMs`,
    /// `createdAt`.
    pub sort: Option<String>,
    /// Substring filter over the Track title.
    pub query: Option<String>,
    /// Restricts to Tracks of the given Release (public id).
    pub release_id: Option<Uuid>,
}

/// Request body for updating editable Release metadata fields.
///
/// Partial-update semantics: only fields present in the body are written. The
/// `country` and `releaseType` fields from the original W0 draft are intentionally
/// absent — they never existed in any migration and were removed from the
/// RELEASE-002 contract (2026-08-23) in favor of the additive, migration-backed
/// fields that actually exist on the `releases` table.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseMetadataUpdate {
    pub title: Option<String>,
    pub sort_title: Option<String>,
    pub display_artist: Option<String>,
    /// ISO date string "YYYY-MM-DD".
    pub release_date: Option<String>,
    pub release_date_text: Option<String>,
}

/// Request body for updating editable Artist metadata fields.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistMetadataUpdate {
    pub canonical_name: Option<String>,
    pub sort_name: Option<String>,
    pub artist_type: Option<String>,
    /// Operator identity assertion (`active` or `verified`). This is the
    /// only path that promotes an imported active Artist, because import
    /// never promotes and no provider-confirmation path exists yet.
    pub status: Option<String>,
}

/// Request body for updating editable Track metadata fields.
/// `duration_ms` was added by the W0 contract fix (2026-08-23) for the
/// operator-facing Track edit form.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMetadataUpdate {
    pub title: Option<String>,
    pub sort_title: Option<String>,
    pub display_artist: Option<String>,
    pub ordinal: Option<i32>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDto<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// Bounded detail section: up to 100 items plus the full section total.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionDto<T> {
    pub items: Vec<T>,
    pub total: i64,
}

fn section<T, D>(section: BoundedSection<T>, map: impl Fn(T) -> D) -> SectionDto<D> {
    SectionDto {
        items: section.items.into_iter().map(map).collect(),
        total: section.total,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistListItemDto {
    pub id: Uuid,
    pub name: String,
    pub sort_name: String,
    pub artist_type: String,
    pub disambiguation: Option<String>,
    pub status: String,
    pub release_count: i64,
    pub track_count: i64,
    pub image_id: String,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
    #[serde(with = "crate::admin_time")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseListItemDto {
    pub id: Uuid,
    pub title: String,
    pub display_artist: Option<String>,
    pub release_date: Option<String>,
    pub release_date_text: Option<String>,
    pub status: String,
    pub workflow_state: Option<String>,
    pub policy_disposition: Option<String>,
    pub policy_evidence_summary: Option<String>,
    pub track_count: i64,
    pub duration_ms: i64,
    pub image_id: String,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
    #[serde(with = "crate::admin_time")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackListItemDto {
    pub id: Uuid,
    pub title: String,
    pub display_artist: Option<String>,
    pub ordinal: i32,
    pub disc_number: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub release_id: Uuid,
    pub release_title: String,
    pub image_id: String,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistAliasDto {
    pub alias: String,
    pub alias_type: String,
    pub locale: Option<String>,
    pub evidence_source: String,
    pub confidence: i32,
    pub locked: bool,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistExternalIdDto {
    pub provider: String,
    pub external_id: String,
    pub confidence: i32,
    pub evidence_source: String,
    #[serde(with = "crate::admin_time::option")]
    pub verified_at: Option<OffsetDateTime>,
    pub locked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistReleaseDto {
    pub release_id: Uuid,
    pub title: String,
    pub role: String,
    pub presentation_order: i32,
    pub display_value: String,
    pub release_date: Option<String>,
    pub track_count: i64,
}

/// One provider-found identity candidate for an Artist (enrichment evidence).
///
/// Recorded by the enrichment worker and never displayed before; it is evidence
/// an operator uses when deciding whether an unverified (`active`) identity is correct.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistProviderCandidateDto {
    pub provider: String,
    pub external_id: String,
    pub query: String,
    pub candidate_rank: i32,
    /// Basis points (0–10000).
    pub confidence: i32,
    #[serde(with = "crate::admin_time")]
    pub observed_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistTrackCreditDto {
    pub track_id: Uuid,
    pub track_title: String,
    pub release_id: Uuid,
    pub release_title: String,
    pub role: Option<String>,
    pub is_performance: bool,
    pub association_type: String,
    pub display_value: Option<String>,
    pub join_phrase: Option<String>,
    pub presentation_order: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistContributionDto {
    pub role: String,
    pub role_detail: Option<String>,
    pub is_performance: bool,
    pub evidence_source: String,
    pub release_id: Uuid,
    pub release_title: String,
    pub track_id: Option<Uuid>,
    pub track_title: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistRelationshipDto {
    pub source_artist_id: Uuid,
    pub source_name: String,
    pub target_artist_id: Uuid,
    pub target_name: String,
    pub relationship_type: String,
    pub role: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub confidence: i32,
    pub evidence_source: String,
    pub locked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistMembershipDto {
    pub member_artist_id: Uuid,
    pub member_name: String,
    pub group_artist_id: Uuid,
    pub group_name: String,
    pub role: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub confidence: i32,
    pub evidence_source: String,
    pub locked: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAuditEventDto {
    pub event_id: Uuid,
    pub event_type: String,
    pub actor: String,
    pub reason: String,
    pub payload: serde_json::Value,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
}

fn audit_event_dto(row: CatalogAuditEventRow) -> CatalogAuditEventDto {
    CatalogAuditEventDto {
        event_id: row.event_id,
        event_type: row.event_type,
        actor: row.actor,
        reason: row.reason,
        payload: row.payload,
        created_at: row.created_at,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistStatsDto {
    pub release_count: i64,
    pub billed_track_count: i64,
    pub performance_track_count: i64,
    pub all_credit_track_count: i64,
}

/// Full Artist detail: identity facts, typed aliases, external identities,
/// set-based catalog stats, billed Releases, contributions (appearances),
/// relationships, group memberships, image reference, and audit history.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistDetailDto {
    pub id: Uuid,
    pub name: String,
    pub sort_name: String,
    pub artist_type: String,
    pub disambiguation: Option<String>,
    pub status: String,
    pub begin_date: Option<String>,
    pub end_date: Option<String>,
    pub merged_into_artist_id: Option<Uuid>,
    pub revision: i64,
    pub image_id: String,
    #[serde(with = "crate::admin_time::option")]
    pub image_updated_at: Option<OffsetDateTime>,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
    #[serde(with = "crate::admin_time")]
    pub updated_at: OffsetDateTime,
    pub stats: ArtistStatsDto,
    pub aliases: SectionDto<ArtistAliasDto>,
    pub external_ids: SectionDto<ArtistExternalIdDto>,
    pub releases: SectionDto<ArtistReleaseDto>,
    pub contributions: SectionDto<ArtistContributionDto>,
    /// Track credits backing `stats`: which Track, on which Release, in which
    /// role. Additive drill-down for the Artist statistics.
    pub track_credits: SectionDto<ArtistTrackCreditDto>,
    /// Provider identity candidates recorded for this Artist (best first).
    pub provider_candidates: SectionDto<ArtistProviderCandidateDto>,
    pub relationships: SectionDto<ArtistRelationshipDto>,
    pub memberships: SectionDto<ArtistMembershipDto>,
    pub history: SectionDto<CatalogAuditEventDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseTrackDto {
    pub id: Uuid,
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
    /// Signed audio preview URL (PRD-UI-007) on the frozen native
    /// `/song/stream/…` route; `null` when the Track has no playable asset
    /// or the signer fails closed for the acting administrator.
    pub stream_url: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BilledArtistDto {
    pub artist_id: Uuid,
    pub name: String,
    pub status: String,
    pub role: String,
    pub presentation_order: i32,
    pub display_value: String,
    pub join_phrase: String,
}

fn billed_artist_dto(row: BilledArtistRow) -> BilledArtistDto {
    BilledArtistDto {
        artist_id: row.public_api_id,
        name: row.canonical_name,
        status: row.status,
        role: row.role,
        presentation_order: row.presentation_order,
        display_value: row.display_value,
        join_phrase: row.join_phrase,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContributionDto {
    pub artist_id: Uuid,
    pub artist_name: String,
    pub role: String,
    pub role_detail: Option<String>,
    pub is_performance: bool,
    pub evidence_source: String,
    pub display_value: Option<String>,
    pub presentation_order: i32,
    pub track_id: Option<Uuid>,
    pub track_title: Option<String>,
}

fn contribution_dto(row: ContributionRow) -> ContributionDto {
    ContributionDto {
        artist_id: row.artist_public_api_id,
        artist_name: row.artist_name,
        role: row.role,
        role_detail: row.role_detail,
        is_performance: row.is_performance,
        evidence_source: row.evidence_source,
        display_value: row.display_value,
        presentation_order: row.presentation_order,
        track_id: row.track_public_api_id,
        track_title: row.track_title,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaAssetDto {
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

fn media_asset_dto(row: MediaAssetRow) -> MediaAssetDto {
    MediaAssetDto {
        id: row.id,
        canonical_relative_path: row.canonical_relative_path,
        byte_size: row.byte_size,
        mime_type: row.mime_type,
        container: row.container,
        codec: row.codec,
        sample_rate_hz: row.sample_rate_hz,
        channels: row.channels,
        bitrate_bps: row.bitrate_bps,
        duration_ms: row.duration_ms,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMediaAssetDto {
    pub purpose: String,
    #[serde(flatten)]
    pub asset: MediaAssetDto,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProvenanceDto {
    pub work_item_id: Uuid,
    pub manifest_id: Uuid,
    pub manifest_schema_version: i32,
    pub semantic_manifest_hash: String,
    pub publication_plan_hash: String,
    pub sealed_manifest_hash: String,
    pub resolver_version: String,
    #[serde(with = "crate::admin_time")]
    pub imported_at: OffsetDateTime,
}

/// Persisted classification (traits + evidence) for a Release.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseClassificationDto {
    pub traits: Vec<String>,
    pub confidence: u16,
    #[serde(with = "crate::admin_time")]
    pub classified_at: OffsetDateTime,
    pub classifier_version: String,
}

fn classification_dto(stored: StoredReleaseClassification) -> ReleaseClassificationDto {
    ReleaseClassificationDto {
        traits: stored
            .traits
            .iter()
            .map(|t| t.as_str().to_string())
            .collect(),
        confidence: stored.confidence.basis_points(),
        classified_at: stored.classified_at,
        classifier_version: stored.classifier_version,
    }
}

/// Full Release detail: overview facts, ordered Tracks with media format,
/// billed Artists, contributors, media files, classification, provenance,
/// image reference, and audit history.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseDetailDto {
    pub id: Uuid,
    pub title: String,
    pub sort_title: String,
    pub display_artist: Option<String>,
    pub release_date: Option<String>,
    pub release_date_text: Option<String>,
    pub status: String,
    pub workflow_state: Option<String>,
    pub policy_disposition: Option<String>,
    pub policy_evidence_summary: Option<String>,
    pub revision: i64,
    pub track_count: i64,
    pub duration_ms: i64,
    pub media_bytes: i64,
    pub image_id: String,
    #[serde(with = "crate::admin_time::option")]
    pub image_updated_at: Option<OffsetDateTime>,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
    #[serde(with = "crate::admin_time")]
    pub updated_at: OffsetDateTime,
    pub tracks: SectionDto<ReleaseTrackDto>,
    pub artists: Vec<BilledArtistDto>,
    pub contributors: SectionDto<ContributionDto>,
    pub media_assets: SectionDto<MediaAssetDto>,
    pub classification: Option<ReleaseClassificationDto>,
    pub provenance: Option<ReleaseProvenanceDto>,
    /// Durable provider/tag evidence. `trusted` rows are the input to
    /// automatic Release convergence; every row is evidence, not identity.
    pub external_identities: SectionDto<ReleaseExternalIdentityDto>,
    /// Audited merge/split history in which this Release participated.
    pub merge_events: SectionDto<ReleaseMergeEventDto>,
    pub history: SectionDto<CatalogAuditEventDto>,
    /// Surviving Release when this Release is a merge tombstone.
    pub merged_into_release_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseExternalIdentityDto {
    pub provider: String,
    pub external_id: String,
    pub trusted: bool,
    pub confidence: i32,
    pub evidence_source: String,
    pub is_locked: bool,
    #[serde(with = "crate::admin_time")]
    pub observed_at: OffsetDateTime,
}

impl From<ReleaseExternalIdentityRow> for ReleaseExternalIdentityDto {
    fn from(row: ReleaseExternalIdentityRow) -> Self {
        Self {
            provider: row.provider,
            external_id: row.external_id,
            trusted: row.trusted,
            confidence: row.confidence,
            evidence_source: row.evidence_source,
            is_locked: row.is_locked,
            observed_at: row.observed_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseMergeEventDto {
    pub id: Uuid,
    pub source_release_id: Uuid,
    pub target_release_id: Uuid,
    pub actor: String,
    pub reason: String,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
}

impl From<ReleaseMergeEventRow> for ReleaseMergeEventDto {
    fn from(row: ReleaseMergeEventRow) -> Self {
        Self {
            id: row.id,
            source_release_id: row.source_release_public_id,
            target_release_id: row.target_release_public_id,
            actor: row.actor,
            reason: row.reason,
            created_at: row.created_at,
        }
    }
}

/// Full Track detail: identity facts, Release linkage, billed Artists,
/// contributors, and linked media assets.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackDetailDto {
    pub id: Uuid,
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
    pub image_id: String,
    /// Signed audio preview URL (PRD-UI-007) on the frozen native
    /// `/song/stream/…` route; `null` when the Track has no playable asset
    /// or the signer fails closed for the acting administrator.
    pub stream_url: Option<String>,
    #[serde(with = "crate::admin_time")]
    pub created_at: OffsetDateTime,
    #[serde(with = "crate::admin_time")]
    pub updated_at: OffsetDateTime,
    pub release_id: Uuid,
    pub release_title: String,
    pub artists: Vec<BilledArtistDto>,
    pub contributors: SectionDto<ContributionDto>,
    pub media_assets: Vec<TrackMediaAssetDto>,
}

pub fn router() -> Router<AdminCatalogState> {
    Router::new()
        .route("/api/admin/v1/catalog/artists", get(list_artists))
        .route("/api/admin/v1/catalog/releases", get(list_releases))
        .route("/api/admin/v1/catalog/tracks", get(list_tracks))
        .route("/api/admin/v1/catalog/artists/{id}", get(artist_detail))
        .route("/api/admin/v1/catalog/releases/{id}", get(release_detail))
        .route("/api/admin/v1/catalog/tracks/{id}", get(track_detail))
        .route(
            "/api/admin/v1/catalog/artists/{id}/image",
            put(upload_artist_image)
                .delete(delete_artist_image)
                .layer(DefaultBodyLimit::max(ARTWORK_BODY_LIMIT_BYTES)),
        )
        .route(
            "/api/admin/v1/catalog/releases/{id}/image",
            put(upload_release_image)
                .delete(delete_release_image)
                .layer(DefaultBodyLimit::max(ARTWORK_BODY_LIMIT_BYTES)),
        )
        .route(
            "/api/admin/v1/catalog/releases/{id}/metadata",
            put(update_release_metadata),
        )
        .route(
            "/api/admin/v1/catalog/artists/{id}/metadata",
            put(update_artist_metadata),
        )
        .route(
            "/api/admin/v1/catalog/tracks/{id}/metadata",
            put(update_track_metadata),
        )
}

/// GET /api/admin/v1/catalog/artists — server-side Artist browsing.
async fn list_artists(
    State(state): State<AdminCatalogState>,
    _reader: CatalogReader,
    Query(params): Query<ArtistListParams>,
) -> Response {
    if let Err(message) = validate_list_offset(params.offset) {
        return invalid_list_offset(message);
    }
    let sort = match params.sort.as_deref() {
        None => CatalogArtistSort::NameAsc,
        Some(raw) => match CatalogArtistSort::parse(raw) {
            Some(sort) => sort,
            None => {
                return catalog_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_sort",
                    format!("unknown sort '{raw}'"),
                );
            }
        },
    };
    let search = match query_pattern(params.query.as_deref()) {
        Ok(search) => search,
        Err(response) => return response,
    };
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    match tokio::time::timeout(
        LIST_QUERY_TIMEOUT,
        CatalogStore.list_catalog_artists(
            &state.admin.pool,
            search.as_deref(),
            limit,
            offset,
            sort,
        ),
    )
    .await
    {
        Ok(Ok((items, total))) => Json(PageDto {
            items: items
                .into_iter()
                .map(|row| ArtistListItemDto {
                    id: row.public_api_id,
                    image_id: image_id("artist", row.public_api_id),
                    name: row.canonical_name,
                    sort_name: row.sort_name,
                    artist_type: row.artist_type,
                    disambiguation: row.disambiguation,
                    status: row.status,
                    release_count: row.release_count,
                    track_count: row.track_count,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                })
                .collect(),
            total,
            limit,
            offset,
        })
        .into_response(),
        Ok(Err(error)) => store_failure("artist listing failed", &error),
        Err(_) => list_query_timeout(),
    }
}

/// GET /api/admin/v1/catalog/releases — server-side Release browsing.
async fn list_releases(
    State(state): State<AdminCatalogState>,
    _reader: CatalogReader,
    Query(params): Query<ReleaseListParams>,
) -> Response {
    if let Err(message) = validate_list_offset(params.offset) {
        return invalid_list_offset(message);
    }
    let sort = match params.sort.as_deref() {
        None => CatalogReleaseSort::TitleAsc,
        Some(raw) => match CatalogReleaseSort::parse(raw) {
            Some(sort) => sort,
            None => {
                return catalog_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_sort",
                    format!("unknown sort '{raw}'"),
                );
            }
        },
    };
    let search = match query_pattern(params.query.as_deref()) {
        Ok(search) => search,
        Err(response) => return response,
    };
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    match tokio::time::timeout(
        LIST_QUERY_TIMEOUT,
        CatalogStore.list_catalog_releases(
            &state.admin.pool,
            search.as_deref(),
            params.artist_id,
            limit,
            offset,
            sort,
        ),
    )
    .await
    {
        Ok(Ok((items, total))) => Json(PageDto {
            items: items
                .into_iter()
                .map(|row| ReleaseListItemDto {
                    id: row.public_api_id,
                    image_id: image_id("album", row.public_api_id),
                    title: row.title,
                    display_artist: row.display_artist,
                    release_date: date_string(row.release_date),
                    release_date_text: row.release_date_text,
                    status: row.status,
                    workflow_state: row.workflow_state,
                    policy_disposition: row.policy_disposition,
                    policy_evidence_summary: row.policy_evidence_summary,
                    track_count: row.track_count,
                    duration_ms: row.duration_ms,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                })
                .collect(),
            total,
            limit,
            offset,
        })
        .into_response(),
        Ok(Err(error)) => store_failure("release listing failed", &error),
        Err(_) => list_query_timeout(),
    }
}

/// GET /api/admin/v1/catalog/tracks — server-side Track browsing.
async fn list_tracks(
    State(state): State<AdminCatalogState>,
    _reader: CatalogReader,
    Query(params): Query<TrackListParams>,
) -> Response {
    if let Err(message) = validate_list_offset(params.offset) {
        return invalid_list_offset(message);
    }
    let sort = match params.sort.as_deref() {
        None => CatalogTrackSort::TitleAsc,
        Some(raw) => match CatalogTrackSort::parse(raw) {
            Some(sort) => sort,
            None => {
                return catalog_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_sort",
                    format!("unknown sort '{raw}'"),
                );
            }
        },
    };
    let search = match query_pattern(params.query.as_deref()) {
        Ok(search) => search,
        Err(response) => return response,
    };
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    match tokio::time::timeout(
        LIST_QUERY_TIMEOUT,
        CatalogStore.list_catalog_tracks(
            &state.admin.pool,
            search.as_deref(),
            params.release_id,
            limit,
            offset,
            sort,
        ),
    )
    .await
    {
        Ok(Ok((items, total))) => Json(PageDto {
            items: items
                .into_iter()
                .map(|row| TrackListItemDto {
                    id: row.public_api_id,
                    image_id: image_id("song", row.public_api_id),
                    title: row.title,
                    display_artist: row.display_artist,
                    ordinal: row.ordinal,
                    disc_number: row.disc_number,
                    track_number: row.track_number,
                    duration_ms: row.duration_ms,
                    release_id: row.release_public_api_id,
                    release_title: row.release_title,
                    created_at: row.created_at,
                })
                .collect(),
            total,
            limit,
            offset,
        })
        .into_response(),
        Ok(Err(error)) => store_failure("track listing failed", &error),
        Err(_) => list_query_timeout(),
    }
}

/// GET /api/admin/v1/catalog/artists/{id} — Artist detail tabs read.
async fn artist_detail(
    State(state): State<AdminCatalogState>,
    _reader: CatalogReader,
    Path(id): Path<Uuid>,
) -> Response {
    let detail = match CatalogStore
        .artist_catalog_detail(&state.admin.pool, id)
        .await
    {
        Ok(Some(detail)) => detail,
        Ok(None) => return not_found("artist", id),
        Err(error) => return store_failure("artist detail read failed", &error),
    };
    // Provider identity candidates are enrichment evidence: recorded by the
    // enrichment worker, previously invisible to operators. A failure here must
    // not hide the Artist, so it degrades to an empty section.
    let provider_candidates = match ProviderIdentityCandidateStore
        .list_artist_candidates(&state.admin.pool, id, PROVIDER_CANDIDATE_LIMIT)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(error = ?error, %id, "provider identity candidates unavailable");
            Vec::new()
        }
    };
    Json(artist_detail_dto(detail, provider_candidates)).into_response()
}

/// Bounded provider-candidate section size for one Artist detail read. A
/// provider search returns at most 10 ranked results per query, so 100 keeps
/// the complete candidate set available for grouping while staying bounded.
const PROVIDER_CANDIDATE_LIMIT: u32 = 100;

fn artist_detail_dto(
    detail: ArtistCatalogDetail,
    provider_candidates: Vec<melodee_persistence::ProviderIdentityCandidate>,
) -> ArtistDetailDto {
    ArtistDetailDto {
        id: detail.public_api_id,
        image_updated_at: detail.image_updated_at,
        image_id: image_id("artist", detail.public_api_id),
        name: detail.canonical_name,
        sort_name: detail.sort_name,
        artist_type: detail.artist_type,
        disambiguation: detail.disambiguation,
        status: detail.status,
        begin_date: date_string(detail.begin_date),
        end_date: date_string(detail.end_date),
        merged_into_artist_id: detail.merged_into_public_api_id,
        revision: detail.revision,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        stats: ArtistStatsDto {
            release_count: detail.release_count,
            billed_track_count: detail.billed_track_count,
            performance_track_count: detail.performance_track_count,
            all_credit_track_count: detail.all_credit_track_count,
        },
        aliases: section(detail.aliases, |row| ArtistAliasDto {
            alias: row.alias,
            alias_type: row.alias_type,
            locale: row.locale,
            evidence_source: row.evidence_source,
            confidence: row.confidence,
            locked: row.is_locked,
            created_at: row.created_at,
        }),
        external_ids: section(detail.external_ids, |row| ArtistExternalIdDto {
            provider: row.provider,
            external_id: row.external_id,
            confidence: row.confidence,
            evidence_source: row.evidence_source,
            verified_at: row.verified_at,
            locked: row.is_locked,
        }),
        releases: section(detail.releases, |row| ArtistReleaseDto {
            release_id: row.public_api_id,
            title: row.title,
            role: row.role,
            presentation_order: row.presentation_order,
            display_value: row.display_value,
            release_date: date_string(row.release_date),
            track_count: row.track_count,
        }),
        contributions: section(detail.contributions, |row| ArtistContributionDto {
            role: row.role,
            role_detail: row.role_detail,
            is_performance: row.is_performance,
            evidence_source: row.evidence_source,
            release_id: row.release_public_api_id,
            release_title: row.release_title,
            track_id: row.track_public_api_id,
            track_title: row.track_title,
        }),
        provider_candidates: SectionDto {
            total: i64::try_from(provider_candidates.len()).unwrap_or(i64::MAX),
            items: provider_candidates
                .into_iter()
                .map(|candidate| ArtistProviderCandidateDto {
                    provider: candidate.provider,
                    external_id: candidate.external_id,
                    query: candidate.query,
                    candidate_rank: candidate.candidate_rank,
                    confidence: candidate.confidence,
                    observed_at: candidate.observed_at,
                })
                .collect(),
        },
        track_credits: section(detail.track_credits, |row| ArtistTrackCreditDto {
            track_id: row.track_public_api_id,
            track_title: row.track_title,
            release_id: row.release_public_api_id,
            release_title: row.release_title,
            role: row.role,
            is_performance: row.is_performance,
            association_type: row.association_type,
            display_value: row.display_value,
            join_phrase: row.join_phrase,
            presentation_order: row.presentation_order,
        }),
        relationships: section(detail.relationships, |row| ArtistRelationshipDto {
            source_artist_id: row.source_public_api_id,
            source_name: row.source_name,
            target_artist_id: row.target_public_api_id,
            target_name: row.target_name,
            relationship_type: row.relationship_type,
            role: row.role,
            start_date: date_string(row.start_date),
            end_date: date_string(row.end_date),
            confidence: row.confidence,
            evidence_source: row.evidence_source,
            locked: row.is_locked,
        }),
        memberships: section(detail.memberships, |row| ArtistMembershipDto {
            member_artist_id: row.member_public_api_id,
            member_name: row.member_name,
            group_artist_id: row.group_public_api_id,
            group_name: row.group_name,
            role: row.role,
            start_date: date_string(row.start_date),
            end_date: date_string(row.end_date),
            confidence: row.confidence,
            evidence_source: row.evidence_source,
            locked: row.is_locked,
        }),
        history: section(detail.history, audit_event_dto),
    }
}

/// GET /api/admin/v1/catalog/releases/{id} — Release detail tabs read. Track
/// rows carry a signed audio preview URL when the Track's playable asset
/// resolves and the acting administrator is stream-authorized (PRD-UI-007).
async fn release_detail(
    State(state): State<AdminCatalogState>,
    reader: CatalogReader,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let binding = match client_binding(&state, peer, &headers) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let detail = match CatalogStore
        .release_catalog_detail(&state.admin.pool, id)
        .await
    {
        Ok(Some(detail)) => detail,
        Ok(None) => return not_found("release", id),
        Err(error) => return store_failure("release detail read failed", &error),
    };
    // Classification is policy evidence keyed by the internal
    // Release id carried by the detail read.
    let classification = match PolicyStore
        .load_classification(
            &state.admin.pool,
            melodee_domain::ReleaseId::from_uuid(detail.internal_id),
        )
        .await
    {
        Ok(classification) => classification.map(classification_dto),
        Err(error) => return store_failure("classification read failed", &error),
    };
    // The production signer resolves the principal and accessible Track set
    // in two queries, preserving a nullable result per Track. This retains
    // the previous fail-closed section behavior without a serial 3N query
    // pattern at the 100-row section bound.
    let playable_ids = detail
        .tracks
        .items
        .iter()
        .filter(|track| track.has_playable)
        .map(|track| track.public_api_id)
        .collect::<Vec<_>>();
    let signed = state
        .stream_url_signer
        .stream_urls_partial(playable_ids.clone(), reader.public_api_id, &binding)
        .await
        .ok()
        .filter(|urls| urls.len() == playable_ids.len())
        .unwrap_or_else(|| vec![None; playable_ids.len()]);
    let mut signed = signed.into_iter();
    let stream_urls = detail
        .tracks
        .items
        .iter()
        .map(|track| {
            if track.has_playable {
                signed.next().flatten()
            } else {
                None
            }
        })
        .collect();
    Json(release_detail_dto(detail, classification, stream_urls)).into_response()
}

fn release_detail_dto(
    detail: ReleaseCatalogDetail,
    classification: Option<ReleaseClassificationDto>,
    stream_urls: Vec<Option<String>>,
) -> ReleaseDetailDto {
    ReleaseDetailDto {
        id: detail.public_api_id,
        image_updated_at: detail.image_updated_at,
        image_id: image_id("album", detail.public_api_id),
        title: detail.title,
        sort_title: detail.sort_title,
        display_artist: detail.display_artist,
        release_date: date_string(detail.release_date),
        release_date_text: detail.release_date_text,
        status: detail.status,
        workflow_state: detail.workflow_state,
        policy_disposition: detail.policy_disposition,
        policy_evidence_summary: detail.policy_evidence_summary,
        revision: detail.revision,
        track_count: detail.track_count,
        duration_ms: detail.duration_ms,
        media_bytes: detail.media_bytes,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        tracks: {
            let section_rows = detail.tracks;
            let total = section_rows.total;
            let items = section_rows
                .items
                .into_iter()
                .zip(stream_urls)
                .map(|(row, stream_url)| ReleaseTrackDto {
                    id: row.public_api_id,
                    ordinal: row.ordinal,
                    title: row.title,
                    display_artist: row.display_artist,
                    disc_number: row.disc_number,
                    track_number: row.track_number,
                    duration_ms: row.duration_ms,
                    source_path: row.source_path,
                    container: row.container,
                    codec: row.codec,
                    byte_size: row.byte_size,
                    mime_type: row.mime_type,
                    stream_url,
                })
                .collect();
            SectionDto { items, total }
        },
        artists: detail.artists.into_iter().map(billed_artist_dto).collect(),
        contributors: section(detail.contributors, contribution_dto),
        media_assets: section(detail.media_assets, media_asset_dto),
        classification,
        provenance: detail.provenance.map(|row| ReleaseProvenanceDto {
            work_item_id: row.work_item_id,
            manifest_id: row.manifest_id,
            manifest_schema_version: row.manifest_schema_version,
            semantic_manifest_hash: row.semantic_manifest_hash,
            publication_plan_hash: row.publication_plan_hash,
            sealed_manifest_hash: row.sealed_manifest_hash,
            resolver_version: row.resolver_version,
            imported_at: row.imported_at,
        }),
        history: section(detail.history, audit_event_dto),
        external_identities: section(detail.external_identities, ReleaseExternalIdentityDto::from),
        merge_events: section(detail.merge_events, ReleaseMergeEventDto::from),
        merged_into_release_id: detail.merged_into_public_id,
    }
}

/// GET /api/admin/v1/catalog/tracks/{id} — Track detail read. Carries a
/// signed audio preview URL when the Track's playable asset exists and the
/// acting administrator is stream-authorized (PRD-UI-007).
async fn track_detail(
    State(state): State<AdminCatalogState>,
    reader: CatalogReader,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let binding = match client_binding(&state, peer, &headers) {
        Ok(binding) => binding,
        Err(response) => return response,
    };
    let detail = match CatalogStore
        .track_catalog_detail(&state.admin.pool, id)
        .await
    {
        Ok(Some(detail)) => detail,
        Ok(None) => return not_found("track", id),
        Err(error) => return store_failure("track detail read failed", &error),
    };
    // The schema guarantees exactly one `playable` link; the check
    // stays defensive so a broken read never produces a preview affordance.
    let has_playable = detail
        .media_assets
        .iter()
        .any(|(purpose, _)| purpose == "playable");
    let stream_url = if has_playable {
        state
            .stream_url_signer
            .stream_urls_partial(vec![detail.public_api_id], reader.public_api_id, &binding)
            .await
            .ok()
            .and_then(|mut urls| (urls.len() == 1).then(|| urls.pop().flatten()))
            .flatten()
    } else {
        None
    };
    Json(track_detail_dto(detail, stream_url)).into_response()
}

fn track_detail_dto(detail: TrackCatalogDetail, stream_url: Option<String>) -> TrackDetailDto {
    TrackDetailDto {
        id: detail.public_api_id,
        image_id: image_id("song", detail.public_api_id),
        title: detail.title,
        sort_title: detail.sort_title,
        display_artist: detail.display_artist,
        ordinal: detail.ordinal,
        disc_number: detail.disc_number,
        track_number: detail.track_number,
        duration_ms: detail.duration_ms,
        source_path: detail.source_path,
        audio_identity_id: detail.audio_identity_id,
        revision: detail.revision,
        stream_url,
        created_at: detail.created_at,
        updated_at: detail.updated_at,
        release_id: detail.release_public_api_id,
        release_title: detail.release_title,
        artists: detail.artists.into_iter().map(billed_artist_dto).collect(),
        contributors: section(detail.contributors, contribution_dto),
        media_assets: detail
            .media_assets
            .into_iter()
            .map(|(purpose, asset)| TrackMediaAssetDto {
                purpose,
                asset: media_asset_dto(asset),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// ADR-0143 catalog artwork overrides (parity matrix artwork row)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageMutationDto {
    pub message: &'static str,
    pub image_id: String,
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDeleteRequest {
    /// Operator reason recorded in the audit event; optional because the
    /// deletion only ever removes the override row (ADR-0143 §4).
    pub reason: Option<String>,
}

/// Detects the image family from magic bytes; the declared Content-Type is
/// never trusted (same rule as the native avatar/playlist uploads).
fn artwork_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    match image::guess_format(bytes).ok()? {
        format @ (ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP) => Some(format),
        _ => None,
    }
}

fn invalid_image(message: &'static str) -> Response {
    catalog_error(StatusCode::BAD_REQUEST, "invalid_image", message)
}

/// Resolves the internal entity id for the override row; unknown public ids
/// earn the shared `404 not_found` on every artwork route (ADR-0143 §4).
async fn resolve_image_entity(
    state: &AdminCatalogState,
    entity: CatalogImageEntity,
    public_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    let query = match entity {
        CatalogImageEntity::Artist => "SELECT id FROM artists WHERE public_api_id = $1",
        CatalogImageEntity::Release => "SELECT id FROM releases WHERE public_api_id = $1",
    };
    sqlx::query_scalar(query)
        .bind(public_id)
        .fetch_optional(&state.admin.pool)
        .await
}

/// Per-entity artwork route constants.
#[derive(Clone, Copy)]
struct ArtworkRoute {
    entity: CatalogImageEntity,
    /// Error-message vocabulary (`artist` / `release`).
    kind: &'static str,
    /// The frozen `/images/{id}/{size}` prefix for this entity (`artist` /
    /// `album`) — the admin surface never invents its own.
    image_kind: &'static str,
}

const ARTIST_ARTWORK: ArtworkRoute = ArtworkRoute {
    entity: CatalogImageEntity::Artist,
    kind: "artist",
    image_kind: "artist",
};
const RELEASE_ARTWORK: ArtworkRoute = ArtworkRoute {
    entity: CatalogImageEntity::Release,
    kind: "release",
    image_kind: "album",
};

async fn upload_entity_image(
    state: &AdminCatalogState,
    admin: &AdminPrincipal,
    route: ArtworkRoute,
    public_id: Uuid,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let internal_id = match resolve_image_entity(state, route.entity, public_id).await {
        Ok(Some(id)) => id,
        Ok(None) => return not_found(route.kind, public_id),
        Err(error) => return store_failure("image entity resolution failed", &error),
    };
    let Some(format) = artwork_image_format(&body) else {
        return invalid_image("image body must be JPEG, PNG, or WebP");
    };
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        });
    if declared.as_deref() != Some(format.to_mime_type()) {
        return invalid_image("declared content type does not match the image bytes");
    }
    let convert_to = (format == ImageFormat::WebP).then_some(ImageFormat::Jpeg);
    let bytes = match prepare_uploaded_image(body.to_vec(), convert_to).await {
        Ok(bytes) => bytes,
        Err(ImageUploadValidationError::Invalid) => {
            return invalid_image("image exceeds the compatibility bounds");
        }
        Err(ImageUploadValidationError::Busy) => {
            return catalog_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "image_pipeline_busy",
                "image processing is temporarily saturated",
            );
        }
    };
    let now = OffsetDateTime::now_utc();
    let actor = admin.public_api_id.to_string();
    let stored = match CatalogImageStore::set_image(
        &state.admin.pool,
        route.entity,
        internal_id,
        convert_to.unwrap_or(format).to_mime_type(),
        &bytes,
        CatalogImageAudit {
            actor: &actor,
            reason: "operator artwork upload",
            now,
        },
    )
    .await
    {
        Ok(stored) => stored,
        Err(CatalogImageError::InvalidRequest(_)) => {
            return invalid_image("image violates the storage bounds");
        }
        Err(error) => return store_failure("image override write failed", &error),
    };
    tracing::info!(
        entity_type = route.entity.as_str(),
        entity = %public_id,
        admin = %actor,
        "catalog artwork override stored"
    );
    Json(ImageMutationDto {
        message: "Artwork updated",
        image_id: image_id(route.image_kind, public_id),
        updated_at: stored.updated_at.format(&Rfc3339).ok(),
    })
    .into_response()
}

async fn delete_entity_image(
    state: &AdminCatalogState,
    admin: &AdminPrincipal,
    route: ArtworkRoute,
    public_id: Uuid,
    body: Bytes,
) -> Response {
    let internal_id = match resolve_image_entity(state, route.entity, public_id).await {
        Ok(Some(id)) => id,
        Ok(None) => return not_found(route.kind, public_id),
        Err(error) => return store_failure("image entity resolution failed", &error),
    };
    // The reason body is optional JSON; absence/blank falls back to the
    // default so the idempotent contract never hinges on a request body.
    let reason = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<ImageDeleteRequest>(&body) {
            Ok(request) => request.reason,
            Err(_) => {
                return catalog_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "delete body must be JSON with an optional reason",
                );
            }
        }
    };
    let reason = reason
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty())
        .unwrap_or_else(|| "operator artwork removal".to_string());
    let actor = admin.public_api_id.to_string();
    let removed = match CatalogImageStore::delete_image(
        &state.admin.pool,
        route.entity,
        internal_id,
        CatalogImageAudit {
            actor: &actor,
            reason: &reason,
            now: OffsetDateTime::now_utc(),
        },
    )
    .await
    {
        Ok(removed) => removed,
        Err(error) => return store_failure("image override delete failed", &error),
    };
    // ADR-0143 §4: 204 whether or not an override existed — removal deletes
    // only the override row; canonical media and publication plans are never
    // touched, and serving falls back to the plan/placeholder branch.
    tracing::info!(
        entity_type = route.entity.as_str(),
        entity = %public_id,
        admin = %actor,
        removed,
        "catalog artwork override removed"
    );
    StatusCode::NO_CONTENT.into_response()
}

/// PUT /api/admin/v1/catalog/artists/{id}/image — raw-body JPEG, PNG, or WebP upload.
async fn upload_artist_image(
    State(state): State<AdminCatalogState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    upload_entity_image(&state, &admin, ARTIST_ARTWORK, id, &headers, body).await
}

/// DELETE /api/admin/v1/catalog/artists/{id}/image — idempotent removal.
async fn delete_artist_image(
    State(state): State<AdminCatalogState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    delete_entity_image(&state, &admin, ARTIST_ARTWORK, id, body).await
}

/// PUT /api/admin/v1/catalog/releases/{id}/image — raw-body JPEG, PNG, or WebP upload.
async fn upload_release_image(
    State(state): State<AdminCatalogState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    upload_entity_image(&state, &admin, RELEASE_ARTWORK, id, &headers, body).await
}

/// DELETE /api/admin/v1/catalog/releases/{id}/image — idempotent removal.
async fn delete_release_image(
    State(state): State<AdminCatalogState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    body: Bytes,
) -> Response {
    delete_entity_image(&state, &admin, RELEASE_ARTWORK, id, body).await
}

// ---------------------------------------------------------------------------
// Metadata update handlers (UI-X: catalog metadata edit forms)
// ---------------------------------------------------------------------------

/// PUT /api/admin/v1/catalog/releases/{id}/metadata — update editable Release metadata.
///
/// The `/metadata` suffix is the W0 contract (2026-08-23, RELEASE-002): the admin SPA
/// has always PUT to `…/{id}/metadata`, so the route — not the client — was the bug.
async fn update_release_metadata(
    State(state): State<AdminCatalogState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(payload): Json<ReleaseMetadataUpdate>,
) -> Response {
    // Resolve public_api_id → internal id.
    let internal_id: Uuid =
        match sqlx::query_scalar("SELECT id FROM releases WHERE public_api_id = $1")
            .bind(id)
            .fetch_one(&state.admin.pool)
            .await
        {
            Ok(id) => id,
            Err(_) => return not_found("release", id),
        };

    // Parse release_date if provided.
    let release_date = match payload.release_date.as_deref().filter(|s| !s.is_empty()) {
        None => None,
        Some(raw) => {
            match Date::parse(raw, &time::format_description::well_known::Iso8601::DEFAULT) {
                Ok(d) => Some(d),
                Err(_) => {
                    return catalog_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_request",
                        "releaseDate must be a valid ISO 8601 date (YYYY-MM-DD)",
                    );
                }
            }
        }
    };

    let actor = admin.public_api_id.to_string();
    let patch = ReleaseMetadataPatch {
        title: payload.title,
        sort_title: payload.sort_title,
        display_artist: payload.display_artist,
        release_date,
        release_date_text: payload.release_date_text,
    };
    match CatalogStore
        .update_release_metadata(&state.admin.pool, internal_id, &patch, &actor)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => metadata_update_failure("release", error),
    }
}

/// PUT /api/admin/v1/catalog/artists/{id}/metadata — update editable Artist metadata.
async fn update_artist_metadata(
    State(state): State<AdminCatalogState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(payload): Json<ArtistMetadataUpdate>,
) -> Response {
    let internal_id: Uuid =
        match sqlx::query_scalar("SELECT id FROM artists WHERE public_api_id = $1")
            .bind(id)
            .fetch_one(&state.admin.pool)
            .await
        {
            Ok(id) => id,
            Err(_) => return not_found("artist", id),
        };

    let actor = admin.public_api_id.to_string();
    let patch = ArtistMetadataPatch {
        canonical_name: payload.canonical_name,
        sort_name: payload.sort_name,
        artist_type: payload.artist_type,
        status: payload.status,
    };
    match CatalogStore
        .update_artist_metadata(&state.admin.pool, internal_id, &patch, &actor)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => metadata_update_failure("artist", error),
    }
}

/// PUT /api/admin/v1/catalog/tracks/{id}/metadata — update editable Track metadata.
async fn update_track_metadata(
    State(state): State<AdminCatalogState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(payload): Json<TrackMetadataUpdate>,
) -> Response {
    let internal_id: Uuid =
        match sqlx::query_scalar("SELECT id FROM tracks WHERE public_api_id = $1")
            .bind(id)
            .fetch_one(&state.admin.pool)
            .await
        {
            Ok(id) => id,
            Err(_) => return not_found("track", id),
        };

    let actor = admin.public_api_id.to_string();
    let patch = TrackMetadataPatch {
        title: payload.title,
        sort_title: payload.sort_title,
        display_artist: payload.display_artist,
        ordinal: payload.ordinal,
        duration_ms: payload.duration_ms,
    };
    match CatalogStore
        .update_track_metadata(&state.admin.pool, internal_id, &patch, &actor)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => metadata_update_failure("track", error),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_LIST_OFFSET, validate_list_offset};

    #[test]
    fn catalog_offsets_are_strictly_bounded() {
        assert!(validate_list_offset(0).is_ok());
        assert!(validate_list_offset(MAX_LIST_OFFSET).is_ok());
        assert!(validate_list_offset(MAX_LIST_OFFSET + 1).is_err());
        assert!(validate_list_offset(u32::MAX).is_err());
    }
}
