//! Native avatar upload/change/delete routes (WP-03, PRD-USER-001,
//! ADR-0134 §2). These are new native-surface routes for the authenticated
//! user's own avatar; no captured contract covers them.
//!
//! Uploads are raw request bodies (`Content-Type: image/jpeg|image/png`),
//! magic-byte validated, decoded under the shared image-pipeline limits, and
//! re-encoded to at most 1 MiB before reaching `native_user_avatars`. The
//! stored `updated_at` drives the OpenSubsonic `avatarLastChanged` value.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use melodee_persistence::{UserAdminError, UserAdminStore};
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::{
    Authenticated, NativeState,
    error::NativeApiError,
    images::{ImageUploadValidationError, normalize_avatar_image},
};

/// Raw upload body bound; the pipeline re-encodes to the 1 MiB store bound.
const MAX_AVATAR_UPLOAD_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AvatarMutationResponse {
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

pub fn router() -> Router<NativeState> {
    Router::new().route(
        "/api/v1/user/me/avatar",
        get(get_my_avatar)
            .post(upload_my_avatar)
            .delete(delete_my_avatar)
            .layer(DefaultBodyLimit::max(MAX_AVATAR_UPLOAD_BYTES)),
    )
}

/// Detects the stored image family from magic bytes; the declared
/// Content-Type is never trusted (same rule as playlist image uploads).
fn avatar_image_format(bytes: &[u8]) -> Option<(&'static str, image::ImageFormat)> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some(("image/png", image::ImageFormat::Png))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", image::ImageFormat::Jpeg))
    } else {
        None
    }
}

async fn get_my_avatar(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    headers: HeaderMap,
) -> Response {
    let avatar = match UserAdminStore::new()
        .avatar_by_user_id(&state.pool, principal.user_id)
        .await
    {
        Ok(Some(avatar)) => avatar,
        Ok(None) => return NativeApiError::NotFound.into_response(),
        Err(_) => return NativeApiError::Unavailable.into_response(),
    };
    let etag = Sha256::digest(&avatar.image)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_matches('"') == etag)
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let mut response = (StatusCode::OK, axum::body::Body::from(avatar.image)).into_response();
    if let Ok(value) = HeaderValue::from_str(&avatar.content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) = HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

async fn upload_my_avatar(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        });
    let Some((mime, format)) = avatar_image_format(&body) else {
        return NativeApiError::BadRequest.into_response();
    };
    if declared.as_deref() != Some(mime) {
        return NativeApiError::BadRequest.into_response();
    }
    let bytes = match normalize_avatar_image(body.to_vec(), format).await {
        Ok(bytes) => bytes,
        Err(ImageUploadValidationError::Invalid) => {
            return NativeApiError::BadRequest.into_response();
        }
        Err(ImageUploadValidationError::Busy) => {
            return NativeApiError::Unavailable.into_response();
        }
    };
    let now = OffsetDateTime::now_utc();
    let stored = match UserAdminStore::new()
        .set_avatar(&state.pool, principal.user_id, mime, &bytes, now)
        .await
    {
        Ok(stored) => stored,
        Err(UserAdminError::InvalidRequest(_)) => {
            return NativeApiError::BadRequest.into_response();
        }
        Err(_) => return NativeApiError::Unavailable.into_response(),
    };
    tracing::info!(user = %principal.public_api_id, "native user avatar updated");
    Json(AvatarMutationResponse {
        message: "Avatar updated",
        updated_at: stored.updated_at.format(&Rfc3339).ok(),
    })
    .into_response()
}

async fn delete_my_avatar(
    State(state): State<NativeState>,
    Authenticated(principal, _binding): Authenticated,
) -> Response {
    match UserAdminStore::new()
        .delete_avatar(&state.pool, principal.user_id)
        .await
    {
        Ok(true) => {
            tracing::info!(user = %principal.public_api_id, "native user avatar deleted");
            Json(AvatarMutationResponse {
                message: "Avatar deleted",
                updated_at: None,
            })
            .into_response()
        }
        Ok(false) => NativeApiError::NotFound.into_response(),
        Err(_) => NativeApiError::Unavailable.into_response(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn magic_bytes_gate_matches_store_constraint() {
        assert_eq!(
            avatar_image_format(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
                .map(|(mime, _)| mime),
            Some("image/png")
        );
        assert_eq!(
            avatar_image_format(&[0xff, 0xd8, 0xff, 0xe0]).map(|(mime, _)| mime),
            Some("image/jpeg")
        );
        assert!(avatar_image_format(b"GIF89a....").is_none());
        assert!(avatar_image_format(b"RIFF....WEBP").is_none());
        assert!(avatar_image_format(b"<svg onload=alert(1)>").is_none());
        assert!(avatar_image_format(b"").is_none());
    }
}
