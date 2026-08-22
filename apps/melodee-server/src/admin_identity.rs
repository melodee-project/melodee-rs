//! Artist Identity Review API (PRD-API-ADMIN-002, SPEC §20):
//! the operator queue of observed Artist strings awaiting an authoritative
//! resolution, observation detail with candidate evidence, audited
//! resolutions (confirm / create-new / leave-provisional / never-match), the
//! identity decision audit feed, and Artist merge/split corrections.
//!
//! All routes are `AdminPrincipal`-gated. Every mutation is driven through
//! the existing catalog identity store paths (`record_artist_match_decision`,
//! `record_artist_negative_assertion`, `merge_artist`, `split_artist`), which
//! carry their own audited reason/actor evidence; nothing here writes
//! identity rows directly. A string is never an Artist identity: resolutions
//! bind an observation to a Melodee-owned Artist id.
//!
//! Split limitation (surfaced honestly to clients): assignment ids are the
//! internal durable row identities of the selected links (ReleaseArtist /
//! TrackArtist / Contribution / Alias / ExternalIdentity rows), matching the
//! store contract; a picker UI arrives with the admin UI phase.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_domain::{
    Artist, ArtistId, ArtistMatchDecision, ArtistMatchOutcome, ArtistMerge,
    ArtistNegativeAssertion, ArtistPublicId, ArtistSplit, ArtistStatus, ArtistType, Confidence,
    DecisionAuthority, NewArtist, NewArtistMatchDecision, SplitAssignment,
};
use melodee_persistence::{
    ArtistIdentityRef, CatalogStore, CatalogStoreError, IdentityDecisionFeedEntry,
    ObservedArtistDetail, ObservedArtistSort, UnresolvedObservedArtist,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};

/// Maximum rows returned by identity listings.
const MAX_LIST_LIMIT: u32 = 500;

/// Resolver-version label recorded for admin-API identity corrections.
const ADMIN_RESOLVER_VERSION: &str = "admin-api-identity-v1";

fn identity_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
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
    tracing::error!(%error, context, "admin identity store failure");
    review_internal(context)
}

fn review_internal(context: &'static str) -> Response {
    identity_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", context)
}

/// Maps catalog identity store failures onto the stable error envelope.
fn catalog_error(error: CatalogStoreError) -> Response {
    match error {
        CatalogStoreError::InvalidRequest => identity_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "invalid request",
        ),
        CatalogStoreError::LockedDecision => identity_error(
            StatusCode::CONFLICT,
            "locked_decision",
            "a locked manual identity decision prevents this change",
        ),
        CatalogStoreError::NegativeAssertion => identity_error(
            StatusCode::CONFLICT,
            "negative_assertion",
            "a durable do-not-match assertion suppresses this candidate",
        ),
        CatalogStoreError::MutationConflict => identity_error(
            StatusCode::CONFLICT,
            "mutation_conflict",
            "the requested identity change conflicts with persisted catalog data",
        ),
        CatalogStoreError::CorrectionScopeExceeded { dimension, limit } => identity_error(
            StatusCode::CONFLICT,
            "scope_exceeded",
            format!("the change exceeds the {dimension} limit of {limit}; narrow the operation"),
        ),
        other => store_failure("identity mutation failed", &other),
    }
}

/// One unresolved observation row in the review queue.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedObservationDto {
    pub id: Uuid,
    pub display_value: String,
    pub raw_value: String,
    pub normalized_value: String,
    pub scope: String,
    pub source_ordinal: i32,
    pub source_pointer: String,
    pub release_id: Uuid,
    pub release_title: String,
    pub track_id: Option<Uuid>,
    pub track_title: Option<String>,
    pub pending_match_count: i64,
    pub evidence_count: i64,
    pub created_at: OffsetDateTime,
}

