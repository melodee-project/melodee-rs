//! Active-generation query, autocomplete, and diagnostic services.

use melodee_mql::{CatalogQueryPlan, parse_and_plan_with_bounds};
use melodee_persistence::{SearchProjectionStatus, SearchProjectionStore};
use serde::Serialize;
use sqlx::PgPool;
use tracing::warn;
use uuid::Uuid;

use crate::config::SearchConfig;
use crate::error::SearchError;
use crate::generation::SearchRoot;
use crate::store::{GenerationStats, SearchHit, SearchStore};

#[derive(Debug, Serialize)]
pub struct SearchDiagnostic {
    pub projection: SearchProjectionStatus,
    pub generation: Option<GenerationStats>,
    pub generation_error_code: Option<String>,
    pub generation_error: Option<String>,
}

pub async fn query_active(
    config: &SearchConfig,
    pool: &PgPool,
    input: &str,
    limit: usize,
    offset: usize,
) -> Result<Vec<SearchHit>, SearchError> {
    let plan = parse_and_plan_with_bounds(input, &config.mql_bounds())?;
    query_plan_active(config, pool, plan, limit, offset).await
}

pub async fn query_plan_active(
    config: &SearchConfig,
    pool: &PgPool,
    plan: CatalogQueryPlan,
    limit: usize,
    offset: usize,
) -> Result<Vec<SearchHit>, SearchError> {
    if limit > config.max_limit {
        return Err(SearchError::InvalidQuery(format!(
            "limit exceeds configured maximum {}",
            config.max_limit
        )));
    }
    let path = active_generation_path(config, pool).await?;
    tokio::task::spawn_blocking(move || {
        SearchStore::open_readonly(&path)?.search(&plan, limit, offset)
    })
    .await
    .map_err(|error| SearchError::Retryable(format!("search task failed: {error}")))?
}

/// Query the active projection within one entity kind and return the total
/// count plus a deterministic page.
pub async fn query_active_kind(
    config: &SearchConfig,
    pool: &PgPool,
    input: &str,
    kind: &str,
    limit: usize,
    offset: usize,
) -> Result<(usize, Vec<SearchHit>), SearchError> {
    if limit > config.max_limit {
        return Err(SearchError::InvalidQuery(format!(
            "limit exceeds configured maximum {}",
            config.max_limit
        )));
    }
    let plan = parse_and_plan_with_bounds(input, &config.mql_bounds())?;
    let path = active_generation_path(config, pool).await?;
    let kind = kind.to_string();
    tokio::task::spawn_blocking(move || {
        SearchStore::open_readonly(&path)?.search_kind(&plan, &kind, limit, offset)
    })
    .await
    .map_err(|error| SearchError::Retryable(format!("search task failed: {error}")))?
}

pub async fn autocomplete_active(
    config: &SearchConfig,
    pool: &PgPool,
    kind: &str,
    prefix: &str,
    limit: usize,
) -> Result<Vec<String>, SearchError> {
    let path = active_generation_path(config, pool).await?;
    let kind = kind.to_string();
    let prefix = prefix.to_string();
    tokio::task::spawn_blocking(move || {
        SearchStore::open_readonly(&path)?.autocomplete(&kind, &prefix, limit)
    })
    .await
    .map_err(|error| SearchError::Retryable(format!("autocomplete task failed: {error}")))?
}

/// Resolve an Artist display name, alias, internal ID, or public ID to a
/// bounded deterministic list of internal IDs in the active generation.
/// This is the admin-lookup boundary used by later API/UI phases; identity is
/// never inferred from the returned display string.
pub async fn resolve_artist_ids_active(
    config: &SearchConfig,
    pool: &PgPool,
    value: &str,
) -> Result<Vec<String>, SearchError> {
    if value.trim().is_empty() || value.len() > melodee_mql::Bounds::default().max_input_len {
        return Err(SearchError::InvalidQuery(
            "Artist lookup must be nonblank and within the MQL input bound".to_string(),
        ));
    }
    if let Ok(artist_uuid) = Uuid::parse_str(value)
        && let Some(redirected) = sqlx::query_scalar::<_, Uuid>(
            "SELECT resolve_artist_redirect(artist.id)
             FROM artists artist
             WHERE artist.id = $1 OR artist.public_api_id = $1
             LIMIT 1",
        )
        .bind(artist_uuid)
        .fetch_optional(pool)
        .await?
    {
        return Ok(vec![redirected.to_string()]);
    }
    let path = active_generation_path(config, pool).await?;
    let value = value.to_string();
    tokio::task::spawn_blocking(move || {
        SearchStore::open_readonly(&path)?.resolve_artist_ids(&value)
    })
    .await
    .map_err(|error| SearchError::Retryable(format!("Artist lookup task failed: {error}")))?
}

