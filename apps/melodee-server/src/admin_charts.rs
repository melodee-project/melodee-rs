//! Admin chart curation API (`/api/admin/v1/charts/*`).
//!
//! The legacy C# admin had a chart editor (`ChartEditor`), an import dialog, an
//! item-resolution dialog, and ranked/missing reports. The Rust server already
//! imported charts (CSV via `melodee-cli charts import`) and served them on the
//! frozen native read routes, but had no operator surface to create, edit,
//! resolve, or report on a chart. This module adds that surface at the admin
//! API level with the shared administrator gate:
//!
//! - `GET    /api/admin/v1/charts` — list charts (including hidden) with item
//!   and missing counts.
//! - `POST   /api/admin/v1/charts` — create a chart (optionally with items).
//! - `GET    /api/admin/v1/charts/{id}` — chart detail with ranked items.
//! - `PATCH  /api/admin/v1/charts/{id}` — update metadata/flags.
//! - `DELETE /api/admin/v1/charts/{id}` — delete a chart and its items.
//! - `PUT    /api/admin/v1/charts/{id}/items` — replace the ranked item list.
//! - `PUT    /api/admin/v1/charts/{id}/items/{rank}/link` — resolve an item to
//!   a catalog Artist/Release (or clear the links).
//! - `GET    /api/admin/v1/charts/{id}/ranked` — ranked report with catalog
//!   linkage.
//! - `GET    /api/admin/v1/charts/{id}/missing` — items with no linked Release
//!   (the legacy "missing report").
//!
//! Charts are curation records: an unlinked item is a *string observation*, not
//! an identity. Marking an item missing never creates or merges catalog
//! entities. Every mutation is bounded, validated against the database
//! constraints, and logged with the acting administrator.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessState, AdminPrincipal};

/// Maximum items accepted in one replace/create request. Legacy charts are
/// curated lists; the bound keeps one request and one transaction small.
pub const MAX_CHART_ITEMS: usize = 500;
pub const MAX_CHART_SLUG_CHARS: usize = 255;
pub const MAX_CHART_TITLE_CHARS: usize = 500;
pub const MAX_CHART_SOURCE_NAME_CHARS: usize = 500;
pub const MAX_CHART_SOURCE_URL_CHARS: usize = 4_096;
pub const MAX_CHART_DESCRIPTION_CHARS: usize = 10_000;
pub const MAX_CHART_TAGS: usize = 50;
pub const MAX_CHART_TAG_CHARS: usize = 100;
/// Listing bound for the admin list route.
pub const MAX_CHART_LIST: i64 = 200;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartErrorBody {
    pub code: &'static str,
    pub message: String,
    pub correlation_id: Uuid,
}

fn admin_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ChartErrorBody {
            code,
            message: message.into(),
            correlation_id: crate::correlation::envelope_id(),
        }),
    )
        .into_response()
}

fn store_failure(context: &'static str, error: impl std::fmt::Display) -> Response {
    tracing::error!(%error, context, "admin chart store failure");
    admin_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "store_error",
        format!("{context}: {error}"),
    )
}

fn validation(message: impl Into<String>) -> Response {
    admin_error(StatusCode::BAD_REQUEST, "validation_error", message)
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| code == "23505")
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route("/api/admin/v1/charts", get(list_charts).post(create_chart))
        .route(
            "/api/admin/v1/charts/{id}",
            get(get_chart).patch(update_chart).delete(delete_chart),
        )
        .route("/api/admin/v1/charts/{id}/items", put(replace_items))
        .route(
            "/api/admin/v1/charts/{id}/items/{rank}/link",
            put(link_item),
        )
        .route("/api/admin/v1/charts/{id}/ranked", get(ranked_report))
        .route("/api/admin/v1/charts/{id}/missing", get(missing_report))
        .route_layer(axum::extract::DefaultBodyLimit::max(256 * 1024))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ChartSummary {
    pub public_api_id: Uuid,
    pub slug: String,
    pub title: String,
    pub source_name: Option<String>,
    pub source_url: Option<String>,
    pub chart_year: Option<i32>,
    pub description: Option<String>,
    pub tags: Vec<String>,
    #[serde(rename = "visible")]
    pub is_visible: bool,
    #[serde(rename = "generatedPlaylistEnabled")]
    pub is_generated_playlist_enabled: bool,
    pub item_count: i64,
    pub missing_count: i64,
    #[serde(serialize_with = "serialize_time")]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "serialize_time")]
    pub updated_at: OffsetDateTime,
}