impl From<UnresolvedObservedArtist> for UnresolvedObservationDto {
    fn from(row: UnresolvedObservedArtist) -> Self {
        Self {
            id: row.id,
            display_value: row.display_value,
            raw_value: row.raw_value,
            normalized_value: row.normalized_value,
            scope: row.scope,
            source_ordinal: row.source_ordinal,
            source_pointer: row.source_pointer,
            release_id: row.release_public_id,
            release_title: row.release_title,
            track_id: row.track_public_id,
            track_title: row.track_title,
            pending_match_count: row.pending_match_count,
            evidence_count: row.evidence_count,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedPageDto {
    pub items: Vec<UnresolvedObservationDto>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnresolvedParams {
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
    /// Whitelisted sort `key:asc|desc`; keys `createdAt`, `displayValue`.
    pub sort: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedParams {
    #[serde(default = "default_list_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

const fn default_list_limit() -> u32 {
    50
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentDecisionDto {
    pub artist_id: Uuid,
    pub artist_name: String,
    pub outcome: String,
    pub authority: String,
    pub confidence: i32,
    pub reason: String,
    pub locked: bool,
    pub decided_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchEvidenceDto {
    pub kind: String,
    pub polarity: String,
    pub confidence: i32,
    pub source: String,
    pub payload: serde_json::Value,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateDto {
    pub artist_id: Uuid,
    pub artist_name: String,
    pub artist_status: String,
    pub latest_outcome: String,
    pub latest_confidence: i32,
    pub latest_reason: String,
    pub latest_locked: bool,
    pub latest_decided_at: OffsetDateTime,
    pub decision_count: i64,
    pub evidence: Vec<MatchEvidenceDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NegativeAssertionDto {
    pub id: Uuid,
    pub candidate_artist_id: Option<Uuid>,
    pub candidate_artist_name: Option<String>,
    pub candidate_provider: Option<String>,
    pub candidate_external_id: Option<String>,
    pub authority: String,
    pub reason: String,
    pub locked: bool,
    pub active: bool,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationDetailDto {
    pub observation: UnresolvedObservationDto,
    pub current: Option<CurrentDecisionDto>,
    pub candidates: Vec<CandidateDto>,
    pub negative_assertions: Vec<NegativeAssertionDto>,
}

/// Resolution request. `artistId` is the Artist public id and is required
/// for `confirm`, forbidden for `create-new`/`leave-provisional`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveRequest {
    pub resolution: String,
    pub artist_id: Option<Uuid>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveResponseDto {
    pub observation_id: Uuid,
    pub resolution: String,
    pub artist_id: Option<Uuid>,
    pub changed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NeverMatchRequest {
    pub artist_id: Uuid,
    pub reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeverMatchResponseDto {
    pub observation_id: Uuid,
    pub artist_id: Uuid,
    pub recorded: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDecisionEntryDto {
    pub event_id: Uuid,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub event_type: String,
    pub actor: String,
    pub reason: String,
    pub payload: serde_json::Value,
    pub created_at: OffsetDateTime,
}

impl From<IdentityDecisionFeedEntry> for IdentityDecisionEntryDto {
    fn from(entry: IdentityDecisionFeedEntry) -> Self {
        Self {
            event_id: entry.event_id,
            entity_type: entry.entity_type,
            entity_id: entry.entity_id,
            event_type: entry.event_type,
            actor: entry.actor,
            reason: entry.reason,
            payload: entry.payload,
            created_at: entry.created_at,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDecisionPageDto {
    pub items: Vec<IdentityDecisionEntryDto>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeRequest {
    pub source_artist_id: Uuid,
    pub target_artist_id: Uuid,
    pub reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResponseDto {
    pub merge_id: Uuid,
    pub source_artist_id: Uuid,
    pub target_artist_id: Uuid,
}

/// One explicitly selected link to move during a split. `id` is the
/// internal durable row identity of the selected link.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitAssignmentDto {
    pub kind: String,
    pub id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewArtistDto {
    pub canonical_name: String,
    pub sort_name: Option<String>,
    pub artist_type: Option<ArtistType>,
    pub disambiguation: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitRequest {
    pub reason: String,
    pub new_artist: NewArtistDto,
    pub assignments: Vec<SplitAssignmentDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SplitResponseDto {
    pub split_id: Uuid,
    pub source_artist_id: Uuid,
    pub new_artist_id: Uuid,
    pub moved_assignments: usize,
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route("/api/admin/v1/identity/unresolved", get(list_unresolved))
        .route(
            "/api/admin/v1/identity/observations/{id}",
            get(observation_detail),
        )
        .route(
            "/api/admin/v1/identity/observations/{id}/resolve",
            post(resolve_observation),
        )
        .route(
            "/api/admin/v1/identity/observations/{id}/never-match",
            post(never_match),
        )
        .route("/api/admin/v1/identity/decisions", get(identity_decisions))
        .route("/api/admin/v1/identity/artists/merge", post(merge_artists))
        .route(
            "/api/admin/v1/identity/artists/{id}/split",
            post(split_artist),
        )
        .route_layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

/// GET /api/admin/v1/identity/unresolved — unresolved observation queue.
async fn list_unresolved(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<UnresolvedParams>,
) -> Response {
    let sort = match params.sort.as_deref() {
        None => ObservedArtistSort::CreatedAtAsc,
        Some(raw) => match ObservedArtistSort::parse(raw) {
            Some(sort) => sort,
            None => {
                return identity_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_sort",
                    format!("unknown sort '{raw}'"),
                );
            }
        },
    };
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    match CatalogStore
        .list_unresolved_observations(&state.pool, limit, offset, sort)
        .await
    {
        Ok((items, total)) => Json(UnresolvedPageDto {
            items: items
                .into_iter()
                .map(UnresolvedObservationDto::from)
                .collect(),
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => store_failure("unresolved observation listing failed", &error),
    }
}

async fn load_detail(
    pool: &sqlx::PgPool,
    observation_id: Uuid,
) -> Result<ObservedArtistDetail, Response> {
    match CatalogStore
        .observed_artist_detail(pool, observation_id)
        .await
    {
        Ok(Some(detail)) => Ok(detail),
        Ok(None) => Err(identity_error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("observed artist not found: {observation_id}"),
        )),
        Err(error) => Err(store_failure("observation detail read failed", &error)),
    }
}

/// GET /api/admin/v1/identity/observations/{id} — observation detail with
/// candidate matches and their persisted evidence breakdown.
async fn observation_detail(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match load_detail(&state.pool, id).await {
        Ok(detail) => Json(detail_dto(detail)).into_response(),
        Err(response) => response,
    }
}

/// POST /api/admin/v1/identity/observations/{id}/resolve — confirm an
/// existing Artist, create a new provisional Artist from the observation, or
/// explicitly leave the observation provisional (an audited no-op).
async fn resolve_observation(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<ResolveRequest>,
) -> Response {
    let reason = request.reason.trim().to_string();
    if reason.is_empty() {
        return identity_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    let detail = match load_detail(&state.pool, id).await {
        Ok(detail) => detail,
        Err(response) => return response,
    };
    let now = OffsetDateTime::now_utc();
    match request.resolution.as_str() {
        "confirm" => {
            let Some(public_id) = request.artist_id else {
                return identity_error(
                    StatusCode::BAD_REQUEST,
                    "missing_artist_id",
                    "confirm requires an artistId",
                );
            };
            let artist = match resolvable_artist(&state.pool, public_id).await {
                Ok(artist) => artist,
                Err(response) => return response,
            };
            match record_confirmation(&state.pool, id, artist.id, &admin, &reason, now).await {
                Ok(()) => {
                    tracing::info!(
                        actor = %admin.public_api_id,
                        observation = %id,
                        artist = %public_id,
                        "admin confirmed observed artist match"
                    );
                    Json(ResolveResponseDto {
                        observation_id: id,
                        resolution: "confirm".to_string(),
                        artist_id: Some(public_id),
                        changed: true,
                    })
                    .into_response()
                }
                Err(response) => response,
            }
        }
        "create-new" => {
            if request.artist_id.is_some() {
                return identity_error(
                    StatusCode::BAD_REQUEST,
                    "unexpected_artist_id",
                    "create-new must not carry an artistId",
                );
            }
            let observation = &detail.observation;
            let artist = match Artist::new(NewArtist {
                id: ArtistId::new(),
                public_api_id: ArtistPublicId::from_uuid(Uuid::now_v7()),
                canonical_name: observation.display_value.clone(),
                normalized_name: observation.normalized_value.clone(),
                sort_name: observation.display_value.clone(),
                artist_type: ArtistType::Unknown,
                disambiguation: None,
                active_dates: None,
                status: ArtistStatus::Provisional,
                created_at: now,
            }) {
                Ok(artist) => artist,
                Err(error) => return store_failure("new artist construction failed", &error),
            };
            let new_public_id = artist.public_api_id().into_uuid();
            let new_artist_id = artist.id();
            if let Err(error) = CatalogStore.create_artist(&state.pool, &artist).await {
                return catalog_error(error);
            }
            match record_confirmation(&state.pool, id, new_artist_id, &admin, &reason, now).await {
                Ok(()) => {
                    tracing::info!(
                        actor = %admin.public_api_id,
                        observation = %id,
                        artist = %new_public_id,
                        "admin created artist for observed artist"
                    );
                    Json(ResolveResponseDto {
                        observation_id: id,
                        resolution: "create-new".to_string(),
                        artist_id: Some(new_public_id),
                        changed: true,
                    })
                    .into_response()
                }
                Err(response) => response,
            }
        }
        "leave-provisional" => {
            if request.artist_id.is_some() {
                return identity_error(
                    StatusCode::BAD_REQUEST,
                    "unexpected_artist_id",
                    "leave-provisional must not carry an artistId",
                );
            }
            // Honest no-op acknowledgement: no decision row is written, so
            // the observation remains unresolved for a later review pass.
            tracing::info!(
                actor = %admin.public_api_id,
                observation = %id,
                reason = %reason,
                "admin left observed artist provisional"
            );
            Json(ResolveResponseDto {
                observation_id: id,
                resolution: "leave-provisional".to_string(),
                artist_id: None,
                changed: false,
            })
            .into_response()
        }
        other => identity_error(
            StatusCode::BAD_REQUEST,
            "unknown_resolution",
            format!("unknown resolution '{other}'"),
        ),
    }
}

/// Records the administrator's confirmed match decision through the durable
/// decision store (locked, so no automated resolver supersedes it).
async fn record_confirmation(
    pool: &sqlx::PgPool,
    observation_id: Uuid,
    artist_id: ArtistId,
    admin: &AdminPrincipal,
    reason: &str,
    now: OffsetDateTime,
) -> Result<(), Response> {
    let decision = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: melodee_domain::ArtistMatchDecisionId::new(),
            observed_artist_id: melodee_domain::ObservedArtistId::from_uuid(observation_id),
            artist_id,
            outcome: ArtistMatchOutcome::Confirmed,
            authority: DecisionAuthority::Administrator {
                actor: admin.public_api_id.to_string(),
            },
            locked: true,
            reason: reason.to_string(),
            decided_at: now,
        },
        &[],
    )
    .map_err(|error| store_failure("match decision construction failed", &error))?;
    let confidence = Confidence::new(10_000)
        .map_err(|error| store_failure("match decision construction failed", &error))?;
    CatalogStore
        .record_artist_match_decision(
            pool,
            &decision,
            confidence,
            serde_json::json!({ "surface": "admin-api/v1/identity" }),
        )
        .await
        .map_err(catalog_error)
}

/// POST /api/admin/v1/identity/observations/{id}/never-match — durable
/// do-not-match assertion against the exact observation/candidate pair.
async fn never_match(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<NeverMatchRequest>,
) -> Response {
    let reason = request.reason.trim().to_string();
    if reason.is_empty() {
        return identity_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    if let Err(response) = load_detail(&state.pool, id).await {
        return response;
    }
    let artist = match resolvable_artist(&state.pool, request.artist_id).await {
        Ok(artist) => artist,
        Err(response) => return response,
    };
    let assertion = ArtistNegativeAssertion {
        id: melodee_domain::ArtistNegativeAssertionId::new(),
        observed_artist_id: melodee_domain::ObservedArtistId::from_uuid(id),
        rejected_artist_id: artist.id,
        authority: DecisionAuthority::Administrator {
            actor: admin.public_api_id.to_string(),
        },
        locked: true,
        reason,
        decided_at: OffsetDateTime::now_utc(),
    };
    match CatalogStore
        .record_artist_negative_assertion(&state.pool, &assertion)
        .await
    {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                observation = %id,
                artist = %request.artist_id,
                "admin recorded do-not-match assertion"
            );
            Json(NeverMatchResponseDto {
                observation_id: id,
                artist_id: request.artist_id,
                recorded: true,
            })
            .into_response()
        }
        Err(error) => catalog_error(error),
    }
}

/// GET /api/admin/v1/identity/decisions — recent identity decisions feed.
async fn identity_decisions(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<FeedParams>,
) -> Response {
    let limit = params.limit.clamp(1, MAX_LIST_LIMIT);
    let offset = params.offset;
    match CatalogStore
        .list_identity_decisions(&state.pool, limit, offset)
        .await
    {
        Ok((items, total)) => Json(IdentityDecisionPageDto {
            items: items
                .into_iter()
                .map(IdentityDecisionEntryDto::from)
                .collect(),
            total,
            limit,
            offset,
        })
        .into_response(),
        Err(error) => store_failure("identity decision feed read failed", &error),
    }
}

/// POST /api/admin/v1/identity/artists/merge — merge the source Artist into
/// the target, preserving the source public id as a redirect.
async fn merge_artists(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<MergeRequest>,
) -> Response {
    let reason = request.reason.trim().to_string();
    if reason.is_empty() {
        return identity_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    if request.source_artist_id == request.target_artist_id {
        return identity_error(
            StatusCode::BAD_REQUEST,
            "self_merge",
            "source and target artists must differ",
        );
    }
    let source = match resolvable_artist(&state.pool, request.source_artist_id).await {
        Ok(artist) => artist,
        Err(response) => return response,
    };
    let target = match resolvable_artist(&state.pool, request.target_artist_id).await {
        Ok(artist) => artist,
        Err(response) => return response,
    };
    let merge = ArtistMerge {
        id: melodee_domain::ArtistMergeId::new(),
        source_artist_id: source.id,
        source_public_api_id: ArtistPublicId::from_uuid(request.source_artist_id),
        target_artist_id: target.id,
        actor: admin.public_api_id.to_string(),
        reason,
        occurred_at: OffsetDateTime::now_utc(),
    };
    match CatalogStore
        .merge_artist(&state.pool, &merge, ADMIN_RESOLVER_VERSION)
        .await
    {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                source = %request.source_artist_id,
                target = %request.target_artist_id,
                "admin merged artist"
            );
            Json(MergeResponseDto {
                merge_id: merge.id.into_uuid(),
                source_artist_id: request.source_artist_id,
                target_artist_id: request.target_artist_id,
            })
            .into_response()
        }
        Err(error) => catalog_error(error),
    }
}

/// POST /api/admin/v1/identity/artists/{id}/split — create a new Artist and
/// move only the explicitly selected assignments off the source Artist.
async fn split_artist(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<SplitRequest>,
) -> Response {
    let reason = request.reason.trim().to_string();
    if reason.is_empty() {
        return identity_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    if request.assignments.is_empty() {
        return identity_error(
            StatusCode::BAD_REQUEST,
            "empty_split",
            "a split requires at least one selected assignment",
        );
    }
    let canonical_name = request.new_artist.canonical_name.trim().to_string();
    if canonical_name.is_empty() {
        return identity_error(
            StatusCode::BAD_REQUEST,
            "invalid_artist_name",
            "the new artist's canonical name must be non-blank",
        );
    }
    let source = match resolvable_artist(&state.pool, id).await {
        Ok(artist) => artist,
        Err(response) => return response,
    };
    let mut assignments = Vec::with_capacity(request.assignments.len());
    for assignment in &request.assignments {
        let parsed = match assignment.kind.as_str() {
            "releaseArtist" => SplitAssignment::ReleaseArtist(
                melodee_domain::ReleaseArtistId::from_uuid(assignment.id),
            ),
            "trackArtist" => SplitAssignment::TrackArtist(
                melodee_domain::TrackArtistId::from_uuid(assignment.id),
            ),
            "contribution" => SplitAssignment::Contribution(
                melodee_domain::ContributionId::from_uuid(assignment.id),
            ),
            "alias" => {
                SplitAssignment::Alias(melodee_domain::ArtistAliasId::from_uuid(assignment.id))
            }
            "externalIdentity" => SplitAssignment::ExternalIdentity(
                melodee_domain::ExternalIdentityId::from_uuid(assignment.id),
            ),
            other => {
                return identity_error(
                    StatusCode::BAD_REQUEST,
                    "unknown_assignment_kind",
                    format!("unknown assignment kind '{other}'"),
                );
            }
        };
        assignments.push(parsed);
    }
    let now = OffsetDateTime::now_utc();
    let sort_name = request
        .new_artist
        .sort_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&canonical_name)
        .to_string();
    let new_artist = match Artist::new(NewArtist {
        id: ArtistId::new(),
        public_api_id: ArtistPublicId::from_uuid(Uuid::now_v7()),
        normalized_name: canonical_name.to_lowercase(),
        canonical_name,
        sort_name,
        artist_type: request
            .new_artist
            .artist_type
            .unwrap_or(ArtistType::Unknown),
        disambiguation: request
            .new_artist
            .disambiguation
            .filter(|value| !value.trim().is_empty()),
        active_dates: None,
        status: ArtistStatus::Provisional,
        created_at: now,
    }) {
        Ok(artist) => artist,
        Err(error) => {
            return identity_error(StatusCode::BAD_REQUEST, "invalid_artist", error.to_string());
        }
    };
    let split = ArtistSplit {
        id: melodee_domain::ArtistSplitId::new(),
        source_artist_id: source.id,
        new_artist_id: new_artist.id(),
        assignments,
        actor: admin.public_api_id.to_string(),
        reason,
        occurred_at: now,
    };
    let moved_assignments = split.assignments.len();
    let new_public_id = new_artist.public_api_id().into_uuid();
    match CatalogStore
        .split_artist(&state.pool, &split, &new_artist, ADMIN_RESOLVER_VERSION)
        .await
    {
        Ok(()) => {
            tracing::info!(
                actor = %admin.public_api_id,
                source = %id,
                new_artist = %new_public_id,
                "admin split artist"
            );
            Json(SplitResponseDto {
                split_id: split.id.into_uuid(),
                source_artist_id: id,
                new_artist_id: new_public_id,
                moved_assignments,
            })
            .into_response()
        }
        Err(CatalogStoreError::Domain(error)) => identity_error(
            StatusCode::BAD_REQUEST,
            "invalid_assignment",
            error.to_string(),
        ),
        Err(error) => catalog_error(error),
    }
}

/// Resolves an Artist public id for a mutation entry point; merged and
/// deprecated Artists are not valid mutation participants.
async fn resolvable_artist(
    pool: &sqlx::PgPool,
    public_id: Uuid,
) -> Result<ArtistIdentityRef, Response> {
    match CatalogStore.artist_by_public_id(pool, public_id).await {
        Ok(Some(artist)) if matches!(artist.status.as_str(), "merged" | "deprecated") => {
            Err(identity_error(
                StatusCode::CONFLICT,
                "artist_not_resolvable",
                format!(
                    "artist {public_id} is {} and cannot participate",
                    artist.status
                ),
            ))
        }
        Ok(Some(artist)) => Ok(artist),
        Ok(None) => Err(identity_error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("artist not found: {public_id}"),
        )),
        Err(error) => Err(store_failure("artist resolution failed", &error)),
    }
}

fn detail_dto(detail: ObservedArtistDetail) -> ObservationDetailDto {
    ObservationDetailDto {
        observation: UnresolvedObservationDto::from(detail.observation),
        current: detail.current.map(|current| CurrentDecisionDto {
            artist_id: current.artist_public_id,
            artist_name: current.artist_name,
            outcome: current.outcome,
            authority: current.authority,
            confidence: current.confidence,
            reason: current.reason,
            locked: current.locked,
            decided_at: current.decided_at,
        }),
        candidates: detail
            .candidates
            .into_iter()
            .map(|candidate| CandidateDto {
                artist_id: candidate.artist_public_id,
                artist_name: candidate.artist_name,
                artist_status: candidate.artist_status,
                latest_outcome: candidate.latest_outcome,
                latest_confidence: candidate.latest_confidence,
                latest_reason: candidate.latest_reason,
                latest_locked: candidate.latest_locked,
                latest_decided_at: candidate.latest_decided_at,
                decision_count: candidate.decision_count,
                evidence: candidate
                    .evidence
                    .into_iter()
                    .map(|evidence| MatchEvidenceDto {
                        kind: evidence.kind,
                        polarity: evidence.polarity,
                        confidence: evidence.confidence,
                        source: evidence.source,
                        payload: evidence.payload,
                        created_at: evidence.created_at,
                    })
                    .collect(),
            })
            .collect(),
        negative_assertions: detail
            .negative_assertions
            .into_iter()
            .map(|assertion| NegativeAssertionDto {
                id: assertion.id,
                candidate_artist_id: assertion.candidate_artist_public_id,
                candidate_artist_name: assertion.candidate_artist_name,
                candidate_provider: assertion.candidate_provider,
                candidate_external_id: assertion.candidate_external_id,
                authority: assertion.authority,
                reason: assertion.reason,
                locked: assertion.locked,
                active: assertion.active,
                created_at: assertion.created_at,
            })
            .collect(),
    }
}
