//! Jellyfin image endpoints (PRD-API-JF-004).
//!
//! Reproduced from the frozen baseline (`ImagesController.cs`):
//!
//! - `/Items/{id}/Images/{type}[/{index}]` probes artist → album → song
//!   (songs serve their release artwork); `/Artists/{id}/Images/{type}`.
//! - Size buckets from maxWidth/maxHeight: none → Large, ≤100 → Small,
//!   ≤300 → Medium, else Large.
//! - Success: sniffed content type, `Cache-Control: public, max-age=86400`,
//!   ETag + If-None-Match → 304, HEAD headers-only.
//! - No-artwork classes: "Artist image not found." / "Album image not
//!   found." / "Song album image not found." / "Item not found."
//!
//! Artwork resolution observes the same immutable publication-plan and
//! operator-override sources as the native endpoint (ADR-0143). The captured
//! no-artwork 404 classes remain the fallback when neither source has bytes.

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use image::ImageFormat;
use sha2::{Digest, Sha256};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use super::JfState;
use super::auth::resolve_token;
use super::browse::{db_failure, raw_query};
use super::error::{bad_request, not_found, unauthorized};
use super::model;
use crate::native_stream::PlayableAsset;

const MAX_IMAGE_BYTES: u64 = 10 * 1024 * 1024;

pub fn router() -> Router<JfState> {
    Router::new()
        .route("/Items/{id}/Images/{image_type}", get(item_image))
        .route(
            "/Items/{id}/Images/{image_type}/{index}",
            get(item_image_indexed),
        )
        .route("/Artists/{id}/Images/{image_type}", get(artist_image))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageSizeBucket {
    Small,
    Medium,
    Large,
}

/// The captured bucket boundaries (`DetermineSizeFromDimensions`).
fn bucket(max_width: Option<i64>, max_height: Option<i64>) -> ImageSizeBucket {
    let max_dimension = max_width.unwrap_or(0).max(max_height.unwrap_or(0));
    match max_dimension {
        0 => ImageSizeBucket::Large,
        1..=100 => ImageSizeBucket::Small,
        101..=300 => ImageSizeBucket::Medium,
        _ => ImageSizeBucket::Large,
    }
}

struct ResolvedImage {
    bytes: Vec<u8>,
}

/// Resolves an operator override first, then immutable canonical artwork for
/// the addressed catalog entity. This does not alter the captured 404 branch:
/// an entity without either source remains no-artwork.
async fn resolve_image(
    state: &JfState,
    kind: &str,
    public_id: Uuid,
    size: ImageSizeBucket,
) -> Result<Option<ResolvedImage>, sqlx::Error> {
    if let Some(bytes) = artwork_override_bytes(&state.pool, kind, public_id).await? {
        return processed_image(bytes, size).await.map(Some);
    }

    let release_id = resolve_release(&state.pool, kind, public_id).await?;
    let Some(release_id) = release_id else {
        return Ok(None);
    };
    match canonical_artwork_bytes(state, release_id).await? {
        Some(bytes) => processed_image(bytes, size).await.map(Some),
        None => Ok(None),
    }
}

const fn output_size(size: ImageSizeBucket) -> u32 {
    match size {
        ImageSizeBucket::Small => 100,
        ImageSizeBucket::Medium => 300,
        ImageSizeBucket::Large => 1024,
    }
}

/// Jellyfin's size-bucket contract shares native's bounded processor while
/// preserving the source PNG/JPEG format expected by Jellyfin clients.
async fn processed_image(
    bytes: Vec<u8>,
    size: ImageSizeBucket,
) -> Result<ResolvedImage, sqlx::Error> {
    let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        ImageFormat::Png
    } else {
        // Published artwork is canonical JPEG and catalog overrides only
        // admit JPEG/PNG through the admin validation boundary.
        ImageFormat::Jpeg
    };
    crate::native::images::resize_compatibility_image(bytes, output_size(size), format)
        .await
        .map(|bytes| ResolvedImage { bytes })
        .map_err(|_| sqlx::Error::Protocol("Jellyfin image processing failed".to_string()))
}

