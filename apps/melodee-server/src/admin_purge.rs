//! Administrator bulk purge for failed, pre-catalog work items.
//!
//! This is deliberately a hard delete, but only for items that failed before a
//! catalog import bound them to an owned Release identity. Storage is removed
//! only below configured roots and only from paths derived from the work item's
//! durable evidence (publication plan, staging handoff, work-item id, and the
//! relative inbound path). The work item, its event log, manifests, settling
//! snapshots, staging handoff, and publication evidence are then deleted in one
//! transaction. One independent `catalog_audit_events` row records the purge so
//! operator accountability survives the deleted processing history.

use std::path::{Component, Path, PathBuf};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use melodee_persistence::{CatalogStore, WorkItemStore, WorkStoreError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};

const MAX_PURGE_BATCH: usize = 100;
const MAX_PURGE_BODY_BYTES: usize = 64 * 1024;

/// One configured inbound root and its optional library id.
#[derive(Debug, Clone)]
pub struct PurgeSourceRoot {
    pub id: Option<Uuid>,
    pub root: PathBuf,
}

#[derive(Clone)]
pub struct AdminPurgeState {
    pub admin: AdminAccessState,
    pub source_roots: Vec<PurgeSourceRoot>,
    pub staging_root: Option<PathBuf>,
    pub work_area_base: PathBuf,
    pub canonical_root: Option<PathBuf>,
}

impl AdminPurgeState {
    #[must_use]
    pub fn new(
        admin: AdminAccessState,
        source_roots: Vec<PurgeSourceRoot>,
        staging_root: Option<PathBuf>,
        work_area_base: PathBuf,
        canonical_root: Option<PathBuf>,
    ) -> Self {
        Self {
            admin,
            source_roots,
            staging_root,
            work_area_base,
            canonical_root,
        }
    }
}

