//! Internet radio endpoint group: getInternetRadioStations,
//! createInternetRadioStation, updateInternetRadioStation,
//! deleteInternetRadioStation (PRD-API-OS-005; golden evidence
//! `compatibility/opensubsonic/golden/community/os-radio-*`).
//!
//! Captured contract: `getInternetRadioStations` lists `radio_{guid}`
//! entries (name, streamUrl, homepageUrl). CRUD mutations require the admin
//! role (the legacy fixture captures admin-gated mutations; a non-admin caller
//! fails closed with the auth error envelope). Unknown radio ids on
//! update/delete answer the captured error 44 "Invalid API key." shape (the
//! legacy fixture captured `{radioId}` template ids that never resolved).
//! Storage: `opensubsonic_radio_stations` (migration 0020).

use axum::{
    Router,
    extract::{Extension, State},
    http::StatusCode,
    response::Response,
};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{
    OsState,
    auth::OsPrincipal,
    envelope::{Format, OsReply, negotiate},
    params::Params,
};

fn format_of(params: &Option<Extension<Params>>) -> Format {
    params
        .as_ref()
        .map_or(Format::Xml, |Extension(params)| negotiate(params))
}

fn reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    payload: serde_json::Value,
) -> Response {
    OsReply::ok(payload).respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn error_reply(
    state: &OsState,
    params: &Option<Extension<Params>>,
    code: i64,
    message: &str,
) -> Response {
    OsReply::error(code, message).respond(&format_of(params), &state.server_version, StatusCode::OK)
}

fn wire_id(public_api_id: Uuid) -> String {
    format!("radio_{public_api_id}")
}

fn public_id_from_param(params: &Params, key: &str) -> Option<Uuid> {
    params.get(key).and_then(|value| {
        value
            .strip_prefix("radio_")
            .and_then(|raw| Uuid::parse_str(raw).ok())
    })
}

fn station_json(
    public_api_id: Uuid,
    name: &str,
    stream_url: &str,
    home_page_url: Option<&str>,
) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "id".to_string(),
        serde_json::Value::from(wire_id(public_api_id)),
    );
    map.insert("name".to_string(), serde_json::Value::from(name));
    map.insert("streamUrl".to_string(), serde_json::Value::from(stream_url));
    if let Some(home) = home_page_url {
        map.insert("homePageUrl".to_string(), serde_json::Value::from(home));
    }
    serde_json::Value::Object(map)
}

