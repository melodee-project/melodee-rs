//! Duplicate-candidate discovery for AI-assisted catalog curation
//! (ADR-0196, PRD-API-ADMIN-002).
//!
//! These are **read-only, advisory** scans. They return bounded pairs of
//! Artists or Releases with the evidence that made them candidates, the
//! scoring weight of each evidence fact, and any blocker that must prevent an
//! automatic merge. A candidate is never an identity decision:
//!
//! * names are search attributes, never identity keys (PRD-ARTIST-001);
//! * provider candidates and external identifiers are evidence, not authority
//!   (PRD-ARTIST-012, ADR-0162);
//! * a name-only pair is always reported with a `name_only` blocker, because
//!   automatic convergence requires non-name evidence (ADR-0174);
//! * the scan never writes identity rows. Execution stays on the existing
//!   audited merge/resolve endpoints.
//!
//! Every route is `AdminPrincipal`-gated and bounded. The scan uses set-based
//! SQL with a server-side execution budget so one request cannot hold a
//! database connection indefinitely.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};

const MAX_LIST_LIMIT: u32 = 200;
const MAX_LIST_OFFSET: u32 = 1_000_000;
const DEFAULT_LIST_LIMIT: u32 = 50;
const SCAN_TIMEOUT: Duration = Duration::from_secs(10);

