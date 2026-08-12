//! Native-v1 playback endpoint registry routes.
//!
//! These four routes are compiled independently from the excluded legacy
//! Party/Queue controller families. PostgreSQL retains the minimal migrated
//! session membership needed to enforce endpoint ownership and DJ authority.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::{Authenticated, NativeState, error::NativeApiError};

pub fn router() -> Router<NativeState> {
    Router::new()
        .route("/api/v1/endpoints", get(endpoints))
        .route(
            "/api/v1/endpoints/for-session/{session_id}",
            get(endpoints_for_session),
        )
        .route(
            "/api/v1/endpoints/{endpoint_id}/attach",
            post(attach_endpoint),
        )
        .route(
            "/api/v1/endpoints/{endpoint_id}/detach",
            post(detach_endpoint),
        )
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct EndpointDto {
    api_key: Uuid,
    name: String,
    r#type: String,
    is_shared: bool,
    room: Option<String>,
    last_seen_at: Option<String>,
    capabilities_json: Option<String>,
    is_owner: bool,
}

#[derive(Debug, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
struct SessionEndpointDto {
    api_key: Uuid,
    name: String,
    r#type: String,
    is_shared: bool,
    room: Option<String>,
    last_seen_at: Option<String>,
    capabilities_json: Option<String>,
    is_owner: bool,
    is_active: bool,
    is_stale: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttachEndpointRequest {
    session_api_key: Uuid,
}

async fn endpoints(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Result<Json<Vec<EndpointDto>>, NativeApiError> {
    let rows = sqlx::query_as::<_, EndpointDto>(
        "SELECT public_api_id AS api_key,name,endpoint_type AS type,is_shared,room,\
         last_seen_at::text,capabilities_json,true AS is_owner \
         FROM native_playback_endpoints WHERE owner_user_id=$1 \
         ORDER BY name,public_api_id LIMIT 1000",
    )
    .bind(principal.user_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

async fn session_role(
    tx: &mut Transaction<'_, Postgres>,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<Option<(String, bool)>, NativeApiError> {
    Ok(sqlx::query_as::<_, (String, bool)>(
        "SELECT member.role,session.is_active \
         FROM native_endpoint_sessions session \
         JOIN native_endpoint_session_members member \
           ON member.session_public_api_id=session.session_public_api_id \
         WHERE session.session_public_api_id=$1 AND member.user_id=$2 \
         FOR UPDATE OF session",
    )
    .bind(session_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?)
}

async fn endpoints_for_session(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(session_id): Path<Uuid>,
) -> Result<Json<Vec<SessionEndpointDto>>, NativeApiError> {
    let role = sqlx::query_scalar::<_, String>(
        "SELECT member.role FROM native_endpoint_sessions session \
         JOIN native_endpoint_session_members member \
           ON member.session_public_api_id=session.session_public_api_id \
         WHERE session.session_public_api_id=$1 AND member.user_id=$2",
    )
    .bind(session_id)
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    let may_use_system = matches!(role.as_str(), "owner" | "dj");
    let rows = sqlx::query_as::<_, SessionEndpointDto>(
        "SELECT endpoint.public_api_id AS api_key,endpoint.name,endpoint.endpoint_type AS type,\
         endpoint.is_shared,endpoint.room,endpoint.last_seen_at::text,endpoint.capabilities_json,\
         COALESCE(endpoint.owner_user_id=$2,false) AS is_owner,\
         COALESCE(endpoint.id=session.active_endpoint_id,false) AS is_active,\
         endpoint.last_seen_at IS NULL OR endpoint.last_seen_at < now()-interval '30 seconds' AS is_stale \
         FROM native_endpoint_sessions session CROSS JOIN native_playback_endpoints endpoint \
         WHERE session.session_public_api_id=$1 AND \
           (endpoint.owner_user_id=$2 OR ($3 AND endpoint.owner_user_id IS NULL)) \
         ORDER BY endpoint.name,endpoint.public_api_id LIMIT 1000",
    )
    .bind(session_id)
    .bind(principal.user_id)
    .bind(may_use_system)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

async fn attach_endpoint(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(endpoint_id): Path<Uuid>,
    Json(request): Json<AttachEndpointRequest>,
) -> Result<StatusCode, NativeApiError> {
    let mut tx = state.pool.begin().await?;
    let (role, active) = session_role(&mut tx, request.session_api_key, principal.user_id)
        .await?
        .ok_or(NativeApiError::NotFound)?;
    if !active {
        return Err(NativeApiError::CapturedBadRequest(
            "Cannot attach endpoint to inactive session",
        ));
    }
    if !matches!(role.as_str(), "owner" | "dj") {
        return Err(NativeApiError::Forbidden);
    }

    let endpoint = sqlx::query_as::<_, (Uuid, Option<Uuid>, bool)>(
        "SELECT id,owner_user_id,is_shared FROM native_playback_endpoints \
         WHERE public_api_id=$1 FOR UPDATE",
    )
    .bind(endpoint_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    if endpoint.1 != Some(principal.user_id) && !(endpoint.1.is_none() && endpoint.2) {
        return Err(NativeApiError::Forbidden);
    }

    let existing = sqlx::query_as::<_, (Uuid, Option<time::OffsetDateTime>)>(
        "SELECT current.public_api_id,current.last_seen_at \
         FROM native_endpoint_sessions session \
         JOIN native_playback_endpoints current ON current.id=session.active_endpoint_id \
         WHERE session.session_public_api_id=$1",
    )
    .bind(request.session_api_key)
    .fetch_optional(&mut *tx)
    .await?;
    if existing.is_some_and(|(id, seen)| {
        id != endpoint_id
            && seen.is_some_and(|seen| {
                seen >= time::OffsetDateTime::now_utc() - time::Duration::seconds(30)
            })
    }) {
        return Err(NativeApiError::CapturedBadRequest(
            "Another endpoint is already attached. Detach it first.",
        ));
    }

    sqlx::query(
        "UPDATE native_endpoint_sessions SET active_endpoint_id=$2,updated_at=now() \
         WHERE session_public_api_id=$1",
    )
    .bind(request.session_api_key)
    .bind(endpoint.0)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE native_playback_endpoints SET last_seen_at=now(),updated_at=now() WHERE id=$1",
    )
    .bind(endpoint.0)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn detach_endpoint(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(endpoint_id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    let mut tx = state.pool.begin().await?;
    let (internal_id, owner_id) = sqlx::query_as::<_, (Uuid, Option<Uuid>)>(
        "SELECT id,owner_user_id FROM native_playback_endpoints \
         WHERE public_api_id=$1 FOR UPDATE",
    )
    .bind(endpoint_id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(NativeApiError::NotFound)?;
    if let Some(owner_id) = owner_id {
        if owner_id != principal.user_id {
            return Err(NativeApiError::Forbidden);
        }
    } else {
        let authorized = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM native_endpoint_sessions session \
             JOIN native_endpoint_session_members member \
               ON member.session_public_api_id=session.session_public_api_id \
             WHERE session.active_endpoint_id=$1 AND member.user_id=$2 \
               AND member.role IN ('owner','dj'))",
        )
        .bind(internal_id)
        .bind(principal.user_id)
        .fetch_one(&mut *tx)
        .await?;
        if !authorized {
            return Err(NativeApiError::Forbidden);
        }
    }
    sqlx::query(
        "UPDATE native_endpoint_sessions SET active_endpoint_id=NULL,updated_at=now() \
         WHERE active_endpoint_id=$1",
    )
    .bind(internal_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