pub async fn diagnostic(
    config: &SearchConfig,
    pool: &PgPool,
) -> Result<SearchDiagnostic, SearchError> {
    let root = SearchRoot::open(&config.search_root)?;
    let projection = SearchProjectionStore::status(
        pool,
        melodee_persistence::SEARCH_PROJECTION_NAME,
        root.path().to_string_lossy().as_ref(),
        time::OffsetDateTime::now_utc(),
    )
    .await?;
    let active =
        SearchProjectionStore::active_generation(pool, root.path().to_string_lossy().as_ref())
            .await?;
    let Some(active) = active else {
        return Ok(SearchDiagnostic {
            projection,
            generation: None,
            generation_error_code: Some("search.no_active_generation".to_string()),
            generation_error: Some("no active search generation".to_string()),
        });
    };
    let path = match validate_active_manifest(&root, &active) {
        Ok(path) => path,
        Err(error) => {
            warn!(error = ?error, generation_id = %active.generation_id, "active search generation manifest validation failed");
            return Ok(SearchDiagnostic {
                projection,
                generation: None,
                generation_error_code: Some(error.reason_code().to_string()),
                generation_error: Some(error.operator_message().to_string()),
            });
        }
    };
    let result = tokio::task::spawn_blocking(move || SearchStore::open_readonly(&path)?.stats())
        .await
        .map_err(|error| SearchError::Retryable(format!("diagnostic task failed: {error}")))?;
    match result {
        Ok(generation)
            if generation.schema_version == u32::try_from(active.schema_version).unwrap_or(0)
                && generation.artists as u64 == active.current_document_counts.artists
                && generation.releases as u64 == active.current_document_counts.releases
                && generation.tracks as u64 == active.current_document_counts.tracks =>
        {
            Ok(SearchDiagnostic {
                projection,
                generation: Some(generation),
                generation_error_code: None,
                generation_error: None,
            })
        }
        Ok(generation) => {
            warn!(
                generation_id = %active.generation_id,
                database_counts = ?active.current_document_counts,
                index_stats = ?generation,
                "active search generation metadata does not match its index"
            );
            Ok(SearchDiagnostic {
                projection,
                generation: None,
                generation_error_code: Some("search.generation_metadata_mismatch".to_string()),
                generation_error: Some(
                    "active search generation metadata does not match its index".to_string(),
                ),
            })
        }
        Err(error) => {
            warn!(error = ?error, generation_id = %active.generation_id, "active search generation could not be inspected");
            Ok(SearchDiagnostic {
                projection,
                generation: None,
                generation_error_code: Some(error.reason_code().to_string()),
                generation_error: Some(error.operator_message().to_string()),
            })
        }
    }
}

async fn active_generation_path(
    config: &SearchConfig,
    pool: &PgPool,
) -> Result<std::path::PathBuf, SearchError> {
    let root = SearchRoot::open(&config.search_root)?;
    let active =
        SearchProjectionStore::active_generation(pool, root.path().to_string_lossy().as_ref())
            .await?
            .ok_or_else(|| SearchError::NoActiveGeneration(root.path().display().to_string()))?;
    validate_active_manifest(&root, &active)
}

fn validate_active_manifest(
    root: &SearchRoot,
    active: &melodee_persistence::StoredSearchGeneration,
) -> Result<std::path::PathBuf, SearchError> {
    let manifest = root.read_manifest(active.generation_id)?;
    let serialized_manifest = serde_json::to_vec(&manifest)?;
    let manifest_hash = blake3::hash(&serialized_manifest).to_hex().to_string();
    if manifest.schema_version != crate::schema::SCHEMA_VERSION
        || active.schema_version
            != i32::try_from(crate::schema::SCHEMA_VERSION).map_err(|_| {
                SearchError::IndexOpen("schema version cannot be represented".to_string())
            })?
        || manifest.document_counts != active.document_counts
        || manifest.source_high_water_mark != active.source_high_water_mark
        || active.manifest_hash.as_deref() != Some(manifest_hash.as_str())
    {
        return Err(SearchError::IndexOpen(
            "active generation manifest does not match durable metadata".to_string(),
        ));
    }
    Ok(root.generation_path(active.generation_id))
}
