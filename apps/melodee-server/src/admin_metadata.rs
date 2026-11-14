//! Safe, release-scoped metadata transformations (ADR-0157/0164).
//!
//! The route is deliberately an admin surface. It previews named operations,
//! writes only same-directory temporary copies, verifies decoded audio before
//! replacement, retains a recovery copy for revert, and records the result in
//! PostgreSQL. No operation changes a path, extension, or catalog identity.

use std::{
    fs::{self, File},
    path::{Component, Path, PathBuf},
};

use crate::admin_time::AdminTimestamp;
use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_ingestion::IngestionConfig;
use melodee_media::{
    TagRewrite, TagRewriteError, TagSnapshot, apply_tag_rewrite, decoded_audio_fingerprint,
    read_tag_snapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};

const MAX_FILES_PER_RELEASE: usize = 2_000;
const MAX_TRANSFORM_PARAMS: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
enum MetadataEditError {
    #[error("invalid metadata transformation request")]
    InvalidRequest,
    #[error("release or media file was not found")]
    NotFound,
    #[error("metadata transformation is not configured: {0}")]
    Configuration(String),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("media file operation failed")]
    Io(#[from] std::io::Error),
    #[error("media tag operation failed")]
    Tag(#[from] TagRewriteError),
    #[error("media integrity verification failed: {0}")]
    Integrity(String),
    #[error("preview no longer matches current media")]
    PreviewMismatch,
    #[error("metadata transformation failed: {0}")]
    Operation(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransformRequest {
    operation: String,
    #[serde(default)]
    params: Value,
    preview_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct FieldDelta {
    file: String,
    field: String,
    old: Option<String>,
    new: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewResponse {
    preview_hash: String,
    operation: String,
    affected_files: usize,
    deltas: Vec<FieldDelta>,
    validation_errors: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransformResponse {
    audit_id: Uuid,
    status: String,
    preview_hash: String,
    affected_files: usize,
    deltas: Vec<FieldDelta>,
    recovery: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
    limit: Option<u32>,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct AuditRow {
    id: Uuid,
    release_id: Uuid,
    operation_name: String,
    operation_params: Value,
    affected_files: i32,
    field_deltas: Value,
    applied_by: String,
    status: String,
    recovery_evidence: Value,
    created_at: AdminTimestamp,
    completed_at: Option<AdminTimestamp>,
}

#[derive(Debug, Clone, FromRow)]
struct ReleaseFile {
    release_id: Uuid,
    media_asset_id: Uuid,
    relative_path: String,
    ordinal: i32,
}

#[derive(Debug, Clone)]
struct PlannedFile {
    file: ReleaseFile,
    path: PathBuf,
    rewrite: TagRewrite,
    deltas: Vec<FieldDelta>,
}

pub fn router(config: IngestionConfig) -> Router<AdminAccessState> {
    let master = std::sync::Arc::new(config);
    let mut router = Router::new();
    let config = master.clone();
    router = router.route(
        "/api/admin/v1/catalog/releases/{id}/metadata-transforms/preview",
        post(
            move |state: State<AdminAccessState>,
                  _admin: AdminPrincipal,
                  id: AxumPath<Uuid>,
                  request: Json<TransformRequest>| async move {
                preview(state, _admin, id, request, &config).await
            },
        ),
    );
    let config = master.clone();
    router = router.route(
        "/api/admin/v1/catalog/releases/{id}/metadata-transforms/apply",
        post(
            move |state: State<AdminAccessState>,
                  admin: AdminPrincipal,
                  id: AxumPath<Uuid>,
                  request: Json<TransformRequest>| async move {
                apply(state, admin, id, request, &config).await
            },
        ),
    );
    let config = master.clone();
    router = router.route(
        "/api/admin/v1/catalog/releases/{id}/metadata-transforms",
        get(
            move |state: State<AdminAccessState>,
                  _admin: AdminPrincipal,
                  id: AxumPath<Uuid>,
                  query: Query<AuditQuery>| async move {
                list_audits(state, _admin, id, query, &config).await
            },
        ),
    );
    let config = master.clone();
    router = router.route(
        "/api/admin/v1/catalog/releases/{id}/metadata-transforms/{audit_id}/revert",
        post(
            move |state: State<AdminAccessState>,
                  admin: AdminPrincipal,
                  id: AxumPath<(Uuid, Uuid)>| async move {
                revert(state, admin, id, &config).await
            },
        ),
    );
    router
}

fn error_response(error: MetadataEditError) -> Response {
    let (status, code, message) = match error {
        MetadataEditError::InvalidRequest | MetadataEditError::PreviewMismatch => (
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "metadata transformation request is invalid",
        ),
        MetadataEditError::NotFound => (
            StatusCode::NOT_FOUND,
            "not_found",
            "release or metadata audit was not found",
        ),
        MetadataEditError::Configuration(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "not_configured",
            "metadata transformation is not configured",
        ),
        MetadataEditError::Integrity(_) | MetadataEditError::Operation(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "transformation_failed",
            "metadata transformation failed validation",
        ),
        MetadataEditError::Database(_) | MetadataEditError::Io(_) | MetadataEditError::Tag(_) => {
            eprintln!("DEBUG METADATA ERROR: {:?}", error);
            tracing::error!(error = ?error, "metadata transformation failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "store_error",
                "metadata transformation failed",
            )
        }
    };
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            message: message.to_owned(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

async fn preview(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    AxumPath(id): AxumPath<Uuid>,
    Json(request): Json<TransformRequest>,
    config: &IngestionConfig,
) -> Response {
    match build_plan(&state.pool, id, &request, config).await {
        Ok((hash, plan)) => Json(PreviewResponse {
            preview_hash: hash,
            operation: request.operation,
            affected_files: plan.iter().filter(|file| !file.deltas.is_empty()).count(),
            deltas: flatten_deltas(&plan),
            validation_errors: Vec::new(),
        })
        .into_response(),
        Err(error) => error_response(error),
    }
}

async fn apply(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    AxumPath(id): AxumPath<Uuid>,
    Json(request): Json<TransformRequest>,
    config: &IngestionConfig,
) -> Response {
    match apply_inner(
        &state.pool,
        id,
        &request,
        admin.public_api_id.to_string(),
        config,
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn list_audits(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    AxumPath(id): AxumPath<Uuid>,
    Query(query): Query<AuditQuery>,
    _config: &IngestionConfig,
) -> Response {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let result = sqlx::query_as::<_, AuditRow>(
        "SELECT a.id,a.release_id,a.operation_name,a.operation_params,a.affected_files,
                a.field_deltas,a.applied_by,a.status,a.recovery_evidence,a.created_at,a.completed_at
         FROM metadata_edit_audits a JOIN releases r ON r.id=a.release_id
         WHERE r.public_api_id=$1 ORDER BY a.created_at DESC,a.id DESC LIMIT $2",
    )
    .bind(id)
    .bind(i64::from(limit))
    .fetch_all(&state.pool)
    .await;
    match result {
        Ok(rows) => Json(rows).into_response(),
        Err(error) => error_response(MetadataEditError::Database(error)),
    }
}

async fn revert(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    AxumPath((release_public_id, audit_id)): AxumPath<(Uuid, Uuid)>,
    config: &IngestionConfig,
) -> Response {
    match revert_inner(
        &state.pool,
        release_public_id,
        audit_id,
        admin.public_api_id.to_string(),
        config,
    )
    .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => error_response(error),
    }
}

async fn apply_inner(
    pool: &PgPool,
    release_public_id: Uuid,
    request: &TransformRequest,
    actor: String,
    config: &IngestionConfig,
) -> Result<TransformResponse, MetadataEditError> {
    let params = bounded_params(&request.params)?;
    let (preview_hash, plan) = build_plan(pool, release_public_id, request, config).await?;
    if request.preview_hash.as_deref() != Some(preview_hash.as_str()) {
        return Err(MetadataEditError::PreviewMismatch);
    }
    let audit_id = Uuid::now_v7();
    let deltas = flatten_deltas(&plan);
    let release_id = plan
        .first()
        .map(|file| file.file.release_id)
        .ok_or(MetadataEditError::NotFound)?;
    sqlx::query(
        "INSERT INTO metadata_edit_audits
         (id,release_id,operation_name,operation_params,affected_files,field_deltas,applied_by,status,created_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,'applying',$8)",
    )
    .bind(audit_id)
    .bind(release_id)
    .bind(&request.operation)
    .bind(params)
    .bind(i32::try_from(plan.iter().filter(|file| !file.deltas.is_empty()).count()).unwrap_or(i32::MAX))
    .bind(json!(deltas))
    .bind(&actor)
    .bind(OffsetDateTime::now_utc())
    .execute(pool)
    .await?;

    let result = apply_files_and_commit(
        pool,
        release_id,
        audit_id,
        &request.operation,
        &plan,
        actor,
        config,
    )
    .await;
    match result {
        Ok(recovery) => Ok(TransformResponse {
            audit_id,
            status: "success".to_owned(),
            preview_hash,
            affected_files: plan.iter().filter(|file| !file.deltas.is_empty()).count(),
            deltas,
            recovery,
        }),
        Err(error) => {
            let _ = sqlx::query(
                "UPDATE metadata_edit_audits SET status='failed',completed_at=$2,recovery_evidence=$3 WHERE id=$1",
            )
            .bind(audit_id)
            .bind(OffsetDateTime::now_utc())
            .bind(json!({"error":"transformation failed; originals restored"}))
            .execute(pool)
            .await;
            Err(error)
        }
    }
}

async fn apply_files_and_commit(
    pool: &PgPool,
    release_id: Uuid,
    audit_id: Uuid,
    operation: &str,
    plan: &[PlannedFile],
    actor: String,
    config: &IngestionConfig,
) -> Result<Value, MetadataEditError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(release_id.to_string())
        .execute(&mut *tx)
        .await?;
    let ffmpeg = &config.ffmpeg_path;
    let timeout = std::time::Duration::from_secs(
        u64::try_from(config.ffmpeg_timeout.whole_seconds()).unwrap_or(300),
    );
    let mut evidence = Vec::new();
    let mut changed = Vec::new();
    for item in plan.iter().filter(|file| !file.deltas.is_empty()) {
        let record = match atomic_rewrite(item, audit_id, ffmpeg, timeout) {
            Ok(record) => record,
            Err(error) => {
                rollback_files(&evidence, config);
                tx.rollback().await?;
                return Err(error);
            }
        };
        evidence.push(json!(&record));
        if let Err(error) =
            sqlx::query("UPDATE media_assets SET byte_hash=$2,byte_size=$3 WHERE id=$1")
                .bind(item.file.media_asset_id)
                .bind(&record.after_hash)
                .bind(record.after_size)
                .execute(&mut *tx)
                .await
        {
            rollback_files(&evidence, config);
            tx.rollback().await?;
            return Err(MetadataEditError::Database(error));
        }
        changed.push(record.relative_path.clone());
    }

    if operation == "set_year"
        && let Some(year) = plan
            .iter()
            .flat_map(|file| file.rewrite.recording_date.as_deref())
            .next()
        && let Err(error) = sqlx::query("UPDATE releases SET release_date_text=$2,revision=revision+1,updated_at=$3 WHERE id=$1")
            .bind(release_id)
            .bind(year)
            .bind(OffsetDateTime::now_utc())
            .execute(&mut *tx)
            .await
    {
        rollback_files(&evidence, config);
        tx.rollback().await?;
        return Err(MetadataEditError::Database(error));
    }
    let recovery = json!({"files": evidence, "actor": actor, "changed": changed});
    if let Err(error) = sqlx::query(
        "UPDATE metadata_edit_audits SET status='success',completed_at=$2,recovery_evidence=$3 WHERE id=$1",
    )
    .bind(audit_id)
    .bind(OffsetDateTime::now_utc())
    .bind(&recovery)
    .execute(&mut *tx)
    .await
    {
        rollback_files(&evidence, config);
        tx.rollback().await?;
        return Err(MetadataEditError::Database(error));
    }
    if let Err(error) = tx.commit().await {
        rollback_files(&evidence, config);
        let _ = sqlx::query(
            "UPDATE metadata_edit_audits SET status='failed',completed_at=$2,recovery_evidence=$3 WHERE id=$1",
        )
        .bind(audit_id)
        .bind(OffsetDateTime::now_utc())
        .bind(json!({"error":"database commit failed; originals restored"}))
        .execute(pool)
        .await;
        return Err(MetadataEditError::Database(error));
    }
    Ok(recovery)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileEvidence {
    relative_path: String,
    backup_name: String,
    before_hash: String,
    after_hash: String,
    before_audio_fingerprint: String,
    after_audio_fingerprint: String,
    after_size: i64,
}

fn atomic_rewrite(
    item: &PlannedFile,
    audit_id: Uuid,
    ffmpeg: &Path,
    timeout: std::time::Duration,
) -> Result<FileEvidence, MetadataEditError> {
    let parent = item.path.parent().ok_or(MetadataEditError::NotFound)?;
    let file_name = item
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(MetadataEditError::InvalidRequest)?;
    let temp_name = format!(".{file_name}.melodee-edit-{audit_id}.wav");
    let backup_name = format!(".{file_name}.melodee-edit-{audit_id}.original");
    let temp = parent.join(&temp_name);
    let backup = parent.join(&backup_name);
    fs::copy(&item.path, &temp)?;
    let result = (|| {
        let _ = apply_tag_rewrite(&temp, &item.rewrite)?;
        // Re-open the rewritten container before any rename. This catches a
        // writer that returned success but produced a file the normal reader
        // cannot parse.
        let _ = read_tag_snapshot(&temp)?;
        File::open(&temp)?.sync_all()?;
        let before_audio = decoded_audio_fingerprint(&item.path, ffmpeg, timeout)
            .map_err(MetadataEditError::Integrity)?;
        let after_audio = decoded_audio_fingerprint(&temp, ffmpeg, timeout)
            .map_err(MetadataEditError::Integrity)?;
        if before_audio != after_audio {
            return Err(MetadataEditError::Integrity(
                "decoded audio fingerprint changed".to_owned(),
            ));
        }
        let before_hash = sha256_file(&item.path)?;
        let after_hash = sha256_file(&temp)?;
        if before_hash == after_hash {
            fs::remove_file(&temp)?;
            return Ok(FileEvidence {
                relative_path: item.file.relative_path.clone(),
                backup_name: String::new(),
                before_hash,
                after_hash,
                before_audio_fingerprint: before_audio.clone(),
                after_audio_fingerprint: after_audio,
                after_size: i64::try_from(fs::metadata(&item.path)?.len()).unwrap_or(i64::MAX),
            });
        }
        fs::rename(&item.path, &backup)?;
        if let Err(error) = fs::rename(&temp, &item.path) {
            let _ = fs::rename(&backup, &item.path);
            return Err(MetadataEditError::Io(error));
        }
        sync_directory(parent)?;
        Ok(FileEvidence {
            relative_path: item.file.relative_path.clone(),
            backup_name,
            before_hash,
            after_hash,
            before_audio_fingerprint: before_audio,
            after_audio_fingerprint: after_audio,
            after_size: i64::try_from(fs::metadata(&item.path)?.len()).unwrap_or(i64::MAX),
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
        // Never discard a recovery copy unless it has been restored to the
        // original pathname. If the restore itself fails, the backup is the
        // only remaining copy of the source media.
        if backup.is_file() && !item.path.exists() {
            let _ = fs::rename(&backup, &item.path);
            let _ = sync_directory(parent);
        }
    }
    result
}

fn rollback_files(evidence: &[Value], config: &IngestionConfig) {
    for value in evidence {
        let Some(relative) = value.get("relativePath").and_then(Value::as_str) else {
            continue;
        };
        let Some(backup_name) = value.get("backupName").and_then(Value::as_str) else {
            continue;
        };
        let Ok(Some(roots)) = config.storage_roots().map(|roots| roots.storage_root) else {
            continue;
        };
        let path = roots.join(relative);
        if backup_name.is_empty() {
            continue;
        }
        let backup = path.parent().map(|parent| parent.join(backup_name));
        if let Some(backup) = backup {
            let Some(parent) = path.parent() else {
                continue;
            };
            if !backup.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let current = parent.join(format!(".{file_name}.melodee-rollback-current"));
            if fs::rename(&path, &current).is_err() {
                continue;
            }
            if fs::rename(&backup, &path).is_err() {
                let _ = fs::rename(&current, &path);
                continue;
            }
            let _ = fs::remove_file(current);
            let _ = sync_directory(parent);
        }
    }
}

fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    File::open(path)?.sync_all()
}

#[derive(Debug)]
struct RevertReplacement {
    path: PathBuf,
    transformed_copy: PathBuf,
}

/// Puts every file back into its pre-revert state. The transformed copy is
/// retained until the database transaction commits, so a database or audit
/// write failure cannot leave the filesystem ahead of PostgreSQL.
fn restore_revert_files(replacements: &[RevertReplacement]) {
    for replacement in replacements {
        restore_one_revert_file(&replacement.path, &replacement.transformed_copy);
    }
}

fn restore_one_revert_file(path: &Path, transformed_copy: &Path) {
    let Some(parent) = path.parent() else { return };
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let displaced = parent.join(format!(".{name}.melodee-revert-displaced"));
    if fs::rename(path, &displaced).is_err() {
        return;
    }
    if fs::rename(transformed_copy, path).is_err() {
        let _ = fs::rename(&displaced, path);
        return;
    }
    let _ = fs::remove_file(displaced);
    let _ = sync_directory(parent);
}

async fn revert_inner(
    pool: &PgPool,
    release_public_id: Uuid,
    audit_id: Uuid,
    actor: String,
    config: &IngestionConfig,
) -> Result<TransformResponse, MetadataEditError> {
    let audit = sqlx::query_as::<_, AuditRow>(
        "SELECT a.id,a.release_id,a.operation_name,a.operation_params,a.affected_files,
                a.field_deltas,a.applied_by,a.status,a.recovery_evidence,a.created_at,a.completed_at
         FROM metadata_edit_audits a JOIN releases r ON r.id=a.release_id
         WHERE a.id=$1 AND r.public_api_id=$2",
    )
    .bind(audit_id)
    .bind(release_public_id)
    .fetch_optional(pool)
    .await?
    .ok_or(MetadataEditError::NotFound)?;
    if audit.status != "success" {
        return Err(MetadataEditError::Operation(
            "only a successful transformation can be reverted".to_owned(),
        ));
    }
    let files = audit
        .recovery_evidence
        .get("files")
        .and_then(Value::as_array)
        .ok_or(MetadataEditError::Operation(
            "recovery evidence is incomplete".to_owned(),
        ))?;
    let roots = config
        .storage_roots()
        .map_err(|error| MetadataEditError::Configuration(error.to_string()))?
        .storage_root
        .ok_or_else(|| {
            MetadataEditError::Configuration("canonical storage root is absent".to_owned())
        })?;
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(audit.release_id.to_string())
        .execute(&mut *tx)
        .await?;
    let ffmpeg = &config.ffmpeg_path;
    let timeout = std::time::Duration::from_secs(
        u64::try_from(config.ffmpeg_timeout.whole_seconds()).unwrap_or(300),
    );
    let mut deltas = Vec::new();
    let mut replacements = Vec::new();
    for value in files {
        let file_result =
            (|| -> Result<(String, PathBuf, PathBuf, i64, String), MetadataEditError> {
                let relative = value.get("relativePath").and_then(Value::as_str).ok_or(
                    MetadataEditError::Operation("recovery path is missing".to_owned()),
                )?;
                let backup_name = value
                    .get("backupName")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .ok_or(MetadataEditError::Operation(
                        "recovery backup is missing".to_owned(),
                    ))?;
                let path = checked_path(&roots, relative)?;
                let parent = path
                    .parent()
                    .ok_or(MetadataEditError::NotFound)?
                    .to_path_buf();
                let backup = parent.join(backup_name);
                if !backup.is_file() {
                    return Err(MetadataEditError::Operation(
                        "recovery backup is missing on disk".to_owned(),
                    ));
                }
                let expected_hash = value.get("beforeHash").and_then(Value::as_str).ok_or(
                    MetadataEditError::Operation("recovery hash is missing".to_owned()),
                )?;
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or(MetadataEditError::InvalidRequest)?
                    .to_owned();
                let temp = parent.join(format!(".{name}.melodee-revert-{audit_id}.tmp"));
                fs::copy(&backup, &temp)?;
                let result = (|| {
                    File::open(&temp)?.sync_all()?;
                    if sha256_file(&temp)? != expected_hash {
                        return Err(MetadataEditError::Integrity(
                            "recovery copy hash does not match the audit record".to_owned(),
                        ));
                    }
                    if let Some(expected_audio) =
                        value.get("beforeAudioFingerprint").and_then(Value::as_str)
                    {
                        let actual = decoded_audio_fingerprint(&temp, ffmpeg, timeout)
                            .map_err(MetadataEditError::Integrity)?;
                        if actual != expected_audio {
                            return Err(MetadataEditError::Integrity(
                                "recovery copy audio fingerprint does not match the audit record"
                                    .to_owned(),
                            ));
                        }
                    }
                    let transformed_copy =
                        parent.join(format!(".{name}.melodee-revert-{audit_id}.current"));
                    fs::rename(&path, &transformed_copy)?;
                    if let Err(error) = fs::rename(&temp, &path) {
                        let _ = fs::rename(&transformed_copy, &path);
                        return Err(MetadataEditError::Io(error));
                    }
                    sync_directory(&parent)?;
                    let hash = sha256_file(&path)?;
                    let size = i64::try_from(fs::metadata(&path)?.len()).unwrap_or(i64::MAX);
                    Ok((relative.to_owned(), path, transformed_copy, size, hash))
                })();
                if result.is_err() {
                    let _ = fs::remove_file(&temp);
                }
                result
            })();
        let (relative, path, transformed_copy, size, hash) = match file_result {
            Ok(value) => value,
            Err(error) => {
                restore_revert_files(&replacements);
                let _ = tx.rollback().await;
                return Err(error);
            }
        };
        let media_id: Uuid = match sqlx::query_scalar(
            "SELECT m.id FROM media_assets m WHERE m.canonical_relative_path=$1",
        )
        .bind(&relative)
        .fetch_optional(&mut *tx)
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => {
                restore_revert_files(&replacements);
                restore_one_revert_file(&path, &transformed_copy);
                let _ = tx.rollback().await;
                return Err(MetadataEditError::NotFound);
            }
            Err(error) => {
                restore_revert_files(&replacements);
                restore_one_revert_file(&path, &transformed_copy);
                let _ = tx.rollback().await;
                return Err(MetadataEditError::Database(error));
            }
        };
        if let Err(error) =
            sqlx::query("UPDATE media_assets SET byte_hash=$2,byte_size=$3 WHERE id=$1")
                .bind(media_id)
                .bind(hash)
                .bind(size)
                .execute(&mut *tx)
                .await
        {
            restore_revert_files(&replacements);
            restore_one_revert_file(&path, &transformed_copy);
            let _ = tx.rollback().await;
            return Err(MetadataEditError::Database(error));
        }
        replacements.push(RevertReplacement {
            path,
            transformed_copy,
        });
        deltas.push(json!({"file":relative,"field":"revert","old":"transformed","new":"original"}));
    }
    let recovery = json!({"revertedBy": actor, "files": files});
    if let Err(error) = sqlx::query("UPDATE metadata_edit_audits SET status='reverted',completed_at=$2,recovery_evidence=$3 WHERE id=$1")
        .bind(audit_id)
        .bind(OffsetDateTime::now_utc())
        .bind(&recovery)
        .execute(&mut *tx)
        .await
    {
        restore_revert_files(&replacements);
        let _ = tx.rollback().await;
        return Err(MetadataEditError::Database(error));
    }
    if let Err(error) = tx.commit().await {
        restore_revert_files(&replacements);
        return Err(MetadataEditError::Database(error));
    }
    for replacement in replacements {
        let _ = fs::remove_file(&replacement.transformed_copy);
        if let Some(parent) = replacement.path.parent() {
            let _ = sync_directory(parent);
        }
    }
    Ok(TransformResponse {
        audit_id,
        status: "reverted".to_owned(),
        preview_hash: preview_hash_for(&audit.operation_name, &audit.operation_params, &[]),
        affected_files: files.len(),
        deltas: deltas
            .into_iter()
            .filter_map(|value| serde_json::from_value(value).ok())
            .collect(),
        recovery,
    })
}

async fn build_plan(
    pool: &PgPool,
    release_public_id: Uuid,
    request: &TransformRequest,
    config: &IngestionConfig,
) -> Result<(String, Vec<PlannedFile>), MetadataEditError> {
    let params = bounded_params(&request.params)?;
    let roots = config
        .storage_roots()
        .map_err(|error| MetadataEditError::Configuration(error.to_string()))?
        .storage_root
        .ok_or_else(|| {
            MetadataEditError::Configuration("canonical storage root is absent".to_owned())
        })?;
    let rows = sqlx::query_as::<_, ReleaseFile>(
        "SELECT r.id AS release_id,m.id AS media_asset_id,
                m.canonical_relative_path AS relative_path,t.ordinal
         FROM releases r JOIN tracks t ON t.release_id=r.id
         JOIN media_assets m ON m.id=t.playable_media_asset_id
         WHERE r.public_api_id=$1 ORDER BY t.ordinal,t.id LIMIT $2",
    )
    .bind(release_public_id)
    .bind(i64::try_from(MAX_FILES_PER_RELEASE + 1).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Err(MetadataEditError::NotFound);
    }
    if rows.len() > MAX_FILES_PER_RELEASE {
        return Err(MetadataEditError::InvalidRequest);
    }
    let mut plan = Vec::with_capacity(rows.len());
    for file in rows {
        let path = checked_path(&roots, &file.relative_path)?;
        let before = read_tag_snapshot(&path)?;
        let rewrite = rewrite_for(&request.operation, &params, &before, file.ordinal)?;
        let after = projected_snapshot(&before, &rewrite);
        let deltas = snapshot_deltas(&file.relative_path, &before, &after);
        plan.push(PlannedFile {
            file,
            path,
            rewrite,
            deltas,
        });
    }
    let hash = preview_hash_for(&request.operation, &params, &flatten_deltas(&plan));
    Ok((hash, plan))
}

fn bounded_params(params: &Value) -> Result<Value, MetadataEditError> {
    let bytes = serde_json::to_vec(params).map_err(|_| MetadataEditError::InvalidRequest)?;
    if bytes.len() > MAX_TRANSFORM_PARAMS {
        return Err(MetadataEditError::InvalidRequest);
    }
    if !params.is_object() && !params.is_null() {
        return Err(MetadataEditError::InvalidRequest);
    }
    Ok(params.clone())
}

fn rewrite_for(
    operation: &str,
    params: &Value,
    snapshot: &TagSnapshot,
    ordinal: i32,
) -> Result<TagRewrite, MetadataEditError> {
    let value = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    };
    match operation {
        "remove_featuring_artists" | "remove_featuring_from_song_titles" => Ok(TagRewrite {
            title: snapshot.title.as_deref().map(clean_featuring),
            ..TagRewrite::default()
        }),
        "remove_featuring_from_song_artists" => Ok(TagRewrite {
            artist: snapshot.artist.as_deref().map(clean_featuring),
            ..TagRewrite::default()
        }),
        "set_artist_on_all_songs" => Ok(TagRewrite {
            album_artist: Some(value("artist").ok_or(MetadataEditError::InvalidRequest)?),
            ..TagRewrite::default()
        }),
        "promote_song_artist" => Ok(TagRewrite {
            album_artist: Some(value("artist").ok_or(MetadataEditError::InvalidRequest)?),
            clear_artist: true,
            ..TagRewrite::default()
        }),
        "demote_song_artist" => Ok(TagRewrite {
            clear_artist: true,
            ..TagRewrite::default()
        }),
        "remove_all_song_artists" => Ok(TagRewrite {
            clear_artist: true,
            clear_album_artist: true,
            ..TagRewrite::default()
        }),
        "set_year" => Ok(TagRewrite {
            recording_date: Some(valid_year(
                &value("year").ok_or(MetadataEditError::InvalidRequest)?,
            )?),
            ..TagRewrite::default()
        }),
        "renumber_songs" | "renumber_tracks" => Ok(TagRewrite {
            track_number: Some(
                u32::try_from(ordinal).map_err(|_| MetadataEditError::InvalidRequest)?,
            ),
            ..TagRewrite::default()
        }),
        "remove_unwanted_album_title" => Ok(TagRewrite {
            album: snapshot.album.as_deref().map(clean_unwanted_album_title),
            ..TagRewrite::default()
        }),
        "remove_unwanted_song_titles" => Ok(TagRewrite {
            title: snapshot.title.as_deref().map(clean_unwanted_song_title),
            ..TagRewrite::default()
        }),
        "replace_song_titles" | "replace_text_in_title" => {
            let from = value("from").ok_or(MetadataEditError::InvalidRequest)?;
            let to = value("to").unwrap_or_default();
            if from.is_empty() {
                return Err(MetadataEditError::InvalidRequest);
            }
            Ok(TagRewrite {
                title: snapshot
                    .title
                    .as_deref()
                    .map(|current| current.replace(&from, &to)),
                ..TagRewrite::default()
            })
        }
        "replace_song_artists" | "replace_text_in_artist" => {
            let from = value("from").ok_or(MetadataEditError::InvalidRequest)?;
            let to = value("to").unwrap_or_default();
            if from.is_empty() {
                return Err(MetadataEditError::InvalidRequest);
            }
            Ok(TagRewrite {
                artist: snapshot
                    .artist
                    .as_deref()
                    .map(|current| current.replace(&from, &to)),
                ..TagRewrite::default()
            })
        }
        "replace_song_artist_separators" => Ok(TagRewrite {
            artist: snapshot.artist.as_deref().map(replace_artist_separators),
            ..TagRewrite::default()
        }),
        _ => Err(MetadataEditError::Operation(
            "operation is not supported".to_owned(),
        )),
    }
}

fn projected_snapshot(before: &TagSnapshot, rewrite: &TagRewrite) -> TagSnapshot {
    let mut after = before.clone();
    if let Some(value) = &rewrite.title {
        after.title = Some(value.clone());
    }
    if let Some(value) = &rewrite.artist {
        after.artist = Some(value.clone());
    }
    if rewrite.clear_artist {
        after.artist = None;
    }
    if let Some(value) = &rewrite.album_artist {
        after.album_artist = Some(value.clone());
    }
    if rewrite.clear_album_artist {
        after.album_artist = None;
    }
    if let Some(value) = &rewrite.album {
        after.album = Some(value.clone());
    }
    if let Some(value) = &rewrite.recording_date {
        after.recording_date = Some(value.clone());
    }
    if let Some(value) = rewrite.track_number {
        after.track_number = Some(value);
    }
    if let Some(value) = rewrite.disc_number {
        after.disc_number = Some(value);
    }
    after
}

fn snapshot_deltas(file: &str, before: &TagSnapshot, after: &TagSnapshot) -> Vec<FieldDelta> {
    let fields = [
        ("title", &before.title, &after.title),
        ("artist", &before.artist, &after.artist),
        ("albumArtist", &before.album_artist, &after.album_artist),
        ("album", &before.album, &after.album),
        (
            "recordingDate",
            &before.recording_date,
            &after.recording_date,
        ),
        (
            "trackNumber",
            &before.track_number.map(|value| value.to_string()),
            &after.track_number.map(|value| value.to_string()),
        ),
        (
            "discNumber",
            &before.disc_number.map(|value| value.to_string()),
            &after.disc_number.map(|value| value.to_string()),
        ),
    ];
    fields
        .into_iter()
        .filter(|(_, old, new)| old != new)
        .map(|(field, old, new)| FieldDelta {
            file: file.to_owned(),
            field: field.to_owned(),
            old: old.clone(),
            new: new.clone(),
        })
        .collect()
}

fn flatten_deltas(plan: &[PlannedFile]) -> Vec<FieldDelta> {
    plan.iter().flat_map(|file| file.deltas.clone()).collect()
}

fn preview_hash_for(operation: &str, params: &Value, deltas: &[FieldDelta]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(operation.as_bytes());
    hasher.update(serde_json::to_vec(params).unwrap_or_default());
    hasher.update(serde_json::to_vec(deltas).unwrap_or_default());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn clean_featuring(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    [" feat.", " feat ", " ft.", " ft ", " featuring ", " with "]
        .iter()
        .filter_map(|marker| lower.find(marker))
        .min()
        .map_or_else(
            || value.trim().to_owned(),
            |index| value[..index].trim().to_owned(),
        )
}

fn valid_year(value: &str) -> Result<String, MetadataEditError> {
    let value = value.trim();
    let year = value
        .parse::<u16>()
        .map_err(|_| MetadataEditError::InvalidRequest)?;
    if !(1000..=9999).contains(&year) {
        return Err(MetadataEditError::InvalidRequest);
    }
    Ok(value.to_owned())
}

fn clean_unwanted_album_title(value: &str) -> String {
    let mut result = value.to_owned();
    for marker in [
        " deluxe",
        " remastered",
        " remaster",
        " anniversary",
        " limited",
        " expanded",
        " compilation",
        " digipak",
        " digipack",
        " vinyl",
        " japan",
        " japanese",
        " asian",
        " web",
        " single",
        " album",
        " bonus",
        " edition",
        " reissue",
        " cd",
        " disc",
        " ep",
        " lp",
    ] {
        result = replace_case_insensitive(&result, marker, "");
    }
    result = result.replace("(320)", "").replace("[320]", "");
    result
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn clean_unwanted_song_title(value: &str) -> String {
    let value = replace_case_insensitive(value, " (prod ", " ");
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_owned()
}

fn replace_artist_separators(value: &str) -> String {
    let mut result = value.to_owned();
    for marker in [
        " with ", " feat. ", " feat ", " ft. ", " ft ", " & ", " x ", ",", ";",
    ] {
        result = replace_case_insensitive(&result, marker, "/");
    }
    result
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn replace_case_insensitive(value: &str, needle: &str, replacement: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut result = String::with_capacity(value.len());
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find(&needle_lower) {
        let start = cursor + relative;
        result.push_str(&value[cursor..start]);
        result.push_str(replacement);
        cursor = start + needle.len();
    }
    result.push_str(&value[cursor..]);
    result
}

fn checked_path(root: &Path, relative: &str) -> Result<PathBuf, MetadataEditError> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(MetadataEditError::InvalidRequest);
    }
    let canonical_root = fs::canonicalize(root)?;
    let path = canonical_root.join(relative_path);
    let canonical = fs::canonicalize(&path).map_err(|_| MetadataEditError::NotFound)?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
        return Err(MetadataEditError::NotFound);
    }
    Ok(canonical)
}

fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let count = std::io::Read::read(&mut file, &mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{process::Command, time::Duration};

    use super::*;

    #[test]
    fn featuring_cleanup_is_case_insensitive_and_preserves_primary_artist() {
        assert_eq!(clean_featuring("Title feat. Guest"), "Title");
        assert_eq!(clean_featuring("Artist WITH Someone"), "Artist");
        assert_eq!(clean_featuring("Plain title"), "Plain title");
    }

    #[test]
    fn preview_hash_changes_when_deltas_change() {
        let params = json!({"from":"old","to":"new"});
        let one = vec![FieldDelta {
            file: "track.mp3".to_owned(),
            field: "title".to_owned(),
            old: Some("old".to_owned()),
            new: Some("new".to_owned()),
        }];
        let mut two = one.clone();
        two[0].new = Some("other".to_owned());
        assert_ne!(
            preview_hash_for("replace_song_titles", &params, &one),
            preview_hash_for("replace_song_titles", &params, &two)
        );
    }

    #[test]
    fn projected_snapshots_are_idempotent() {
        let before = TagSnapshot {
            title: Some("Title feat. Guest".to_owned()),
            ..TagSnapshot::default()
        };
        let rewrite = rewrite_for("remove_featuring_artists", &json!({}), &before, 1)
            .expect("featuring operation");
        let after = projected_snapshot(&before, &rewrite);
        assert_eq!(after.title.as_deref(), Some("Title"));
        assert!(
            snapshot_deltas("track.mp3", &after, &projected_snapshot(&after, &rewrite)).is_empty()
        );
    }

    #[test]
    fn failed_tag_write_restores_the_original_and_removes_temp_artifacts() {
        let directory = tempfile::tempdir().expect("metadata failure fixture directory");
        let path = directory.path().join("track.mp3");
        let original = b"not an audio container".to_vec();
        fs::write(&path, &original).expect("write invalid fixture");
        let item = PlannedFile {
            file: ReleaseFile {
                release_id: Uuid::now_v7(),
                media_asset_id: Uuid::now_v7(),
                relative_path: "track.mp3".to_owned(),
                ordinal: 1,
            },
            path: path.clone(),
            rewrite: TagRewrite {
                title: Some("changed".to_owned()),
                ..TagRewrite::default()
            },
            deltas: Vec::new(),
        };
        let result = atomic_rewrite(
            &item,
            Uuid::now_v7(),
            Path::new("/definitely/missing/ffmpeg"),
            Duration::from_secs(1),
        );
        assert!(matches!(result, Err(MetadataEditError::Tag(_))));
        assert_eq!(fs::read(&path).expect("read original"), original);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("list directory")
                .count(),
            1
        );
    }

    #[test]
    fn verifier_failure_restores_the_original_after_a_rewrite() {
        let directory = tempfile::tempdir().expect("metadata verifier fixture directory");
        let path = directory.path().join("track.mp3");
        let ffmpeg = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.1",
                "-q:a",
                "8",
                "-metadata",
                "title=Original",
                "-y",
                path.to_str().expect("fixture path"),
            ])
            .output();
        let Ok(output) = ffmpeg else { return };
        if !output.status.success() {
            return;
        }
        let original = fs::read(&path).expect("read generated fixture");
        let item = PlannedFile {
            file: ReleaseFile {
                release_id: Uuid::now_v7(),
                media_asset_id: Uuid::now_v7(),
                relative_path: "track.mp3".to_owned(),
                ordinal: 1,
            },
            path: path.clone(),
            rewrite: TagRewrite {
                title: Some("Changed".to_owned()),
                ..TagRewrite::default()
            },
            deltas: Vec::new(),
        };
        let result = atomic_rewrite(
            &item,
            Uuid::now_v7(),
            Path::new("/definitely/missing/ffmpeg"),
            Duration::from_secs(1),
        );
        assert!(
            result.is_err(),
            "expected verifier or tag failure: {result:?}"
        );
        assert_eq!(fs::read(&path).expect("read restored original"), original);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("list directory")
                .count(),
            1
        );
    }
}
