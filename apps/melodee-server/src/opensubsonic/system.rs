//! System endpoint group: ping, getLicense, getOpenSubsonicExtensions,
//! getMusicFolders (PRD-API-OS-004).

use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::Response,
};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    OsState,
    envelope::{OsReply, negotiate},
    params::Params,
};

fn format_from(extensions: &Option<Params>) -> super::envelope::Format {
    extensions
        .as_ref()
        .map_or(super::envelope::Format::Xml, negotiate)
}

fn params_of(extension: Option<Extension<Params>>) -> Option<Params> {
    extension.map(|Extension(params)| params)
}

pub async fn ping(State(state): State<OsState>, params: Option<Extension<Params>>) -> Response {
    let format = format_from(&params_of(params));
    OsReply::ok(serde_json::json!({})).respond(&format, &state.server_version, StatusCode::OK)
}

/// Captured quirks preserved: the JSON key is `trailExpires` (sic) while the
/// XML attribute is `trialExpires`, and the XML nests a second `<license>`
/// element with irregular spacing. Expiry values are now+50y.
pub async fn get_license(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
) -> Response {
    let format = format_from(&params_of(params));
    let expires = (OffsetDateTime::now_utc() + Duration::days(365 * 50))
        .format(&Rfc3339)
        .unwrap_or_else(|_| "9999-12-31T23:59:59Z".to_string());
    let email = "license@melodee.invalid";
    let reply = OsReply::ok(serde_json::json!({
        "license": {
            "valid": true,
            "email": email,
            "licenseExpires": expires,
            "trailExpires": expires,
        }
    }))
    .with_xml_children_override(format!(
        "<license><license valid=\"true\" email=\"{email}\" licenseExpires=\"{expires}\"  trialExpires=\"{expires}\" /></license>"
    ));
    reply.respond(&format, &state.server_version, StatusCode::OK)
}

/// The extension list is frozen by ADR-0130 decision 2, including the
/// captured malformed XML serialization (attributes with no element name).
pub async fn get_opensubsonic_extensions(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
) -> Response {
    let format = format_from(&params_of(params));
    let reply = OsReply::ok(serde_json::json!({
        "openSubsonicExtensions": [
            {"name": "melodeeExtensions", "versions": [1]},
            {"name": "apiKeyAuthentication", "versions": [1]},
            {"name": "formPost", "versions": [1]},
            {"name": "songLyrics", "versions": [1]},
            {"name": "transcodeOffset", "versions": [1]}
        ]
    }))
    .with_xml_children_override(
        "<openSubsonicExtensions>name=\"melodeeExtensions\" versions=\"1 \"/>name=\"apiKeyAuthentication\" versions=\"1 \"/>name=\"formPost\" versions=\"1 \"/>name=\"songLyrics\" versions=\"1 \"/>name=\"transcodeOffset\" versions=\"1 \"/></openSubsonicExtensions>".to_string(),
    );
    reply.respond(&format, &state.server_version, StatusCode::OK)
}

pub async fn get_music_folders(
    State(state): State<OsState>,
    params: Option<Extension<Params>>,
) -> Response {
    let format = format_from(&params_of(params));
    let folders: Vec<serde_json::Value> = state
        .music_folders
        .iter()
        .map(|folder| {
            serde_json::json!({
                "id": folder.wire_id(),
                "name": folder.name,
            })
        })
        .collect();
    let reply = OsReply::ok(serde_json::json!({
        "musicFolders": {"musicFolder": folders}
    }));
    reply.respond(&format, &state.server_version, StatusCode::OK)
}
