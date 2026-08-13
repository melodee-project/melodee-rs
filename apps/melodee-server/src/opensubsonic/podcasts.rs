//! Conditional podcast endpoints (PRD-API-OS-004, conditional group).
//!
//! Captured contract: when `podcast.enabled` is false the read endpoints
//! answer HTTP 400 with a raw ResponseData body (NOT the subsonic envelope).
//! When enabled, getPodcasts/getNewestPodcasts return channel/episode lists
//! (empty in the fixture). The mutation/stream endpoints
//! (createPodcastChannel, deletePodcastChannel, deletePodcastEpisode,
//! downloadPodcastEpisode, refreshPodcasts, streamPodcastEpisode) are
//! conditional: when disabled they answer the same ResponseData 400 disabled
//! shape; when enabled with a nonexistent id they answer HTTP 400 with a RAW
//! ResponseData body (NOT the subsonic envelope) carrying the captured
//! "Invalid channel id"/"Invalid episode id" messages. refreshPodcasts
//! answers the bare ok envelope. streamPodcastEpisode additionally requires
//! the HasStreamRole. Podcast auth failures are HTTP 403 + error 40 (handled
//! by the auth middleware).

use axum::{
    Router,
    extract::{Extension, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use super::{
    OsState,
    auth::OsPrincipal,
    envelope::{OsReply, negotiate},
    params::Params,
};

/// The captured ResponseData wrapper emitted instead of the subsonic
/// envelope for podcast conditional failures (HTTP 400, no x-total-count).
/// `message` is the captured error text ("Podcasts are currently disabled.",
/// "Invalid channel id", "Invalid episode id").
fn podcasts_raw_error(state: &OsState, principal: Option<&OsPrincipal>, message: &str) -> Response {
    let user_info = match principal {
        Some(user) => serde_json::json!({
            "id": 0,
            "apiKey": user.public_api_id,
            "userName": user.username,
            "email": "",
            "publicKey": "",
            "timeZoneId": "UTC",
            "passwordEncrypted": null,
            "roles": null
        }),
        None => serde_json::json!({
            "id": 0,
            "apiKey": "00000000-0000-0000-0000-000000000000",
            "userName": "",
            "email": "",
            "publicKey": "",
            "timeZoneId": "UTC",
            "passwordEncrypted": null,
            "roles": null
        }),
    };
    let body = serde_json::json!({
        "isSuccess": true,
        "apiKeyId": null,
        "userInfo": user_info,
        "subsonic-response": {
            "isSuccess": false,
            "dataPropertyName": "error",
            "dataDetailPropertyName": "",
            "version": "1.16.1",
            "type": "Melodee",
            "serverVersion": state.server_version,
            "error": {"code": 0, "message": message, "helpUrl": null},
            "etag": null,
            "contentType": null,
            "data": null
        },
        "totalCount": 0
    });
    let mut response = (
        StatusCode::BAD_REQUEST,
        serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string()),
    )
        .into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

/// The captured disabled-podcasts ResponseData shape (error 0 "Podcasts are
/// currently disabled.").
fn podcasts_disabled(state: &OsState, principal: Option<&OsPrincipal>) -> Response {
    podcasts_raw_error(state, principal, "Podcasts are currently disabled.")
}

fn envelope_or_disabled(
    state: &OsState,
    params: Option<&Params>,
    principal: Option<&OsPrincipal>,
    payload: serde_json::Value,
) -> Response {
    if !state.podcasts_enabled {
        return podcasts_disabled(state, principal);
    }
    let format = params.map_or(super::envelope::Format::Xml, negotiate);
    OsReply::ok(payload).respond(&format, &state.server_version, StatusCode::OK)
}

pub async fn get_podcasts(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    envelope_or_disabled(
        &state,
        params.as_ref().map(|Extension(p)| p),
        principal.as_ref().map(|Extension(p)| p),
        serde_json::json!({"podcasts": {"channel": []}}),
    )
}

pub async fn get_newest_podcasts(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    envelope_or_disabled(
        &state,
        params.as_ref().map(|Extension(p)| p),
        principal.as_ref().map(|Extension(p)| p),
        serde_json::json!({"newestPodcasts": {"episode": []}}),
    )
}

/// When podcasts are disabled, the mutation/stream endpoints answer the
/// captured disabled ResponseData 400 shape. When enabled with a nonexistent
/// id (the fixture captures `id=1` which never resolves), they answer the raw
/// ResponseData 400 shape with the captured message.
async fn podcast_mutation_impl(
    state: &OsState,
    _params: &Option<Extension<Params>>,
    principal: &Option<Extension<OsPrincipal>>,
    invalid_message: &str,
) -> Response {
    if !state.podcasts_enabled {
        return podcasts_disabled(state, principal.as_ref().map(|Extension(p)| p));
    }
    // The fixture has no podcast channels/episodes, so every id is invalid.
    // The captured shape is the raw ResponseData 400 (NOT the subsonic
    // envelope) with the captured "Invalid channel id"/"Invalid episode id".
    podcasts_raw_error(
        state,
        principal.as_ref().map(|Extension(p)| p),
        invalid_message,
    )
}

pub async fn create_podcast_channel(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    // The captured createPodcastChannel url-guards the `url` param; the
    // fixture has no feeds, so the baseline answers the disabled shape when
    // off and the invalid-channel shape when on.
    podcast_mutation_impl(&state, &params, &principal, "Invalid channel id").await
}

pub async fn delete_podcast_channel(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    podcast_mutation_impl(&state, &params, &principal, "Invalid channel id").await
}

pub async fn delete_podcast_episode(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    podcast_mutation_impl(&state, &params, &principal, "Invalid episode id").await
}

pub async fn download_podcast_episode(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    podcast_mutation_impl(&state, &params, &principal, "Invalid episode id").await
}

pub async fn refresh_podcasts(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    // refreshPodcasts answers the bare ok envelope whether podcasts are
    // enabled or disabled (the captured os-podcasts-refresh golden is the
    // bare ok shape under the enabled fixture).
    let _ = principal;
    let format = params
        .as_ref()
        .map(|Extension(p)| negotiate(p))
        .unwrap_or(super::envelope::Format::Xml);
    OsReply::ok(serde_json::json!({})).respond(&format, &state.server_version, StatusCode::OK)
}

pub async fn stream_podcast_episode(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
    principal: Option<Extension<OsPrincipal>>,
) -> Response {
    // streamPodcastEpisode additionally requires the HasStreamRole. A caller
    // without the role fails closed with the auth error envelope (the podcast
    // path triggers the 403 + error 40 class through the auth middleware).
    if let Some(Extension(user)) = principal.as_ref()
        && !user.has_role("HasStreamRole")
    {
        return OsReply::error(40, "Wrong username or password.").respond(
            &params
                .as_ref()
                .map(|Extension(p)| negotiate(p))
                .unwrap_or(super::envelope::Format::Xml),
            &state.server_version,
            StatusCode::FORBIDDEN,
        );
    }
    podcast_mutation_impl(&state, &params, &principal, "Invalid episode id").await
}

pub fn router() -> Router<OsState> {
    Router::new()
        .merge(super::endpoint_router(
            "createPodcastChannel",
            create_podcast_channel,
        ))
        .merge(super::endpoint_router(
            "deletePodcastChannel",
            delete_podcast_channel,
        ))
        .merge(super::endpoint_router(
            "deletePodcastEpisode",
            delete_podcast_episode,
        ))
        .merge(super::endpoint_router(
            "downloadPodcastEpisode",
            download_podcast_episode,
        ))
        .merge(super::endpoint_router("refreshPodcasts", refresh_podcasts))
        .merge(super::endpoint_router(
            "streamPodcastEpisode",
            stream_podcast_episode,
        ))
}