/// RFC3339 timestamps, matching the other admin DTOs (serde's default for
/// `OffsetDateTime` is an opaque array).
fn serialize_time<S>(value: &OffsetDateTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(
        &value
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".to_string()),
    )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ChartItem {
    pub rank: i32,
    pub artist_name: String,
    pub release_title: String,
    pub release_year: Option<i32>,
    pub linked_artist_id: Option<Uuid>,
    pub linked_release_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartItemInput {
    pub rank: i32,
    #[serde(default)]
    pub artist_name: String,
    #[serde(default)]
    pub release_title: String,
    #[serde(default)]
    pub release_year: Option<i32>,
    /// Optional catalog public ids; when present they are resolved now so the
    /// item is linked, otherwise it stays a string observation.
    #[serde(default)]
    pub artist_id: Option<Uuid>,
    #[serde(default)]
    pub release_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartUpsert {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source_name: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub chart_year: Option<i32>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub generated_playlist_enabled: bool,
    #[serde(default)]
    pub items: Vec<ChartItemInput>,
}

/// Distinguishes "field omitted" (`None`, keep) from "explicit null"
/// (`Some(None)`, clear) for nullable columns.
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartPatch {
    pub slug: Option<String>,
    pub title: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub source_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub source_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub chart_year: Option<Option<i32>>,
    #[serde(default, deserialize_with = "double_option")]
    pub description: Option<Option<String>>,
    pub tags: Option<Vec<String>>,
    pub visible: Option<bool>,
    pub generated_playlist_enabled: Option<bool>,
}

const fn default_true() -> bool {
    true
}

fn bounded(value: &str, max: usize) -> bool {
    value.chars().count() <= max
}

fn validate_chart(
    slug: &str,
    title: &str,
    source_name: Option<&str>,
    source_url: Option<&str>,
    description: Option<&str>,
    tags: &[String],
) -> Result<(), String> {
    if slug.trim().is_empty() || !bounded(slug, MAX_CHART_SLUG_CHARS) {
        return Err(format!(
            "slug must be non-empty and at most {MAX_CHART_SLUG_CHARS} characters"
        ));
    }
    if slug.chars().any(|character| character.is_whitespace()) {
        return Err("slug must not contain whitespace".to_string());
    }
    if title.trim().is_empty() || !bounded(title, MAX_CHART_TITLE_CHARS) {
        return Err(format!(
            "title must be non-empty and at most {MAX_CHART_TITLE_CHARS} characters"
        ));
    }
    if source_name.is_some_and(|value| !bounded(value, MAX_CHART_SOURCE_NAME_CHARS)) {
        return Err(format!(
            "sourceName must be at most {MAX_CHART_SOURCE_NAME_CHARS} characters"
        ));
    }
    if source_url.is_some_and(|value| !bounded(value, MAX_CHART_SOURCE_URL_CHARS)) {
        return Err(format!(
            "sourceUrl must be at most {MAX_CHART_SOURCE_URL_CHARS} characters"
        ));
    }
    if description.is_some_and(|value| !bounded(value, MAX_CHART_DESCRIPTION_CHARS)) {
        return Err(format!(
            "description must be at most {MAX_CHART_DESCRIPTION_CHARS} characters"
        ));
    }
    if tags.len() > MAX_CHART_TAGS {
        return Err(format!("at most {MAX_CHART_TAGS} tags are allowed"));
    }
    if tags
        .iter()
        .any(|tag| tag.trim().is_empty() || !bounded(tag, MAX_CHART_TAG_CHARS))
    {
        return Err(format!(
            "tags must be non-empty and at most {MAX_CHART_TAG_CHARS} characters"
        ));
    }
    Ok(())
}

fn validate_items(items: &[ChartItemInput]) -> Result<(), String> {
    if items.len() > MAX_CHART_ITEMS {
        return Err(format!("at most {MAX_CHART_ITEMS} items are allowed"));
    }
    let mut ranks = std::collections::BTreeSet::new();
    for item in items {
        if item.rank < 1 {
            return Err("item ranks must be positive".to_string());
        }
        if !ranks.insert(item.rank) {
            return Err(format!("duplicate item rank {}", item.rank));
        }
        if item.artist_name.trim().is_empty() || !bounded(&item.artist_name, 500) {
            return Err("item artistName must be non-empty and at most 500 characters".to_string());
        }
        if item.release_title.trim().is_empty() || !bounded(&item.release_title, 500) {
            return Err(
                "item releaseTitle must be non-empty and at most 500 characters".to_string(),
            );
        }
    }
    Ok(())
}

/// Resolves the public ids in one item to internal catalog ids. Returns
/// `Ok(None)` for an unknown public id (caller maps to 404) so a typo never
/// silently produces an unlinked item.
async fn resolve_item_links(
    pool: &sqlx::PgPool,
    item: &ChartItemInput,
) -> Result<Option<(Option<Uuid>, Option<Uuid>)>, sqlx::Error> {
    let artist_id = match item.artist_id {
        Some(public_id) => {
            let resolved: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM artists WHERE public_api_id=$1")
                    .bind(public_id)
                    .fetch_optional(pool)
                    .await?;
            match resolved {
                Some(id) => Some(id),
                None => return Ok(None),
            }
        }
        None => None,
    };
    let release_id = match item.release_id {
        Some(public_id) => {
            let resolved: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM releases WHERE public_api_id=$1")
                    .bind(public_id)
                    .fetch_optional(pool)
                    .await?;
            match resolved {
                Some(id) => Some(id),
                None => return Ok(None),
            }
        }
        None => None,
    };
    Ok(Some((artist_id, release_id)))
}

async fn chart_id_by_public_id(
    pool: &sqlx::PgPool,
    public_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM native_charts WHERE public_api_id=$1")
        .bind(public_id)
        .fetch_optional(pool)
        .await
}

/// GET /api/admin/v1/charts
async fn list_charts(State(state): State<AdminAccessState>, _admin: AdminPrincipal) -> Response {
    match sqlx::query_as::<_, ChartSummary>(
        "SELECT c.public_api_id, c.slug, c.title, c.source_name, c.source_url, c.chart_year,
                c.description, c.tags, c.is_visible, c.is_generated_playlist_enabled,
                (SELECT count(*) FROM native_chart_items i WHERE i.chart_id=c.id) AS item_count,
                (SELECT count(*) FROM native_chart_items i WHERE i.chart_id=c.id
                    AND i.linked_release_id IS NULL) AS missing_count,
                c.created_at, c.updated_at
         FROM native_charts c
         ORDER BY c.created_at DESC, c.public_api_id
         LIMIT $1",
    )
    .bind(MAX_CHART_LIST)
    .fetch_all(&state.pool)
    .await
    {
        Ok(charts) => Json(charts).into_response(),
        Err(error) => store_failure("chart listing failed", error),
    }
}

async fn insert_items(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    chart_id: Uuid,
    items: &[ChartItemInput],
    now: OffsetDateTime,
) -> Result<Option<()>, Box<Response>> {
    for item in items {
        let links = match resolve_item_links_direct(transaction, item).await {
            Ok(Some(links)) => links,
            Ok(None) => return Ok(None),
            Err(error) => {
                return Err(Box::new(store_failure(
                    "chart item link resolution failed",
                    error,
                )));
            }
        };
        if let Err(error) = sqlx::query(
            "INSERT INTO native_chart_items (
                 chart_id, rank, artist_name, release_title, release_year,
                 linked_artist_id, linked_release_id, created_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(chart_id)
        .bind(item.rank)
        .bind(&item.artist_name)
        .bind(&item.release_title)
        .bind(item.release_year)
        .bind(links.0)
        .bind(links.1)
        .bind(now)
        .execute(&mut **transaction)
        .await
        {
            return Err(Box::new(store_failure("chart item insert failed", error)));
        }
    }
    Ok(Some(()))
}

/// Transaction-scoped variant of [`resolve_item_links`].
async fn resolve_item_links_direct(
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    item: &ChartItemInput,
) -> Result<Option<(Option<Uuid>, Option<Uuid>)>, sqlx::Error> {
    let artist_id = match item.artist_id {
        Some(public_id) => {
            let resolved: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM artists WHERE public_api_id=$1")
                    .bind(public_id)
                    .fetch_optional(&mut **transaction)
                    .await?;
            match resolved {
                Some(id) => Some(id),
                None => return Ok(None),
            }
        }
        None => None,
    };
    let release_id = match item.release_id {
        Some(public_id) => {
            let resolved: Option<Uuid> =
                sqlx::query_scalar("SELECT id FROM releases WHERE public_api_id=$1")
                    .bind(public_id)
                    .fetch_optional(&mut **transaction)
                    .await?;
            match resolved {
                Some(id) => Some(id),
                None => return Ok(None),
            }
        }
        None => None,
    };
    Ok(Some((artist_id, release_id)))
}

/// POST /api/admin/v1/charts
async fn create_chart(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<ChartUpsert>,
) -> Response {
    if let Err(message) = validate_chart(
        &request.slug,
        &request.title,
        request.source_name.as_deref(),
        request.source_url.as_deref(),
        request.description.as_deref(),
        &request.tags,
    ) {
        return validation(message);
    }
    if let Err(message) = validate_items(&request.items) {
        return validation(message);
    }
    let now = OffsetDateTime::now_utc();
    let id = Uuid::now_v7();
    let public_id = Uuid::now_v7();
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return store_failure("chart transaction start failed", error),
    };
    if let Err(error) = sqlx::query(
        "INSERT INTO native_charts (
             id, public_api_id, slug, title, source_name, source_url, chart_year,
             description, tags, is_visible, is_generated_playlist_enabled, created_at, updated_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(id)
    .bind(public_id)
    .bind(request.slug.trim())
    .bind(request.title.trim())
    .bind(request.source_name.as_deref())
    .bind(request.source_url.as_deref())
    .bind(request.chart_year)
    .bind(request.description.as_deref())
    .bind(&request.tags)
    .bind(request.visible)
    .bind(request.generated_playlist_enabled)
    .bind(now)
    .bind(now)
    .execute(&mut *transaction)
    .await
    {
        if is_unique_violation(&error) {
            return admin_error(
                StatusCode::CONFLICT,
                "chart_exists",
                "a chart with that slug already exists",
            );
        }
        return store_failure("chart insert failed", error);
    }
    match insert_items(&mut transaction, id, &request.items, now).await {
        Ok(Some(())) => {}
        Ok(None) => {
            let _ = transaction.rollback().await;
            return admin_error(
                StatusCode::NOT_FOUND,
                "catalog_entity_not_found",
                "one of the item artistId/releaseId values does not exist",
            );
        }
        Err(response) => {
            let _ = transaction.rollback().await;
            return *response;
        }
    }
    if let Err(error) = transaction.commit().await {
        return store_failure("chart commit failed", error);
    }
    tracing::info!(
        actor = %admin.public_api_id,
        chart = %public_id,
        items = request.items.len(),
        "admin created chart"
    );
    match load_chart(&state.pool, public_id).await {
        Ok(Some(chart)) => (StatusCode::CREATED, Json(chart)).into_response(),
        Ok(None) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
        Err(error) => store_failure("chart reload failed", error),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ChartDetail {
    #[serde(flatten)]
    summary: ChartSummary,
    items: Vec<ChartItem>,
}

async fn load_chart(
    pool: &sqlx::PgPool,
    public_id: Uuid,
) -> Result<Option<ChartDetail>, sqlx::Error> {
    let summary = sqlx::query_as::<_, ChartSummary>(
        "SELECT c.public_api_id, c.slug, c.title, c.source_name, c.source_url, c.chart_year,
                c.description, c.tags, c.is_visible, c.is_generated_playlist_enabled,
                (SELECT count(*) FROM native_chart_items i WHERE i.chart_id=c.id) AS item_count,
                (SELECT count(*) FROM native_chart_items i WHERE i.chart_id=c.id
                    AND i.linked_release_id IS NULL) AS missing_count,
                c.created_at, c.updated_at
         FROM native_charts c WHERE c.public_api_id=$1",
    )
    .bind(public_id)
    .fetch_optional(pool)
    .await?;
    let Some(summary) = summary else {
        return Ok(None);
    };
    let items = sqlx::query_as::<_, ChartItem>(
        "SELECT i.rank, i.artist_name, i.release_title, i.release_year,
                a.public_api_id AS linked_artist_id, r.public_api_id AS linked_release_id
         FROM native_chart_items i
         JOIN native_charts c ON c.id=i.chart_id
         LEFT JOIN artists a ON a.id=i.linked_artist_id
         LEFT JOIN releases r ON r.id=i.linked_release_id
         WHERE c.public_api_id=$1
         ORDER BY i.rank",
    )
    .bind(public_id)
    .fetch_all(pool)
    .await?;
    Ok(Some(ChartDetail { summary, items }))
}

/// GET /api/admin/v1/charts/{id}
async fn get_chart(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match load_chart(&state.pool, id).await {
        Ok(Some(chart)) => Json(chart).into_response(),
        Ok(None) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
        Err(error) => store_failure("chart load failed", error),
    }
}

/// PATCH /api/admin/v1/charts/{id}
async fn update_chart(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(request): Json<ChartPatch>,
) -> Response {
    let Some(existing) = (match load_chart(&state.pool, id).await {
        Ok(existing) => existing,
        Err(error) => return store_failure("chart load failed", error),
    }) else {
        return admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found");
    };
    let summary = existing.summary;
    let slug = request.slug.unwrap_or(summary.slug);
    let title = request.title.unwrap_or(summary.title);
    let source_name = request.source_name.unwrap_or(summary.source_name);
    let source_url = request.source_url.unwrap_or(summary.source_url);
    let description = request.description.unwrap_or(summary.description);
    let tags = request.tags.unwrap_or(summary.tags);
    let visible = request.visible.unwrap_or(summary.is_visible);
    let generated = request
        .generated_playlist_enabled
        .unwrap_or(summary.is_generated_playlist_enabled);
    let chart_year = request.chart_year.unwrap_or(summary.chart_year);
    if let Err(message) = validate_chart(
        &slug,
        &title,
        source_name.as_deref(),
        source_url.as_deref(),
        description.as_deref(),
        &tags,
    ) {
        return validation(message);
    }
    let now = OffsetDateTime::now_utc();
    match sqlx::query(
        "UPDATE native_charts SET slug=$2, title=$3, source_name=$4, source_url=$5,
             chart_year=$6, description=$7, tags=$8, is_visible=$9,
             is_generated_playlist_enabled=$10, updated_at=$11
         WHERE public_api_id=$1",
    )
    .bind(id)
    .bind(slug.trim())
    .bind(title.trim())
    .bind(source_name.as_deref())
    .bind(source_url.as_deref())
    .bind(chart_year)
    .bind(description.as_deref())
    .bind(&tags)
    .bind(visible)
    .bind(generated)
    .bind(now)
    .execute(&state.pool)
    .await
    {
        Ok(_) => {
            tracing::info!(actor = %admin.public_api_id, chart = %id, "admin updated chart");
            match load_chart(&state.pool, id).await {
                Ok(Some(chart)) => Json(chart).into_response(),
                Ok(None) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
                Err(error) => store_failure("chart reload failed", error),
            }
        }
        Err(error) if is_unique_violation(&error) => admin_error(
            StatusCode::CONFLICT,
            "chart_exists",
            "a chart with that slug already exists",
        ),
        Err(error) => store_failure("chart update failed", error),
    }
}

/// DELETE /api/admin/v1/charts/{id}
async fn delete_chart(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match sqlx::query("DELETE FROM native_charts WHERE public_api_id=$1")
        .bind(id)
        .execute(&state.pool)
        .await
    {
        Ok(result) if result.rows_affected() == 1 => {
            tracing::warn!(
                actor = %admin.public_api_id,
                chart = %id,
                "admin deleted chart and its items"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(_) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
        Err(error) => store_failure("chart delete failed", error),
    }
}

/// PUT /api/admin/v1/charts/{id}/items
async fn replace_items(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<Uuid>,
    Json(items): Json<Vec<ChartItemInput>>,
) -> Response {
    if let Err(message) = validate_items(&items) {
        return validation(message);
    }
    let Some(chart_id) = (match chart_id_by_public_id(&state.pool, id).await {
        Ok(chart_id) => chart_id,
        Err(error) => return store_failure("chart lookup failed", error),
    }) else {
        return admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found");
    };
    let now = OffsetDateTime::now_utc();
    let mut transaction = match state.pool.begin().await {
        Ok(transaction) => transaction,
        Err(error) => return store_failure("chart transaction start failed", error),
    };
    if let Err(error) = sqlx::query("DELETE FROM native_chart_items WHERE chart_id=$1")
        .bind(chart_id)
        .execute(&mut *transaction)
        .await
    {
        return store_failure("chart item clear failed", error);
    }
    match insert_items(&mut transaction, chart_id, &items, now).await {
        Ok(Some(())) => {}
        Ok(None) => {
            let _ = transaction.rollback().await;
            return admin_error(
                StatusCode::NOT_FOUND,
                "catalog_entity_not_found",
                "one of the item artistId/releaseId values does not exist",
            );
        }
        Err(response) => {
            let _ = transaction.rollback().await;
            return *response;
        }
    }
    if let Err(error) = sqlx::query("UPDATE native_charts SET updated_at=$2 WHERE id=$1")
        .bind(chart_id)
        .bind(now)
        .execute(&mut *transaction)
        .await
    {
        return store_failure("chart touch failed", error);
    }
    if let Err(error) = transaction.commit().await {
        return store_failure("chart commit failed", error);
    }
    tracing::info!(
        actor = %admin.public_api_id,
        chart = %id,
        items = items.len(),
        "admin replaced chart items"
    );
    match load_chart(&state.pool, id).await {
        Ok(Some(chart)) => Json(chart).into_response(),
        Ok(None) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
        Err(error) => store_failure("chart reload failed", error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartLinkRequest {
    /// Omit or `null` to clear the link.
    pub artist_id: Option<Uuid>,
    /// Omit or `null` to clear the link.
    pub release_id: Option<Uuid>,
}

/// PUT /api/admin/v1/charts/{id}/items/{rank}/link
async fn link_item(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path((id, rank)): Path<(Uuid, i32)>,
    Json(request): Json<ChartLinkRequest>,
) -> Response {
    let Some(chart_id) = (match chart_id_by_public_id(&state.pool, id).await {
        Ok(chart_id) => chart_id,
        Err(error) => return store_failure("chart lookup failed", error),
    }) else {
        return admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found");
    };
    let resolved = ChartItemInput {
        rank,
        artist_name: String::new(),
        release_title: String::new(),
        release_year: None,
        artist_id: request.artist_id,
        release_id: request.release_id,
    };
    let links = match resolve_item_links(&state.pool, &resolved).await {
        Ok(Some(links)) => links,
        Ok(None) => {
            return admin_error(
                StatusCode::NOT_FOUND,
                "catalog_entity_not_found",
                "artistId or releaseId does not exist",
            );
        }
        Err(error) => return store_failure("chart link resolution failed", error),
    };
    let now = OffsetDateTime::now_utc();
    match sqlx::query(
        "UPDATE native_chart_items SET linked_artist_id=$3, linked_release_id=$4
         WHERE chart_id=$1 AND rank=$2",
    )
    .bind(chart_id)
    .bind(rank)
    .bind(links.0)
    .bind(links.1)
    .execute(&state.pool)
    .await
    {
        Ok(result) if result.rows_affected() == 1 => {
            if let Err(error) = sqlx::query("UPDATE native_charts SET updated_at=$2 WHERE id=$1")
                .bind(chart_id)
                .bind(now)
                .execute(&state.pool)
                .await
            {
                return store_failure("chart touch failed", error);
            }
            tracing::info!(
                actor = %admin.public_api_id,
                chart = %id,
                rank,
                linked = links.0.is_some() || links.1.is_some(),
                "admin resolved chart item"
            );
            match load_chart(&state.pool, id).await {
                Ok(Some(chart)) => Json(chart).into_response(),
                Ok(None) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
                Err(error) => store_failure("chart reload failed", error),
            }
        }
        Ok(_) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart item not found"),
        Err(error) => store_failure("chart item link update failed", error),
    }
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RankedChartItem {
    pub rank: i32,
    pub artist_name: String,
    pub release_title: String,
    pub release_year: Option<i32>,
    /// Linked Release public id (absent for missing items).
    pub linked_release_id: Option<Uuid>,
    /// Catalog Release title when linked (differs from the curated string when
    /// the catalog was corrected after curation).
    pub catalog_release_title: Option<String>,
    pub missing: bool,
}

/// GET /api/admin/v1/charts/{id}/ranked
async fn ranked_report(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match sqlx::query_as::<_, RankedChartItem>(
        "SELECT i.rank, i.artist_name, i.release_title, i.release_year,
                r.public_api_id AS linked_release_id,
                r.title AS catalog_release_title,
                (i.linked_release_id IS NULL) AS missing
         FROM native_chart_items i
         JOIN native_charts c ON c.id=i.chart_id
         LEFT JOIN releases r ON r.id=i.linked_release_id
         WHERE c.public_api_id=$1
         ORDER BY i.rank",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(items) if items.is_empty() => match chart_id_by_public_id(&state.pool, id).await {
            Ok(None) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
            Ok(Some(_)) => Json(items).into_response(),
            Err(error) => store_failure("chart lookup failed", error),
        },
        Ok(items) => Json(items).into_response(),
        Err(error) => store_failure("chart ranked report failed", error),
    }
}

/// GET /api/admin/v1/charts/{id}/missing
async fn missing_report(
    State(state): State<AdminAccessState>,
    _admin: AdminPrincipal,
    Path(id): Path<Uuid>,
) -> Response {
    match sqlx::query_as::<_, RankedChartItem>(
        "SELECT i.rank, i.artist_name, i.release_title, i.release_year,
                NULL::uuid AS linked_release_id,
                NULL::text AS catalog_release_title,
                true AS missing
         FROM native_chart_items i
         JOIN native_charts c ON c.id=i.chart_id
         WHERE c.public_api_id=$1 AND i.linked_release_id IS NULL
         ORDER BY i.rank",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    {
        Ok(items) if items.is_empty() => match chart_id_by_public_id(&state.pool, id).await {
            Ok(None) => admin_error(StatusCode::NOT_FOUND, "not_found", "chart not found"),
            Ok(Some(_)) => Json(items).into_response(),
            Err(error) => store_failure("chart lookup failed", error),
        },
        Ok(items) => Json(items).into_response(),
        Err(error) => store_failure("chart missing report failed", error),
    }
}