/// Evidence weights are basis-point-style advisory scores. They follow the
/// same conservative preference as the resolver: provider identity evidence
/// outranks name evidence, and a name-only pair can never reach a high score.
const WEIGHT_SHARED_TRUSTED_EXTERNAL_ID: u32 = 6_000;
const WEIGHT_EXACT_NORMALIZED_NAME: u32 = 2_000;
const WEIGHT_ALIAS_OVERLAP: u32 = 1_500;
const WEIGHT_SHARED_PROVIDER_CANDIDATE: u32 = 3_000;
const WEIGHT_SHARED_AUDIO_IDENTITY: u32 = 3_500;
const WEIGHT_SHARED_AUDIO_PER_TRACK: u32 = 700;
const WEIGHT_SHARED_TRACK_TITLES: u32 = 3_000;
const WEIGHT_SHARED_TRACK_PER_TRACK: u32 = 600;
const WEIGHT_SAME_TITLE_AND_ARTIST: u32 = 2_500;
const WEIGHT_SAME_NORMALIZED_TITLE: u32 = 1_000;

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route(
            "/api/admin/v1/catalog/duplicates/artists",
            get(artist_duplicates),
        )
        .route(
            "/api/admin/v1/catalog/duplicates/releases",
            get(release_duplicates),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CandidateParams {
    limit: Option<u32>,
    offset: Option<u32>,
    min_score: Option<u16>,
}

impl CandidateParams {
    #[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
    fn validated(&self) -> Result<(i64, i64, i32), Response> {
        let limit = self.limit.unwrap_or(DEFAULT_LIST_LIMIT);
        if limit == 0 || limit > MAX_LIST_LIMIT {
            return Err(duplicates_error(
                StatusCode::BAD_REQUEST,
                "invalid_limit",
                format!("limit must be between 1 and {MAX_LIST_LIMIT}"),
            ));
        }
        let offset = self.offset.unwrap_or(0);
        if offset > MAX_LIST_OFFSET {
            return Err(duplicates_error(
                StatusCode::BAD_REQUEST,
                "invalid_offset",
                "offset exceeds the maximum supported candidate window",
            ));
        }
        let min_score = self.min_score.unwrap_or(0);
        if min_score > 10_000 {
            return Err(duplicates_error(
                StatusCode::BAD_REQUEST,
                "invalid_min_score",
                "minScore must be between 0 and 10000",
            ));
        }
        Ok((i64::from(limit), i64::from(offset), i32::from(min_score)))
    }
}

fn duplicates_error(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
) -> Response {
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

fn query_timeout() -> Response {
    duplicates_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "query_timeout",
        "duplicate candidate scan exceeded its execution budget",
    )
}

fn store_failure(context: &'static str, error: &sqlx::Error) -> Response {
    tracing::error!(%error, context, "admin duplicate candidate store failure");
    duplicates_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", context)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateEvidenceDto {
    kind: &'static str,
    weight: u32,
    detail: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateBlockerDto {
    kind: &'static str,
    detail: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistCandidateDto {
    score: u16,
    left: ArtistCandidateSideDto,
    right: ArtistCandidateSideDto,
    evidence: Vec<DuplicateEvidenceDto>,
    blockers: Vec<DuplicateBlockerDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistCandidateSideDto {
    id: Uuid,
    name: String,
    sort_name: String,
    artist_type: String,
    disambiguation: Option<String>,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtistCandidatePageDto {
    items: Vec<ArtistCandidateDto>,
    total: i64,
    limit: u32,
    offset: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseCandidateDto {
    score: u16,
    left: ReleaseCandidateSideDto,
    right: ReleaseCandidateSideDto,
    evidence: Vec<DuplicateEvidenceDto>,
    blockers: Vec<DuplicateBlockerDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseCandidateSideDto {
    id: Uuid,
    title: String,
    display_artist: Option<String>,
    release_date: Option<String>,
    status: String,
    track_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseCandidatePageDto {
    items: Vec<ReleaseCandidateDto>,
    total: i64,
    limit: u32,
    offset: u32,
}

#[derive(Debug, sqlx::FromRow)]
struct ArtistPairRow {
    left_id: Uuid,
    right_id: Uuid,
    name_match: bool,
    alias_match: bool,
    external_match: bool,
    provider_match: bool,
    score: i32,
    total: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct ReleasePairRow {
    left_id: Uuid,
    right_id: Uuid,
    external_match: bool,
    shared_audio: i64,
    shared_titles: i64,
    title_artist_match: bool,
    title_match: bool,
    score: i32,
    total: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct ArtistSummaryRow {
    id: Uuid,
    public_api_id: Uuid,
    canonical_name: String,
    normalized_name: String,
    sort_name: String,
    artist_type: String,
    disambiguation: Option<String>,
    status: String,
}

#[derive(Debug, sqlx::FromRow)]
struct ReleaseSummaryRow {
    id: Uuid,
    public_api_id: Uuid,
    title: String,
    display_artist: Option<String>,
    release_date: Option<String>,
    status: String,
    track_count: i64,
    normalized_title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProviderEvidence {
    provider: String,
    external_id: String,
}

async fn artist_duplicates(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<CandidateParams>,
) -> Response {
    let (limit, offset, min_score) = match params.validated() {
        Ok(values) => values,
        Err(response) => return response,
    };
    match tokio::time::timeout(
        SCAN_TIMEOUT,
        scan_artist_duplicates(&state.pool, min_score, limit, offset),
    )
    .await
    {
        Ok(Ok(page)) => Json(page).into_response(),
        Ok(Err(error)) => store_failure("artist duplicate scan failed", &error),
        Err(_) => query_timeout(),
    }
}

async fn release_duplicates(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Query(params): Query<CandidateParams>,
) -> Response {
    let (limit, offset, min_score) = match params.validated() {
        Ok(values) => values,
        Err(response) => return response,
    };
    match tokio::time::timeout(
        SCAN_TIMEOUT,
        scan_release_duplicates(&state.pool, min_score, limit, offset),
    )
    .await
    {
        Ok(Ok(page)) => Json(page).into_response(),
        Ok(Err(error)) => store_failure("release duplicate scan failed", &error),
        Err(_) => query_timeout(),
    }
}

async fn scan_artist_duplicates(
    pool: &PgPool,
    min_score: i32,
    limit: i64,
    offset: i64,
) -> Result<ArtistCandidatePageDto, sqlx::Error> {
    let pairs = sqlx::query_as::<_, ArtistPairRow>(
        "WITH eligible AS (
             SELECT id, normalized_name
               FROM artists
              WHERE status IN ('active', 'verified')
         ),
         name_pairs AS (
             SELECT a.id AS left_id, b.id AS right_id, true AS name_match
               FROM eligible a
               JOIN eligible b
                 ON b.normalized_name = a.normalized_name
                AND a.id < b.id
         ),
         alias_pairs AS (
             SELECT DISTINCT a.id AS left_id, b.id AS right_id, true AS alias_match
               FROM eligible a
               JOIN eligible b ON a.id < b.id
              WHERE a.normalized_name IN (
                        SELECT normalized_alias
                          FROM artist_aliases
                         WHERE artist_id = b.id
                    )
                 OR b.normalized_name IN (
                        SELECT normalized_alias
                          FROM artist_aliases
                         WHERE artist_id = a.id
                    )
                 OR EXISTS (
                        SELECT 1
                          FROM artist_aliases aal
                          JOIN artist_aliases bal
                            ON bal.normalized_alias = aal.normalized_alias
                         WHERE aal.artist_id = a.id
                           AND bal.artist_id = b.id
                    )
         ),
         external_pairs AS (
             SELECT DISTINCT a.artist_id AS left_id,
                             b.artist_id AS right_id,
                             true AS external_match
               FROM artist_external_identities a
               JOIN artist_external_identities b
                 ON b.provider = a.provider
                AND b.external_id = a.external_id
                AND a.artist_id < b.artist_id
               JOIN eligible ea ON ea.id = a.artist_id
               JOIN eligible eb ON eb.id = b.artist_id
         ),
         provider_pairs AS (
             SELECT DISTINCT a.artist_id AS left_id,
                             b.artist_id AS right_id,
                             true AS provider_match
               FROM provider_identity_candidates a
               JOIN provider_identity_candidates b
                 ON b.provider = a.provider
                AND b.external_id = a.external_id
                AND a.artist_id < b.artist_id
               JOIN eligible ea ON ea.id = a.artist_id
               JOIN eligible eb ON eb.id = b.artist_id
         ),
         combined AS (
             SELECT left_id, right_id, name_match,
                    NULL::bool AS alias_match, NULL::bool AS external_match,
                    NULL::bool AS provider_match
               FROM name_pairs
             UNION ALL
             SELECT left_id, right_id, NULL,
                    alias_match, NULL, NULL
               FROM alias_pairs
             UNION ALL
             SELECT left_id, right_id, NULL,
                    NULL, external_match, NULL
               FROM external_pairs
             UNION ALL
             SELECT left_id, right_id, NULL,
                    NULL, NULL, provider_match
               FROM provider_pairs
         ),
         grouped AS (
             SELECT left_id, right_id,
                    COALESCE(bool_or(name_match), false) AS name_match,
                    COALESCE(bool_or(alias_match), false) AS alias_match,
                    COALESCE(bool_or(external_match), false) AS external_match,
                    COALESCE(bool_or(provider_match), false) AS provider_match
               FROM combined
              GROUP BY left_id, right_id
         ),
         scored AS (
             SELECT left_id, right_id, name_match, alias_match,
                    external_match, provider_match,
                    LEAST(
                        10000,
                        (CASE WHEN name_match THEN 2000 ELSE 0 END)
                      + (CASE WHEN alias_match THEN 1500 ELSE 0 END)
                      + (CASE WHEN external_match THEN 6000 ELSE 0 END)
                      + (CASE WHEN provider_match THEN 3000 ELSE 0 END)
                    )::int AS score
               FROM grouped
         )
         SELECT left_id, right_id, name_match, alias_match, external_match,
                provider_match, score, COUNT(*) OVER () AS total
           FROM scored
          WHERE score >= $1
          ORDER BY score DESC, left_id, right_id
          LIMIT $2 OFFSET $3",
    )
    .bind(min_score)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    if pairs.is_empty() {
        return Ok(ArtistCandidatePageDto {
            items: Vec::new(),
            total: 0,
            limit: u32::try_from(limit).unwrap_or(MAX_LIST_LIMIT),
            offset: u32::try_from(offset).unwrap_or(MAX_LIST_OFFSET),
        });
    }

    let total = pairs.first().map_or(0, |pair| pair.total);
    let ids = pairs
        .iter()
        .flat_map(|pair| [pair.left_id, pair.right_id])
        .collect::<BTreeSet<_>>();
    let ids = ids.into_iter().collect::<Vec<_>>();
    let summaries = load_artist_summaries(pool, &ids).await?;
    let aliases = load_artist_aliases(pool, &ids).await?;
    let external = load_artist_external_identities(pool, &ids).await?;
    let provider_candidates = load_artist_provider_candidates(pool, &ids).await?;

    let items = pairs
        .into_iter()
        .filter_map(|pair| {
            let left = summaries.get(&pair.left_id)?;
            let right = summaries.get(&pair.right_id)?;
            Some(artist_candidate_dto(
                &pair,
                left,
                right,
                ArtistSideEvidence {
                    aliases: aliases.get(&pair.left_id).map(Vec::as_slice).unwrap_or(&[]),
                    external: external
                        .get(&pair.left_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    provider_candidates: provider_candidates
                        .get(&pair.left_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                },
                ArtistSideEvidence {
                    aliases: aliases
                        .get(&pair.right_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    external: external
                        .get(&pair.right_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                    provider_candidates: provider_candidates
                        .get(&pair.right_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                },
            ))
        })
        .collect::<Vec<_>>();

    Ok(ArtistCandidatePageDto {
        items,
        total,
        limit: u32::try_from(limit).unwrap_or(MAX_LIST_LIMIT),
        offset: u32::try_from(offset).unwrap_or(MAX_LIST_OFFSET),
    })
}

/// Evidence attached to one side of an Artist candidate pair.
#[derive(Clone, Copy)]
struct ArtistSideEvidence<'a> {
    aliases: &'a [String],
    external: &'a [ProviderEvidence],
    provider_candidates: &'a [ProviderEvidence],
}

fn artist_candidate_dto(
    pair: &ArtistPairRow,
    left: &ArtistSummaryRow,
    right: &ArtistSummaryRow,
    left_evidence: ArtistSideEvidence<'_>,
    right_evidence: ArtistSideEvidence<'_>,
) -> ArtistCandidateDto {
    let mut evidence = Vec::new();
    if pair.name_match {
        evidence.push(DuplicateEvidenceDto {
            kind: "exact_normalized_name",
            weight: WEIGHT_EXACT_NORMALIZED_NAME,
            detail: json!({ "normalizedName": left.normalized_name }),
        });
    }
    let left_aliases = left_evidence.aliases;
    let right_aliases = right_evidence.aliases;
    let left_external = left_evidence.external;
    let right_external = right_evidence.external;
    let left_provider_candidates = left_evidence.provider_candidates;
    let right_provider_candidates = right_evidence.provider_candidates;
    let mut left_values = left_aliases.to_vec();
    left_values.push(left.normalized_name.clone());
    let mut right_values = right_aliases.to_vec();
    right_values.push(right.normalized_name.clone());
    let alias_overlap = overlapping_values(&left_values, &right_values)
        .into_iter()
        .filter(|value| !(pair.name_match && value == &left.normalized_name))
        .collect::<Vec<_>>();
    for value in alias_overlap.iter().take(5) {
        evidence.push(DuplicateEvidenceDto {
            kind: "alias_overlap",
            weight: WEIGHT_ALIAS_OVERLAP,
            detail: json!({ "normalizedAlias": value }),
        });
    }
    let shared_external = intersect_evidence(left_external, right_external);
    for shared in shared_external.iter().take(5) {
        evidence.push(DuplicateEvidenceDto {
            kind: "shared_trusted_external_id",
            weight: WEIGHT_SHARED_TRUSTED_EXTERNAL_ID,
            detail: json!({
                "provider": shared.provider,
                "externalId": shared.external_id,
            }),
        });
    }
    let shared_provider_candidates =
        intersect_evidence(left_provider_candidates, right_provider_candidates);
    for shared in shared_provider_candidates.iter().take(5) {
        evidence.push(DuplicateEvidenceDto {
            kind: "shared_provider_candidate",
            weight: WEIGHT_SHARED_PROVIDER_CANDIDATE,
            detail: json!({
                "provider": shared.provider,
                "externalId": shared.external_id,
            }),
        });
    }

    let mut blockers = Vec::new();
    if left.disambiguation != right.disambiguation
        && (left.disambiguation.is_some() || right.disambiguation.is_some())
    {
        // A disambiguation hint on only one side still distinguishes the
        // identities for automatic purposes; an operator may override it.
        blockers.push(DuplicateBlockerDto {
            kind: "disambiguation_conflict",
            detail: json!({
                "left": left.disambiguation,
                "right": right.disambiguation,
            }),
        });
    }
    if left.artist_type != "unknown"
        && right.artist_type != "unknown"
        && left.artist_type != right.artist_type
    {
        blockers.push(DuplicateBlockerDto {
            kind: "artist_type_conflict",
            detail: json!({
                "left": left.artist_type,
                "right": right.artist_type,
            }),
        });
    }
    for (provider, left_ids, right_ids) in provider_conflicts(left_external, right_external) {
        blockers.push(DuplicateBlockerDto {
            kind: "trusted_provider_conflict",
            detail: json!({
                "provider": provider,
                "leftExternalIds": left_ids,
                "rightExternalIds": right_ids,
            }),
        });
    }
    if !pair.external_match && !pair.provider_match && !pair.alias_match {
        blockers.push(DuplicateBlockerDto {
            kind: "name_only",
            detail: json!({
                "message": "name equality is a review hint, never an automatic identity predicate",
            }),
        });
    }

    ArtistCandidateDto {
        score: clamp_score(pair.score),
        left: ArtistCandidateSideDto {
            id: left.public_api_id,
            name: left.canonical_name.clone(),
            sort_name: left.sort_name.clone(),
            artist_type: left.artist_type.clone(),
            disambiguation: left.disambiguation.clone(),
            status: left.status.clone(),
        },
        right: ArtistCandidateSideDto {
            id: right.public_api_id,
            name: right.canonical_name.clone(),
            sort_name: right.sort_name.clone(),
            artist_type: right.artist_type.clone(),
            disambiguation: right.disambiguation.clone(),
            status: right.status.clone(),
        },
        evidence,
        blockers,
    }
}

async fn scan_release_duplicates(
    pool: &PgPool,
    min_score: i32,
    limit: i64,
    offset: i64,
) -> Result<ReleaseCandidatePageDto, sqlx::Error> {
    let pairs = sqlx::query_as::<_, ReleasePairRow>(
        "WITH eligible AS (
             SELECT id, normalized_title,
                    lower(btrim(coalesce(display_artist, ''))) AS artist_key
               FROM releases
              WHERE status = 'active'
         ),
         external_pairs AS (
             SELECT DISTINCT a.release_id AS left_id,
                             b.release_id AS right_id,
                             true AS external_match
               FROM release_external_identities a
               JOIN release_external_identities b
                 ON b.provider = a.provider
                AND b.external_id = a.external_id
                AND a.release_id < b.release_id
                AND a.trusted
                AND b.trusted
               JOIN eligible ea ON ea.id = a.release_id
               JOIN eligible eb ON eb.id = b.release_id
         ),
         audio_pairs AS (
             SELECT t1.release_id AS left_id,
                    t2.release_id AS right_id,
                    COUNT(DISTINCT t1.audio_identity_id)::bigint AS shared_audio
               FROM tracks t1
               JOIN tracks t2
                 ON t2.audio_identity_id = t1.audio_identity_id
                AND t1.release_id < t2.release_id
               JOIN eligible ea ON ea.id = t1.release_id
               JOIN eligible eb ON eb.id = t2.release_id
              GROUP BY t1.release_id, t2.release_id
         ),
         track_pairs AS (
             SELECT t1.release_id AS left_id,
                    t2.release_id AS right_id,
                    COUNT(*)::bigint AS shared_titles
               FROM tracks t1
               JOIN tracks t2
                 ON t2.release_id > t1.release_id
                AND t2.normalized_title = t1.normalized_title
                AND t1.duration_ms IS NOT NULL
                AND t2.duration_ms IS NOT NULL
                AND abs(t1.duration_ms - t2.duration_ms) <= 2000
               JOIN eligible ea ON ea.id = t1.release_id
               JOIN eligible eb ON eb.id = t2.release_id
              GROUP BY t1.release_id, t2.release_id
         ),
         title_artist_pairs AS (
             SELECT DISTINCT a.id AS left_id, b.id AS right_id,
                    true AS title_artist_match
               FROM eligible a
               JOIN eligible b
                 ON b.id > a.id
                AND b.normalized_title = a.normalized_title
                AND a.artist_key <> ''
                AND b.artist_key = a.artist_key
         ),
         title_pairs AS (
             SELECT DISTINCT a.id AS left_id, b.id AS right_id, true AS title_match
               FROM eligible a
               JOIN eligible b
                 ON b.id > a.id
                AND b.normalized_title = a.normalized_title
         ),
         combined AS (
             SELECT left_id, right_id, external_match,
                    0::bigint AS shared_audio, 0::bigint AS shared_titles,
                    NULL::bool AS title_artist_match,
                    NULL::bool AS title_match
               FROM external_pairs
             UNION ALL
             SELECT left_id, right_id, NULL,
                    shared_audio, 0, NULL, NULL
               FROM audio_pairs
             UNION ALL
             SELECT left_id, right_id, NULL,
                    0, shared_titles, NULL, NULL
               FROM track_pairs
             UNION ALL
             SELECT left_id, right_id, NULL,
                    0, 0, title_artist_match, NULL
               FROM title_artist_pairs
             UNION ALL
             SELECT left_id, right_id, NULL,
                    0, 0, NULL, title_match
               FROM title_pairs
         ),
         grouped AS (
             SELECT left_id, right_id,
                    COALESCE(bool_or(external_match), false) AS external_match,
                    COALESCE(MAX(shared_audio), 0) AS shared_audio,
                    COALESCE(MAX(shared_titles), 0) AS shared_titles,
                    COALESCE(bool_or(title_artist_match), false) AS title_artist_match,
                    COALESCE(bool_or(title_match), false) AS title_match
               FROM combined
              GROUP BY left_id, right_id
         ),
         scored AS (
             SELECT left_id, right_id, external_match, shared_audio,
                    shared_titles, title_artist_match, title_match,
                    LEAST(
                        10000,
                        (CASE WHEN external_match THEN 6000 ELSE 0 END)
                      + LEAST(3500, (shared_audio * 700)::int)
                      + LEAST(3000, (shared_titles * 600)::int)
                      + (CASE WHEN title_artist_match THEN 2500
                              WHEN title_match THEN 1000
                              ELSE 0 END)
                    )::int AS score
               FROM grouped
         )
         SELECT left_id, right_id, external_match, shared_audio, shared_titles,
                title_artist_match, title_match, score,
                COUNT(*) OVER () AS total
           FROM scored
          WHERE score >= $1
          ORDER BY score DESC, left_id, right_id
          LIMIT $2 OFFSET $3",
    )
    .bind(min_score)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    if pairs.is_empty() {
        return Ok(ReleaseCandidatePageDto {
            items: Vec::new(),
            total: 0,
            limit: u32::try_from(limit).unwrap_or(MAX_LIST_LIMIT),
            offset: u32::try_from(offset).unwrap_or(MAX_LIST_OFFSET),
        });
    }

    let ids = pairs
        .iter()
        .flat_map(|pair| [pair.left_id, pair.right_id])
        .collect::<BTreeSet<_>>();
    let ids = ids.into_iter().collect::<Vec<_>>();
    let summaries = load_release_summaries(pool, &ids).await?;
    let external = load_release_external_identities(pool, &ids).await?;

    let items = pairs
        .iter()
        .filter_map(|pair| {
            let left = summaries.get(&pair.left_id)?;
            let right = summaries.get(&pair.right_id)?;
            Some(release_candidate_dto(
                pair,
                left,
                right,
                external
                    .get(&pair.left_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                external
                    .get(&pair.right_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            ))
        })
        .collect::<Vec<_>>();

    Ok(ReleaseCandidatePageDto {
        items,
        total: pairs.first().map_or(0, |pair| pair.total),
        limit: u32::try_from(limit).unwrap_or(MAX_LIST_LIMIT),
        offset: u32::try_from(offset).unwrap_or(MAX_LIST_OFFSET),
    })
}

fn release_candidate_dto(
    pair: &ReleasePairRow,
    left: &ReleaseSummaryRow,
    right: &ReleaseSummaryRow,
    left_external: &[ProviderEvidence],
    right_external: &[ProviderEvidence],
) -> ReleaseCandidateDto {
    let mut evidence = Vec::new();
    if pair.external_match {
        for shared in intersect_evidence(left_external, right_external)
            .iter()
            .take(5)
        {
            evidence.push(DuplicateEvidenceDto {
                kind: "shared_trusted_external_id",
                weight: WEIGHT_SHARED_TRUSTED_EXTERNAL_ID,
                detail: json!({
                    "provider": shared.provider,
                    "externalId": shared.external_id,
                }),
            });
        }
    }
    if pair.shared_audio > 0 {
        evidence.push(DuplicateEvidenceDto {
            kind: "shared_audio_identity",
            weight: shared_audio_weight(pair.shared_audio),
            detail: json!({ "sharedTrackCount": pair.shared_audio }),
        });
    }
    if pair.shared_titles > 0 {
        evidence.push(DuplicateEvidenceDto {
            kind: "shared_track_titles",
            weight: shared_title_weight(pair.shared_titles),
            detail: json!({ "sharedTrackCount": pair.shared_titles }),
        });
    }
    if pair.title_artist_match {
        evidence.push(DuplicateEvidenceDto {
            kind: "same_title_and_display_artist",
            weight: WEIGHT_SAME_TITLE_AND_ARTIST,
            detail: json!({
                "normalizedTitle": left.normalized_title,
                "displayArtist": left.display_artist,
            }),
        });
    } else if pair.title_match {
        evidence.push(DuplicateEvidenceDto {
            kind: "same_normalized_title",
            weight: WEIGHT_SAME_NORMALIZED_TITLE,
            detail: json!({ "normalizedTitle": left.normalized_title }),
        });
    }

    let mut blockers = Vec::new();
    for (provider, left_ids, right_ids) in provider_conflicts(left_external, right_external) {
        blockers.push(DuplicateBlockerDto {
            kind: "trusted_provider_conflict",
            detail: json!({
                "provider": provider,
                "leftExternalIds": left_ids,
                "rightExternalIds": right_ids,
            }),
        });
    }
    if pair.shared_audio == 0 && pair.shared_titles == 0 && !pair.external_match {
        blockers.push(DuplicateBlockerDto {
            kind: "no_shared_recording_or_provider_evidence",
            detail: json!({
                "message": "title similarity alone is not enough to merge Releases",
            }),
        });
    }
    if pair.title_artist_match
        && let (Some(left_date), Some(right_date)) =
            (left.release_date.as_deref(), right.release_date.as_deref())
        && left_date != right_date
    {
        blockers.push(DuplicateBlockerDto {
            kind: "release_date_mismatch",
            detail: json!({ "left": left_date, "right": right_date }),
        });
    }
    if pair.shared_audio == 0
        && pair.shared_titles == 0
        && left.track_count > 0
        && right.track_count > 0
        && left.track_count != right.track_count
    {
        blockers.push(DuplicateBlockerDto {
            kind: "track_count_mismatch",
            detail: json!({
                "left": left.track_count,
                "right": right.track_count,
            }),
        });
    }

    ReleaseCandidateDto {
        score: clamp_score(pair.score),
        left: ReleaseCandidateSideDto {
            id: left.public_api_id,
            title: left.title.clone(),
            display_artist: left.display_artist.clone(),
            release_date: left.release_date.clone(),
            status: left.status.clone(),
            track_count: left.track_count,
        },
        right: ReleaseCandidateSideDto {
            id: right.public_api_id,
            title: right.title.clone(),
            display_artist: right.display_artist.clone(),
            release_date: right.release_date.clone(),
            status: right.status.clone(),
            track_count: right.track_count,
        },
        evidence,
        blockers,
    }
}

fn clamp_score(score: i32) -> u16 {
    u16::try_from(score.clamp(0, 10_000)).unwrap_or(10_000)
}

fn shared_audio_weight(shared_tracks: i64) -> u32 {
    let weighted = shared_tracks
        .max(0)
        .saturating_mul(i64::from(WEIGHT_SHARED_AUDIO_PER_TRACK));
    u32::try_from(weighted.min(i64::from(WEIGHT_SHARED_AUDIO_IDENTITY))).unwrap_or(0)
}

fn shared_title_weight(shared_tracks: i64) -> u32 {
    let weighted = shared_tracks
        .max(0)
        .saturating_mul(i64::from(WEIGHT_SHARED_TRACK_PER_TRACK));
    u32::try_from(weighted.min(i64::from(WEIGHT_SHARED_TRACK_TITLES))).unwrap_or(0)
}

fn overlapping_values(left: &[String], right: &[String]) -> Vec<String> {
    let right = right.iter().collect::<BTreeSet<_>>();
    left.iter()
        .filter(|value| right.contains(value))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn intersect_evidence(
    left: &[ProviderEvidence],
    right: &[ProviderEvidence],
) -> Vec<ProviderEvidence> {
    let right = right.iter().cloned().collect::<BTreeSet<_>>();
    left.iter()
        .filter(|value| right.contains(*value))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Returns one entry per provider where both Artists have trusted observations
/// but no shared external id. A conflicting provider identity is a hard
/// blocker for an automatic merge.
fn provider_conflicts(
    left: &[ProviderEvidence],
    right: &[ProviderEvidence],
) -> Vec<(String, Vec<String>, Vec<String>)> {
    let mut by_provider: BTreeMap<&str, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for value in left {
        by_provider
            .entry(value.provider.as_str())
            .or_default()
            .0
            .insert(value.external_id.as_str());
    }
    for value in right {
        by_provider
            .entry(value.provider.as_str())
            .or_default()
            .1
            .insert(value.external_id.as_str());
    }
    by_provider
        .into_iter()
        .filter_map(|(provider, (left_ids, right_ids))| {
            (!left_ids.is_empty() && !right_ids.is_empty() && left_ids.is_disjoint(&right_ids))
                .then(|| {
                    (
                        provider.to_string(),
                        left_ids.into_iter().map(str::to_string).collect(),
                        right_ids.into_iter().map(str::to_string).collect(),
                    )
                })
        })
        .collect()
}

async fn load_artist_summaries(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, ArtistSummaryRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ArtistSummaryRow>(
        "SELECT id, public_api_id, canonical_name, normalized_name, sort_name,
                artist_type, disambiguation, status
           FROM artists
          WHERE id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

async fn load_artist_aliases(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT artist_id, normalized_alias
           FROM artist_aliases
          WHERE artist_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut values: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (artist_id, value) in rows {
        values.entry(artist_id).or_default().push(value);
    }
    Ok(values)
}

async fn load_artist_external_identities(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<ProviderEvidence>>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT artist_id, provider, external_id
           FROM artist_external_identities
          WHERE artist_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut values: HashMap<Uuid, Vec<ProviderEvidence>> = HashMap::new();
    for (artist_id, provider, external_id) in rows {
        values.entry(artist_id).or_default().push(ProviderEvidence {
            provider,
            external_id,
        });
    }
    Ok(values)
}

async fn load_artist_provider_candidates(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<ProviderEvidence>>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT DISTINCT artist_id, provider, external_id
           FROM provider_identity_candidates
          WHERE artist_id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut values: HashMap<Uuid, Vec<ProviderEvidence>> = HashMap::new();
    for (artist_id, provider, external_id) in rows {
        values.entry(artist_id).or_default().push(ProviderEvidence {
            provider,
            external_id,
        });
    }
    Ok(values)
}

async fn load_release_summaries(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, ReleaseSummaryRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, ReleaseSummaryRow>(
        "SELECT r.id, r.public_api_id, r.title, r.display_artist,
                r.release_date::text AS release_date, r.status,
                COALESCE(aggregate.track_count, 0) AS track_count,
                r.normalized_title
           FROM releases r
           LEFT JOIN release_catalog_aggregates aggregate
             ON aggregate.release_id = r.id
          WHERE r.id = ANY($1)",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

async fn load_release_external_identities(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<ProviderEvidence>>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT release_id, provider, external_id
           FROM release_external_identities
          WHERE release_id = ANY($1)
            AND trusted",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut values: HashMap<Uuid, Vec<ProviderEvidence>> = HashMap::new();
    for (release_id, provider, external_id) in rows {
        values
            .entry(release_id)
            .or_default()
            .push(ProviderEvidence {
                provider,
                external_id,
            });
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_title_weight_is_bounded_and_proportional() {
        assert_eq!(0, shared_title_weight(0));
        assert_eq!(600, shared_title_weight(1));
        assert_eq!(WEIGHT_SHARED_TRACK_TITLES, shared_title_weight(100));
    }

    #[test]
    fn shared_audio_weight_is_bounded_and_proportional() {
        assert_eq!(0, shared_audio_weight(0));
        assert_eq!(700, shared_audio_weight(1));
        assert_eq!(WEIGHT_SHARED_AUDIO_IDENTITY, shared_audio_weight(100));
    }

    #[test]
    fn provider_conflicts_require_disjoint_ids_for_the_same_provider() {
        let left = vec![
            ProviderEvidence {
                provider: "musicbrainz".to_owned(),
                external_id: "a".to_owned(),
            },
            ProviderEvidence {
                provider: "spotify".to_owned(),
                external_id: "shared".to_owned(),
            },
        ];
        let right = vec![
            ProviderEvidence {
                provider: "musicbrainz".to_owned(),
                external_id: "b".to_owned(),
            },
            ProviderEvidence {
                provider: "spotify".to_owned(),
                external_id: "shared".to_owned(),
            },
        ];
        let conflicts = provider_conflicts(&left, &right);
        assert_eq!(1, conflicts.len());
        assert_eq!("musicbrainz", conflicts[0].0);
    }
}