/// ADR-0143 precedence lookup. Tracks resolve their Release override; Artist
/// overrides remain artist-scoped even when the artist has no released media.
async fn artwork_override_bytes(
    pool: &sqlx::PgPool,
    kind: &str,
    public_id: Uuid,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let query = match kind {
        "artist" => {
            "SELECT image.asset_key
             FROM catalog_entity_images image
             JOIN artists artist ON artist.id = image.artist_id
             WHERE image.entity_type = 'artist'
               AND artist.public_api_id = $1
               AND artist.status NOT IN ('merged', 'deprecated')"
        }
        "album" => {
            "SELECT image.asset_key
             FROM catalog_entity_images image
             JOIN releases release ON release.id = image.release_id
             WHERE image.entity_type = 'release'
               AND release.public_api_id = $1
               AND release.status = 'active'"
        }
        _ => return Ok(None),
    };
    let override_bytes = sqlx::query_scalar(query)
        .bind(public_id)
        .fetch_optional(pool)
        .await?;
    if override_bytes.is_some() {
        return melodee_persistence::asset_files::read_optional(override_bytes).await;
    }
    let provider_query = match kind {
        "artist" => {
            "SELECT image.asset_key FROM catalog_provider_images image
             JOIN artists artist ON artist.id=image.entity_id
             WHERE image.entity_type='artist' AND artist.public_api_id=$1
               AND artist.status NOT IN ('merged','deprecated')
             ORDER BY image.confidence DESC, image.observed_at DESC, image.id DESC LIMIT 1"
        }
        "album" => {
            "SELECT image.asset_key FROM catalog_provider_images image
             JOIN releases release ON release.id=image.entity_id
             WHERE image.entity_type='release' AND release.public_api_id=$1
               AND release.status='active'
             ORDER BY image.confidence DESC, image.observed_at DESC, image.id DESC LIMIT 1"
        }
        _ => return Ok(None),
    };
    let key = sqlx::query_scalar(provider_query)
        .bind(public_id)
        .fetch_optional(pool)
        .await?;
    melodee_persistence::asset_files::read_optional(key).await
}

/// The immutable publication-plan artwork source used after the operator
/// override lookup. `DirectPlayService` retains no-follow containment and a
/// second byte bound while the asset is read.
async fn canonical_artwork_bytes(
    state: &JfState,
    release_id: Uuid,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT plan.target_relative_path || '/' || (asset.value->>'canonical_path'),
                (asset.value->>'size_bytes')::bigint
         FROM catalog_imports catalog_import
         JOIN release_publication_plans plan ON plan.work_item_id = catalog_import.work_item_id
         CROSS JOIN LATERAL jsonb_array_elements(COALESCE(plan.plan_payload->'assets', '[]')) asset(value)
         WHERE catalog_import.release_id = $1 AND asset.value->>'kind' = 'artwork'
         ORDER BY asset.value->>'canonical_path' LIMIT 1",
    )
    .bind(release_id)
    .fetch_optional(&state.pool)
    .await?;
    let (Some(service), Some(row)) = (state.direct_play.as_ref(), row) else {
        return Ok(None);
    };
    let byte_size: i64 = row.try_get(1)?;
    let Ok(byte_size) = u64::try_from(byte_size) else {
        return Ok(None);
    };
    let asset = PlayableAsset {
        canonical_relative_path: row.try_get(0)?,
        byte_size,
        mime_type: "image/jpeg".to_string(),
    };
    let Ok(bytes) = service
        .read_asset_bounded(&asset, MAX_IMAGE_BYTES, None)
        .await
    else {
        return Ok(None);
    };
    Ok(Some(bytes))
}

async fn resolve_release(
    pool: &sqlx::PgPool,
    kind: &str,
    public_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    match kind {
        "album" => {
            sqlx::query_scalar(
                "SELECT id FROM releases WHERE public_api_id = $1 AND status = 'active'",
            )
            .bind(public_id)
            .fetch_optional(pool)
            .await
        }
        "artist" => {
            sqlx::query_scalar(
                "SELECT billing.release_id FROM artists artist
                 JOIN release_artists billing ON billing.artist_id = artist.id
                 JOIN releases release ON release.id = billing.release_id
                 WHERE artist.public_api_id = $1
                   AND artist.status NOT IN ('merged', 'deprecated')
                   AND release.status = 'active'
                 ORDER BY release.release_date DESC NULLS LAST, release.public_api_id LIMIT 1",
            )
            .bind(public_id)
            .fetch_optional(pool)
            .await
        }
        _ => Ok(None),
    }
}

/// Captured magic-byte content sniffing (default image/jpeg).
fn sniff_content_type(bytes: &[u8]) -> &'static str {
    if bytes.len() < 8 {
        return "image/jpeg";
    }
    if bytes[0] == 0x89 && bytes[1] == 0x50 && bytes[2] == 0x4E && bytes[3] == 0x47 {
        return "image/png";
    }
    if bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return "image/jpeg";
    }
    if bytes[0] == 0x47 && bytes[1] == 0x49 && bytes[2] == 0x46 && bytes[3] == 0x38 {
        return "image/gif";
    }
    if bytes[0] == 0x52
        && bytes[1] == 0x49
        && bytes[2] == 0x46
        && bytes[3] == 0x46
        && bytes.len() > 11
        && bytes[8] == 0x57
        && bytes[9] == 0x45
        && bytes[10] == 0x42
        && bytes[11] == 0x50
    {
        return "image/webp";
    }
    if bytes[0] == 0x42 && bytes[1] == 0x4D {
        return "image/bmp";
    }
    "image/jpeg"
}