impl axum::extract::FromRequestParts<AdminPurgeState> for AdminPrincipal {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AdminPurgeState,
    ) -> Result<Self, Self::Rejection> {
        <Self as axum::extract::FromRequestParts<AdminAccessState>>::from_request_parts(
            parts,
            &state.admin,
        )
        .await
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeWorkItemsRequest {
    pub work_item_ids: Vec<Uuid>,
    pub reason: String,
    /// Explicit destructive acknowledgement; the UI also requires typed
    /// confirmation before it sends this.
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeWorkItemsResponse {
    pub deleted: Vec<PurgeDeletedItem>,
    pub failed: Vec<PurgeFailedItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeDeletedItem {
    pub id: Uuid,
    pub paths: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeFailedItem {
    pub id: Uuid,
    pub code: &'static str,
    pub message: String,
}

pub fn router() -> Router<AdminPurgeState> {
    Router::new()
        .route(
            "/api/admin/v1/ingest/work-items/purge",
            post(purge_work_items),
        )
        .layer(DefaultBodyLimit::max(MAX_PURGE_BODY_BYTES))
}

fn purge_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
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

async fn purge_work_items(
    State(state): State<AdminPurgeState>,
    admin: AdminPrincipal,
    Json(request): Json<PurgeWorkItemsRequest>,
) -> Response {
    let reason = request.reason.trim().to_string();
    if reason.is_empty() {
        return purge_error(
            StatusCode::BAD_REQUEST,
            "invalid_reason",
            "a non-blank reason is required",
        );
    }
    if !request.confirm {
        return purge_error(
            StatusCode::BAD_REQUEST,
            "confirmation_required",
            "the purge request must carry explicit confirmation",
        );
    }
    if request.work_item_ids.is_empty() || request.work_item_ids.len() > MAX_PURGE_BATCH {
        return purge_error(
            StatusCode::BAD_REQUEST,
            "invalid_batch",
            format!("select between 1 and {MAX_PURGE_BATCH} work items"),
        );
    }

    let mut response = PurgeWorkItemsResponse {
        deleted: Vec::new(),
        failed: Vec::new(),
    };
    for id in request.work_item_ids {
        match purge_one(&state, &admin, id, &reason).await {
            Ok(paths) => response.deleted.push(PurgeDeletedItem { id, paths }),
            Err(failure) => response.failed.push(PurgeFailedItem {
                id,
                code: failure.code,
                message: failure.message,
            }),
        }
    }
    Json(response).into_response()
}

struct PurgeFailure {
    code: &'static str,
    message: String,
}

fn failure(code: &'static str, message: impl Into<String>) -> PurgeFailure {
    PurgeFailure {
        code,
        message: message.into(),
    }
}

async fn purge_one(
    state: &AdminPurgeState,
    admin: &AdminPrincipal,
    id: Uuid,
    reason: &str,
) -> Result<serde_json::Value, PurgeFailure> {
    let item = WorkItemStore
        .get(&state.admin.pool, id)
        .await
        .map_err(|error| match error {
            WorkStoreError::NotFound(_) => failure("not_found", "work item no longer exists"),
            other => failure("store_error", other.to_string()),
        })?;
    if item.state != melodee_domain::IngestionState::Failed {
        return Err(failure(
            "invalid_state",
            format!(
                "only failed work items can be purged; this item is '{}'",
                item.state.as_str()
            ),
        ));
    }
    let import = CatalogStore
        .get_import(&state.admin.pool, item.id)
        .await
        .map_err(|error| failure("store_error", error.to_string()))?;
    if import.is_some() {
        return Err(failure(
            "cataloged",
            "this work item is bound to a catalog Release; use Release Review deletion instead",
        ));
    }

    let publication = WorkItemStore
        .get_publication(&state.admin.pool, item.id)
        .await
        .map_err(|error| failure("store_error", error.to_string()))?;
    let handoff = WorkItemStore
        .staging_handoff(&state.admin.pool, item.id)
        .await
        .map_err(|error| failure("store_error", error.to_string()))?;

    let mut paths = serde_json::Map::new();
    if let Some(publication) = &publication {
        let Some(root) = state.canonical_root.as_deref() else {
            return Err(failure(
                "storage_unavailable",
                "canonical storage is not configured; refusing a partial purge",
            ));
        };
        delete_owned(root, &publication.plan.target_relative_path).await?;
        paths.insert(
            "canonical".to_owned(),
            json!(publication.plan.target_relative_path),
        );
    }
    if let Some(handoff) = &handoff {
        let Some(root) = state.staging_root.as_deref() else {
            return Err(failure(
                "storage_unavailable",
                "staging storage is not configured; refusing a partial purge",
            ));
        };
        delete_owned(root, &handoff.staging_relative_path).await?;
        paths.insert("staging".to_owned(), json!(handoff.staging_relative_path));
    }
    let work_relative = item.id.simple().to_string();
    delete_owned(&state.work_area_base, &work_relative).await?;
    paths.insert("workArea".to_owned(), json!(work_relative));

    // Once a staging handoff exists, the inbound path may have been released
    // for a new arrival; deleting it could remove an unrelated active source.
    // The durable staging tree is the authoritative copy in that case.
    if handoff.is_none()
        && let Some((root, relative)) = inbound_path(state, &item).await?
    {
        delete_owned(&root, &relative).await?;
        paths.insert("inbound".to_owned(), json!(relative));
    }

    WorkItemStore
        .purge_pre_catalog_work_item(
            &state.admin.pool,
            item.id,
            &admin.public_api_id.to_string(),
            reason,
            serde_json::Value::Object(paths.clone()),
            OffsetDateTime::now_utc(),
        )
        .await
        .map_err(|error| match error {
            WorkStoreError::Cataloged(_) => failure(
                "cataloged",
                "a catalog binding appeared while purging; nothing was deleted",
            ),
            WorkStoreError::InvalidState(_, state) => failure(
                "invalid_state",
                format!("work item entered '{}' while purging", state.as_str()),
            ),
            WorkStoreError::NotFound(_) => failure("not_found", "work item no longer exists"),
            other => failure("store_error", other.to_string()),
        })?;
    tracing::info!(
        actor = %admin.public_api_id,
        work_item = %id,
        reason,
        paths = %serde_json::Value::Object(paths.clone()),
        "admin purged failed pre-catalog work item"
    );
    Ok(serde_json::Value::Object(paths))
}

async fn delete_owned(root: &Path, relative: &str) -> Result<(), PurgeFailure> {
    if relative.trim().is_empty() {
        return Ok(());
    }
    let Some(safe_relative) = safe_relative(relative) else {
        return Err(failure(
            "unsafe_path",
            "work-item storage path is not a safe relative directory",
        ));
    };
    melodee_ingestion::delete_release_storage(root, &safe_relative, None, None)
        .await
        .map_err(|error| failure("storage_delete_failed", error.to_string()))?;
    Ok(())
}

fn safe_relative(relative: &str) -> Option<String> {
    let path = Path::new(relative);
    let safe = !relative.trim().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    safe.then(|| relative.to_owned())
}

/// Resolves the inbound root for the item's `current_path`. Multiple roots
/// without a library id are only accepted when exactly one contains the path.
async fn inbound_path(
    state: &AdminPurgeState,
    item: &melodee_domain::ReleaseWorkItem,
) -> Result<Option<(PathBuf, String)>, PurgeFailure> {
    let relative = item.current_path.clone();
    if relative.trim().is_empty() {
        return Ok(None);
    }
    if safe_relative(&relative).is_none() {
        return Err(failure(
            "unsafe_path",
            "the work item's inbound path is not a safe relative directory",
        ));
    }
    if let Some(source_id) = item.source_library_id {
        let matching = state
            .source_roots
            .iter()
            .filter(|root| root.id == Some(source_id))
            .map(|root| root.root.clone())
            .collect::<Vec<_>>();
        return match matching.as_slice() {
            [root] => Ok(Some((root.clone(), relative))),
            [] => Ok(None),
            _ => Err(failure(
                "ambiguous_source_root",
                "multiple inbound roots declare the same library id",
            )),
        };
    }
    if state.source_roots.len() == 1 {
        return Ok(Some((state.source_roots[0].root.clone(), relative)));
    }
    let mut existing = Vec::new();
    for root in &state.source_roots {
        if tokio::fs::try_exists(root.root.join(&relative))
            .await
            .unwrap_or(false)
        {
            existing.push(root.root.clone());
        }
    }
    match existing.as_slice() {
        [] => Ok(None),
        [root] => Ok(Some((root.clone(), relative))),
        _ => Err(failure(
            "ambiguous_source_root",
            "the relative inbound path exists under more than one configured root",
        )),
    }
}