pub async fn get_internet_radio_stations(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _ = match &principal {
        Some(Extension(p)) => p,
        None => return error_reply(&state, &params, 40, "Wrong username or password."),
    };
    let rows = match sqlx::query(
        "SELECT public_api_id, name, stream_url, home_page_url
         FROM opensubsonic_radio_stations
         ORDER BY name, public_api_id",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "opensubsonic getInternetRadioStations failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    let mut stations = Vec::with_capacity(rows.len());
    for row in rows {
        let public_api_id: Uuid = row.try_get(0).unwrap_or_else(|_| Uuid::nil());
        let name: String = row.try_get(1).unwrap_or_default();
        let stream_url: String = row.try_get(2).unwrap_or_default();
        let home_page_url: Option<String> = row.try_get(3).unwrap_or(None);
        stations.push(station_json(
            public_api_id,
            &name,
            &stream_url,
            home_page_url.as_deref(),
        ));
    }
    reply(
        &state,
        &params,
        serde_json::json!({ "internetRadioStations": { "internetRadioStation": stations } }),
    )
}

/// Admin gate for radio CRUD mutations (the captured fixture performs them as
/// the admin fixture user). A non-admin caller fails closed with the auth
/// error envelope.
fn require_admin(
    _state: &OsState,
    _params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
) -> Option<OsPrincipal> {
    principal
        .as_ref()
        .and_then(|Extension(p)| (p.is_admin || p.has_role("Administrator")).then(|| p.clone()))
        .or({
            // Non-admin or bypass: no captured mutation behavior, fail closed.
            None
        })
}

#[allow(clippy::result_large_err)]
fn admin_or_auth_error(
    state: &OsState,
    params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
) -> Result<OsPrincipal, Response> {
    match require_admin(state, params, principal) {
        Some(p) => Ok(p),
        None => Err(error_reply(
            state,
            params,
            40,
            "Wrong username or password.",
        )),
    }
}

pub async fn create_internet_radio_station(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _user = match admin_or_auth_error(&state, &params, &principal) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(name) = params_ref.get("name") else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(stream_url) = params_ref.get("streamUrl") else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let home_page_url = params_ref
        .get("homepageUrl")
        .or_else(|| params_ref.get("homePageUrl"));
    let now = OffsetDateTime::now_utc();
    let id = Uuid::now_v7();
    let public_api_id = Uuid::now_v7();
    if let Err(error) = sqlx::query(
        "INSERT INTO opensubsonic_radio_stations (id, public_api_id, name, stream_url, home_page_url, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $6)",
    )
    .bind(id)
    .bind(public_api_id)
    .bind(name)
    .bind(stream_url)
    .bind(home_page_url)
    .bind(now)
    .execute(&state.pool)
    .await
    {
        tracing::error!(%error, "opensubsonic createInternetRadioStation failed");
        return error_reply(&state, &params, 0, "A generic error.");
    }
    reply(&state, &params, serde_json::json!({}))
}

pub async fn update_internet_radio_station(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _user = match admin_or_auth_error(&state, &params, &principal) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    // The captured update golden used a `{radioId}` template id that never
    // resolved, producing the error 44 "Invalid API key." shape. A real
    // `radio_<uuid>` that resolves applies the update; an unresolvable id
    // answers error 44.
    let Some(id) = public_id_from_param(params_ref, "id") else {
        return error_reply(&state, &params, 44, "Invalid API key.");
    };
    let now = OffsetDateTime::now_utc();
    let mut updates: Vec<String> = Vec::new();
    let mut name_bind: Option<String> = None;
    let mut stream_bind: Option<String> = None;
    let mut home_bind: Option<String> = None;
    if let Some(name) = params_ref.get("name") {
        name_bind = Some(name.to_string());
        updates.push("name = $2".to_string());
    }
    if let Some(stream_url) = params_ref.get("streamUrl") {
        stream_bind = Some(stream_url.to_string());
        let idx = updates.len() + 2;
        updates.push(format!("stream_url = ${idx}"));
    }
    if let Some(home) = params_ref
        .get("homepageUrl")
        .or_else(|| params_ref.get("homePageUrl"))
    {
        home_bind = Some(home.to_string());
        let idx = updates.len() + 2;
        updates.push(format!("home_page_url = ${idx}"));
    }
    if updates.is_empty() {
        return reply(&state, &params, serde_json::json!({}));
    }
    let updates_sql = updates.join(", ");
    let finished = format!("updated_at = ${}", updates.len() + 2);
    let sql = format!(
        "UPDATE opensubsonic_radio_stations SET {updates_sql}, {finished} WHERE public_api_id = $1"
    );
    let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id);
    if let Some(name) = name_bind {
        query = query.bind(name);
    }
    if let Some(stream) = stream_bind {
        query = query.bind(stream);
    }
    if let Some(home) = home_bind {
        query = query.bind(home);
    }
    query = query.bind(now);
    let result = match query.execute(&state.pool).await {
        Ok(result) => result,
        Err(error) => {
            tracing::error!(%error, "opensubsonic updateInternetRadioStation failed");
            return error_reply(&state, &params, 0, "A generic error.");
        }
    };
    if result.rows_affected() == 0 {
        return error_reply(&state, &params, 44, "Invalid API key.");
    }
    reply(&state, &params, serde_json::json!({}))
}

pub async fn delete_internet_radio_station(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    let _user = match admin_or_auth_error(&state, &params, &principal) {
        Ok(user) => user,
        Err(response) => return response,
    };
    let Some(Extension(params_ref)) = params.as_ref() else {
        return error_reply(&state, &params, 10, "Required parameter is missing.");
    };
    let Some(id) = public_id_from_param(params_ref, "id") else {
        return error_reply(&state, &params, 44, "Invalid API key.");
    };
    let result =
        match sqlx::query("DELETE FROM opensubsonic_radio_stations WHERE public_api_id = $1")
            .bind(id)
            .execute(&state.pool)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                tracing::error!(%error, "opensubsonic deleteInternetRadioStation failed");
                return error_reply(&state, &params, 0, "A generic error.");
            }
        };
    if result.rows_affected() == 0 {
        return error_reply(&state, &params, 44, "Invalid API key.");
    }
    reply(&state, &params, serde_json::json!({}))
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router(
            "getInternetRadioStations",
            get_internet_radio_stations,
        ))
        .merge(super::endpoint_router(
            "createInternetRadioStation",
            create_internet_radio_station,
        ))
        .merge(super::endpoint_router(
            "updateInternetRadioStation",
            update_internet_radio_station,
        ))
        .merge(super::endpoint_router(
            "deleteInternetRadioStation",
            delete_internet_radio_station,
        ))
}