fn image_response(image: ResolvedImage, headers: &HeaderMap, is_head: bool) -> Response {
    // Content-derived tags avoid a stale 304 if an operator replaces an
    // override more than once in a single timestamp tick.
    let etag = format!("\"{}\"", hex_digest(&image.bytes));
    let not_modified = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_matches('"') == etag.trim_matches('"'));
    if not_modified {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(value) = HeaderValue::from_str(&etag) {
            response.headers_mut().insert(header::ETAG, value);
        }
        return response;
    }
    let content_type = sniff_content_type(&image.bytes);
    if is_head {
        let mut response = StatusCode::OK.into_response();
        insert(&mut response, header::CONTENT_TYPE, content_type);
        insert(
            &mut response,
            header::CONTENT_LENGTH,
            &image.bytes.len().to_string(),
        );
        insert(
            &mut response,
            header::CACHE_CONTROL,
            "public, max-age=86400",
        );
        insert(&mut response, header::ETAG, &etag);
        return response;
    }
    let mut response = (StatusCode::OK, image.bytes).into_response();
    insert(&mut response, header::CONTENT_TYPE, content_type);
    insert(
        &mut response,
        header::CACHE_CONTROL,
        "public, max-age=86400",
    );
    insert(&mut response, header::ETAG, &etag);
    response
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn insert(response: &mut Response, name: header::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

fn dimensions(uri: &Uri) -> (Option<i64>, Option<i64>) {
    let mut max_width = None;
    let mut max_height = None;
    for (key, value) in raw_query(uri) {
        match key.as_str() {
            "maxWidth" => max_width = value.parse::<i64>().ok(),
            "maxHeight" => max_height = value.parse::<i64>().ok(),
            _ => {}
        }
    }
    (max_width, max_height)
}

/// Entity probe for /Items images: artist → album → song (songs serve their
/// release artwork; captured detail strings on the 404 classes).
async fn item_image_response(state: JfState, id: String, uri: Uri, headers: HeaderMap) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid item ID format.");
    };
    let (max_width, max_height) = dimensions(&uri);
    let size = bucket(max_width, max_height);
    let is_head = false; // axum serves HEAD through the same handler and strips the body

    let is_artist = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM artists WHERE public_api_id = $1 AND status NOT IN ('merged', 'deprecated')",
    )
    .bind(public_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0)
        > 0;
    if is_artist {
        return match resolve_image(&state, "artist", public_id, size).await {
            Ok(Some(image)) => image_response(image, &headers, is_head),
            Ok(None) => not_found("Artist image not found."),
            Err(error) => db_failure("images/artist", error),
        };
    }

    let album_created: Option<OffsetDateTime> = sqlx::query(
        "SELECT created_at FROM releases WHERE public_api_id = $1 AND status = 'active'",
    )
    .bind(public_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .map(|row| row.get(0));
    if album_created.is_some() {
        return match resolve_image(&state, "album", public_id, size).await {
            Ok(Some(image)) => image_response(image, &headers, is_head),
            Ok(None) => not_found("Album image not found."),
            Err(error) => db_failure("images/album", error),
        };
    }

    let song_album: Option<Uuid> = sqlx::query(
        "SELECT release.public_api_id FROM tracks track \
         JOIN releases release ON release.id = track.release_id AND release.status = 'active' \
         WHERE track.public_api_id = $1",
    )
    .bind(public_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .map(|row| row.get(0));
    if let Some(album_id) = song_album {
        return match resolve_image(&state, "album", album_id, size).await {
            Ok(Some(image)) => image_response(image, &headers, is_head),
            Ok(None) => not_found("Song album image not found."),
            Err(error) => db_failure("images/song", error),
        };
    }

    not_found("Item not found.")
}

async fn item_image(
    State(state): State<JfState>,
    Path((id, _image_type)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    item_image_response(state, id, uri, headers).await
}

async fn item_image_indexed(
    State(state): State<JfState>,
    Path((id, _image_type, _index)): Path<(String, String, i32)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    item_image_response(state, id, uri, headers).await
}

async fn artist_image(
    State(state): State<JfState>,
    Path((id, _image_type)): Path<(String, String)>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if resolve_token(&state, &headers, &uri).await.is_none() {
        return unauthorized();
    }
    let Some(public_id) = model::parse_id(&id) else {
        return bad_request("Invalid artist ID format.");
    };
    let (max_width, max_height) = dimensions(&uri);
    let size = bucket(max_width, max_height);
    match resolve_image(&state, "artist", public_id, size).await {
        Ok(Some(image)) => image_response(image, &headers, false),
        Ok(None) => not_found("Artist image not found."),
        Err(error) => db_failure("images/artist", error),
    }
}
