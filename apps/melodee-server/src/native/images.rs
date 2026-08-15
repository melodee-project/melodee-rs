//! Anonymous image endpoint consumed by native clients.
//!
//! Entity IDs are parsed as typed compatibility IDs and never become paths.
//! Catalog artwork is resolved from the immutable publication plan and opened
//! through the same no-follow canonical-root boundary used by direct play.
//! ADR-0143 §2: an operator-uploaded override (`catalog_entity_images`) takes
//! precedence for the `album_`/`artist_`/`song_` kinds; entities without an
//! override keep the captured plan-then-placeholder behavior exactly.

use std::{
    io::Cursor,
    sync::{Arc, OnceLock},
};

use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::Response,
    routing::get,
};
use image::{ImageFormat, ImageReader, Limits, imageops::FilterType};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use super::NativeState;
use crate::native_stream::PlayableAsset;

const DEFAULT_ALBUM: &[u8] = include_bytes!(
    "../../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-album-large.bin"
);
const DEFAULT_ARTIST: &[u8] = include_bytes!(
    "../../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-artist-thumb.bin"
);
const DEFAULT_PLAYLIST: &[u8] = include_bytes!(
    "../../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-playlist-thumb.bin"
);
/// Legacy default user avatar bytes; shared with the OpenSubsonic getAvatar
/// fallback (identical bytes in the frozen `os-getavatar` golden).
pub(crate) const DEFAULT_USER: &[u8] = include_bytes!(
    "../../../../compatibility/melodee-api-v1/golden/native-catalog/catalog-images-image-user-thumb.bin"
);
const MAX_IMAGE_INPUT_BYTES: u64 = 10 * 1024 * 1024;
const MAX_IMAGE_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 4096;
const IMAGE_PROCESSING_CONCURRENCY: usize = 4;

static IMAGE_PROCESSOR: OnceLock<ImageProcessor> = OnceLock::new();

pub fn router() -> Router<NativeState> {
    Router::new().route("/images/{id}/{size}", get(get_image))
}

#[derive(Debug, Clone, Copy)]
enum ImageKind {
    Album,
    Artist,
    Song,
    Playlist,
    User,
}

impl ImageKind {
    const fn content_type(self) -> &'static str {
        match self {
            // Frozen v1 reports playlist artwork as GIF even when its stored
            // bytes are the legacy JPEG placeholder.
            Self::Playlist => "image/gif",
            Self::Album | Self::Artist | Self::Song | Self::User => "image/jpeg",
        }
    }

    const fn fallback(self) -> &'static [u8] {
        match self {
            Self::Album | Self::Song => DEFAULT_ALBUM,
            Self::Artist => DEFAULT_ARTIST,
            Self::Playlist => DEFAULT_PLAYLIST,
            Self::User => DEFAULT_USER,
        }
    }

    const fn output_format(self) -> ImageFormat {
        match self {
            Self::Playlist => ImageFormat::Gif,
            Self::Album | Self::Artist | Self::Song | Self::User => ImageFormat::Jpeg,
        }
    }
}

#[derive(Debug, Clone)]
struct ImageProcessor {
    permits: Arc<Semaphore>,
}

impl ImageProcessor {
    fn new(max_concurrency: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }

    fn try_admit(&self) -> Result<OwnedSemaphorePermit, ImageProcessingError> {
        self.permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| ImageProcessingError)
    }

    async fn process(
        &self,
        input: Vec<u8>,
        requested_size: u32,
        format: ImageFormat,
        permit: OwnedSemaphorePermit,
    ) -> Result<(Vec<u8>, OwnedSemaphorePermit), ImageProcessingError> {
        if input.is_empty()
            || u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_IMAGE_INPUT_BYTES
        {
            return Err(ImageProcessingError);
        }
        let (result, permit) = tokio::task::spawn_blocking(move || {
            let result = process_image(&input, requested_size, format);
            (result, permit)
        })
        .await
        .map_err(|_| ImageProcessingError)?;
        result.map(|bytes| (bytes, permit))
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("image cannot be processed within compatibility bounds")]
struct ImageProcessingError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageUploadValidationError {
    Invalid,
    Busy,
}

/// Shared upload validation for raw image bodies (avatars, playlist images,
/// and the ADR-0143 admin catalog artwork overrides): the encoded-input
/// bound plus magic-byte/dimension/pixel decode under the bounded pipeline.
pub(crate) async fn validate_uploaded_image(
    input: Vec<u8>,
) -> Result<Vec<u8>, ImageUploadValidationError> {
    if input.is_empty() || u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_IMAGE_INPUT_BYTES {
        return Err(ImageUploadValidationError::Invalid);
    }
    let processor =
        IMAGE_PROCESSOR.get_or_init(|| ImageProcessor::new(IMAGE_PROCESSING_CONCURRENCY));
    let permit = processor
        .try_admit()
        .map_err(|_| ImageUploadValidationError::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        decode_image(&input)
            .map(|_| input)
            .map_err(|_| ImageUploadValidationError::Invalid)
    })
    .await
    .map_err(|_| ImageUploadValidationError::Invalid)?
}

fn process_image(
    input: &[u8],
    requested_size: u32,
    format: ImageFormat,
) -> Result<Vec<u8>, ImageProcessingError> {
    if !(1..=MAX_IMAGE_DIMENSION).contains(&requested_size) {
        return Err(ImageProcessingError);
    }
    let image = decode_image(input)?;
    let resized = image.resize(requested_size, requested_size, FilterType::Lanczos3);
    let mut output = Cursor::new(Vec::new());
    resized
        .write_to(&mut output, format)
        .map_err(|_| ImageProcessingError)?;
    let output = output.into_inner();
    if output.len() > MAX_IMAGE_OUTPUT_BYTES {
        return Err(ImageProcessingError);
    }
    Ok(output)
}

/// Decoded, dimension-bounded, re-encoded avatar bytes for the WP-03 native
/// avatar store. Output never exceeds the 1 MiB `native_user_avatars` bound.
pub(super) async fn normalize_avatar_image(
    input: Vec<u8>,
    format: ImageFormat,
) -> Result<Vec<u8>, ImageUploadValidationError> {
    const MAX_AVATAR_STORE_BYTES: usize = 1024 * 1024;
    const MAX_AVATAR_DIMENSION: u32 = 1024;
    if input.is_empty() || u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_IMAGE_INPUT_BYTES {
        return Err(ImageUploadValidationError::Invalid);
    }
    let processor =
        IMAGE_PROCESSOR.get_or_init(|| ImageProcessor::new(IMAGE_PROCESSING_CONCURRENCY));
    let permit = processor
        .try_admit()
        .map_err(|_| ImageUploadValidationError::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let image = decode_image(&input).map_err(|_| ImageUploadValidationError::Invalid)?;
        // Bound the long edge without ever upscaling small uploads.
        let resized =
            if image.width() > MAX_AVATAR_DIMENSION || image.height() > MAX_AVATAR_DIMENSION {
                image.resize(
                    MAX_AVATAR_DIMENSION,
                    MAX_AVATAR_DIMENSION,
                    FilterType::Lanczos3,
                )
            } else {
                image
            };
        let mut output = Cursor::new(Vec::new());
        resized
            .write_to(&mut output, format)
            .map_err(|_| ImageUploadValidationError::Invalid)?;
        let bytes = output.into_inner();
        if bytes.is_empty() || bytes.len() > MAX_AVATAR_STORE_BYTES {
            return Err(ImageUploadValidationError::Invalid);
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| ImageUploadValidationError::Invalid)?
}

fn decode_image(input: &[u8]) -> Result<image::DynamicImage, ImageProcessingError> {
    let reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| ImageProcessingError)?;
    let (width, height) = reader.into_dimensions().map_err(|_| ImageProcessingError)?;
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width).saturating_mul(u64::from(height)) > MAX_IMAGE_PIXELS
    {
        return Err(ImageProcessingError);
    }
    let mut reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| ImageProcessingError)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_PIXELS.saturating_mul(4));
    reader.limits(limits);
    reader.decode().map_err(|_| ImageProcessingError)
}

fn parse_image_id(value: &str) -> Option<(ImageKind, Uuid)> {
    let (kind, value) = value.split_once('_')?;
    let kind = match kind {
        "album" => ImageKind::Album,
        "artist" => ImageKind::Artist,
        "song" => ImageKind::Song,
        "playlist" => ImageKind::Playlist,
        "user" => ImageKind::User,
        _ => return None,
    };
    Uuid::parse_str(value).ok().map(|id| (kind, id))
}

fn image_size(value: &str) -> Option<u32> {
    if value.len() > 16 || value.is_empty() {
        return None;
    }
    match value.to_ascii_lowercase().as_str() {
        "thumbnail" => Some(80),
        "small" => Some(160),
        "medium" => Some(640),
        "large" => Some(1024),
        _ => value
            .parse::<u32>()
            .ok()
            .filter(|size| (1..=MAX_IMAGE_DIMENSION).contains(size)),
    }
}

async fn get_image(
    State(state): State<NativeState>,
    Path((raw_id, size)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some((kind, public_id)) = parse_image_id(&raw_id) else {
        return empty_ok();
    };
    let Some(size) = image_size(&size) else {
        return empty_ok();
    };
    let processor =
        IMAGE_PROCESSOR.get_or_init(|| ImageProcessor::new(IMAGE_PROCESSING_CONCURRENCY));
    let Ok(permit) = processor.try_admit() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match load_image(&state, kind, public_id, size, &headers, permit).await {
        Ok(Some(response)) => response,
        Ok(None) => empty_ok(),
        Err(error) => {
            tracing::error!(?error, image_kind = ?kind, "native image lookup failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

async fn load_image(
    state: &NativeState,
    kind: ImageKind,
    public_id: Uuid,
    size: u32,
    headers: &HeaderMap,
    permit: OwnedSemaphorePermit,
) -> Result<Option<Response<Body>>, sqlx::Error> {
    match kind {
        ImageKind::Playlist => {
            let image = sqlx::query_as::<_, (Option<String>, Option<Vec<u8>>)>(
                "SELECT image_mime_type, image_bytes FROM native_playlists WHERE public_api_id = $1",
            )
            .bind(public_id)
            .fetch_optional(&state.pool)
            .await?;
            match image {
                Some((_, Some(bytes))) => Ok(Some(
                    processed_response(kind, bytes, size, headers, permit).await?,
                )),
                Some((_, None)) => Ok(Some(byte_response(
                    kind.fallback().to_vec(),
                    kind.content_type(),
                    headers,
                    Some(permit),
                ))),
                None => Ok(None),
            }
        }
        ImageKind::User => {
            // WP-03: `native_user_avatars` is the avatar authority; the
            // migration-0010 columns remain the legacy-import fallback.
            let image = sqlx::query_as::<_, (Option<String>, Option<Vec<u8>>)>(
                "SELECT COALESCE(a.content_type, u.image_mime_type), COALESCE(a.image, u.image_bytes)
                 FROM native_users u
                 LEFT JOIN native_user_avatars a ON a.user_id = u.id
                 WHERE u.public_api_id = $1",
            )
            .bind(public_id)
            .fetch_optional(&state.pool)
            .await?;
            match image {
                Some((_, Some(bytes))) => Ok(Some(
                    processed_response(kind, bytes, size, headers, permit).await?,
                )),
                Some((_, None)) => Ok(Some(byte_response(
                    kind.fallback().to_vec(),
                    kind.content_type(),
                    headers,
                    Some(permit),
                ))),
                None => Ok(None),
            }
        }
        ImageKind::Album | ImageKind::Artist | ImageKind::Song => {
            // ADR-0143 §2: an operator-uploaded artwork override takes
            // precedence over the immutable publication plan. Absence of an
            // override preserves the captured behavior exactly below (plan
            // artwork, then placeholder).
            if let Some(bytes) = artwork_override_bytes(&state.pool, kind, public_id).await? {
                return Ok(Some(
                    processed_response(kind, bytes, size, headers, permit).await?,
                ));
            }
            let release = resolve_release(&state.pool, kind, public_id).await?;
            let Some(release_id) = release else {
                return Ok(None);
            };
            if let Some(bytes) = canonical_artwork_bytes(state, release_id).await? {
                return Ok(Some(
                    processed_response(kind, bytes, size, headers, permit).await?,
                ));
            }
            Ok(Some(byte_response(
                kind.fallback().to_vec(),
                kind.content_type(),
                headers,
                Some(permit),
            )))
        }
    }
}

async fn resolve_release(
    pool: &sqlx::PgPool,
    kind: ImageKind,
    public_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    match kind {
        ImageKind::Album => {
            sqlx::query_scalar(
                "SELECT id FROM releases WHERE public_api_id = $1 AND status = 'active'",
            )
                .bind(public_id)
                .fetch_optional(pool)
                .await
        }
        ImageKind::Song => {
            sqlx::query_scalar(
                "SELECT release.id FROM tracks track
                 JOIN releases release ON release.id = track.release_id AND release.status = 'active'
                 WHERE track.public_api_id = $1",
            )
            .bind(public_id)
            .fetch_optional(pool)
            .await
        }
        ImageKind::Artist => {
            sqlx::query_scalar(
                "SELECT billing.release_id FROM artists artist
                 JOIN release_artists billing ON billing.artist_id = artist.id
                 JOIN releases release ON release.id = billing.release_id
                 WHERE artist.public_api_id = $1 AND release.status = 'active'
                 ORDER BY release.release_date DESC NULLS LAST, release.public_api_id LIMIT 1",
            )
            .bind(public_id)
            .fetch_optional(pool)
            .await
        }
        ImageKind::Playlist | ImageKind::User => Ok(None),
    }
}

/// ADR-0143 §2 precedence lookup: the operator-uploaded override bytes for
/// the addressed entity, when one is stored. `album_`/`song_` resolve to the
/// Release override (a Song's artwork is its Release's, as today); `artist_`
/// resolves to the Artist override. The same status gates as
/// `resolve_release` apply so an override never revives an entity the frozen
/// endpoint would not serve.
async fn artwork_override_bytes(
    pool: &sqlx::PgPool,
    kind: ImageKind,
    public_id: Uuid,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let query = match kind {
        ImageKind::Album => {
            "SELECT img.image_bytes FROM catalog_entity_images img
             JOIN releases release ON release.id = img.release_id AND release.status = 'active'
             WHERE img.entity_type = 'release' AND release.public_api_id = $1"
        }
        ImageKind::Song => {
            "SELECT img.image_bytes FROM catalog_entity_images img
             JOIN releases release ON release.id = img.release_id AND release.status = 'active'
             JOIN tracks track ON track.release_id = release.id
             WHERE img.entity_type = 'release' AND track.public_api_id = $1"
        }
        ImageKind::Artist => {
            "SELECT img.image_bytes FROM catalog_entity_images img
             JOIN artists artist ON artist.id = img.artist_id
             WHERE img.entity_type = 'artist' AND artist.public_api_id = $1"
        }
        ImageKind::Playlist | ImageKind::User => return Ok(None),
    };
    sqlx::query_scalar(query)
        .bind(public_id)
        .fetch_optional(pool)
        .await
}

async fn canonical_artwork_bytes(
    state: &NativeState,
    release_id: Uuid,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT plan.target_relative_path || '/' || (asset.value->>'canonical_path'),
                (asset.value->>'size_bytes')::bigint, asset.value->>'byte_hash'
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
        .read_asset_bounded(&asset, MAX_IMAGE_INPUT_BYTES)
        .await
    else {
        return Ok(None);
    };
    let _source_hash: String = row.try_get(2)?;
    Ok(Some(bytes))
}

async fn processed_response(
    kind: ImageKind,
    bytes: Vec<u8>,
    size: u32,
    headers: &HeaderMap,
    permit: OwnedSemaphorePermit,
) -> Result<Response<Body>, sqlx::Error> {
    let processor =
        IMAGE_PROCESSOR.get_or_init(|| ImageProcessor::new(IMAGE_PROCESSING_CONCURRENCY));
    processor
        .process(bytes, size, kind.output_format(), permit)
        .await
        .map(|(bytes, permit)| byte_response(bytes, kind.content_type(), headers, Some(permit)))
        .map_err(|_| {
            sqlx::Error::Protocol("image processing failed within compatibility bounds".to_string())
        })
}

fn byte_response(
    bytes: Vec<u8>,
    content_type: &'static str,
    request_headers: &HeaderMap,
    permit: Option<OwnedSemaphorePermit>,
) -> Response<Body> {
    let etag = hex_digest(&bytes);
    if not_modified(request_headers, &etag) {
        return not_modified_response(&etag);
    }
    let length = bytes.len();
    let body = match permit {
        Some(permit) => Body::from(Bytes::from_owner(PermittedBytes {
            bytes,
            _permit: permit,
        })),
        None => Body::from(bytes),
    };
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(length) = HeaderValue::from_str(&length.to_string()) {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, length);
    }
    insert_cache_headers(response.headers_mut(), &etag);
    response
}

struct PermittedBytes {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for PermittedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim_matches('"') == etag)
}

fn not_modified_response(etag: &str) -> Response<Body> {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_MODIFIED;
    insert_cache_headers(response.headers_mut(), etag);
    response
}

fn insert_cache_headers(headers: &mut HeaderMap, etag: &str) {
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
}

fn empty_ok() -> Response<Body> {
    Response::new(Body::empty())
}

use axum::response::IntoResponse as _;

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn image_ids_and_sizes_are_bounded_and_typed() {
        let id = Uuid::nil();
        assert!(parse_image_id(&format!("album_{id}")).is_some());
        assert!(parse_image_id(&format!("../../album_{id}")).is_none());
        assert_eq!(image_size("80"), Some(80));
        assert_eq!(image_size("large"), Some(1024));
        assert_eq!(image_size("0"), None);
        assert_eq!(image_size("99999999999999999"), None);
    }

    #[test]
    fn image_processing_is_deterministic_and_honors_requested_dimensions() {
        let first = process_image(DEFAULT_USER, 80, ImageFormat::Jpeg).expect("process avatar");
        let second = process_image(DEFAULT_USER, 80, ImageFormat::Jpeg).expect("process avatar");
        assert_eq!(first, second);
        let dimensions = ImageReader::new(Cursor::new(first))
            .with_guessed_format()
            .expect("format")
            .into_dimensions()
            .expect("dimensions");
        assert_eq!(dimensions, (80, 80));
    }

    #[test]
    fn unsupported_and_dimension_bomb_inputs_fail_closed() {
        assert!(process_image(b"not an image", 80, ImageFormat::Jpeg).is_err());
        let image =
            image::DynamicImage::ImageRgba8(image::RgbaImage::new(MAX_IMAGE_DIMENSION + 1, 1));
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode bounded bomb header");
        assert!(process_image(&encoded.into_inner(), 80, ImageFormat::Jpeg).is_err());
    }

    #[tokio::test]
    async fn encoded_input_byte_limit_is_checked_before_decode() {
        let processor = ImageProcessor::new(1);
        let too_large = vec![0_u8; (MAX_IMAGE_INPUT_BYTES + 1) as usize];
        let permit = processor.try_admit().expect("image admission");
        assert!(
            processor
                .process(too_large, 80, ImageFormat::Jpeg, permit)
                .await
                .is_err()
        );
        assert_eq!(processor.permits.available_permits(), 1);
    }

    #[test]
    fn anonymous_image_processing_concurrency_is_explicitly_bounded() {
        let processor = ImageProcessor::new(1);
        let permit = processor.try_admit().expect("first permit");
        let admitted = Bytes::from_owner(PermittedBytes {
            bytes: vec![1, 2, 3],
            _permit: permit,
        });
        let in_flight = admitted.clone();
        assert!(processor.try_admit().is_err());
        drop(admitted);
        assert!(processor.try_admit().is_err());
        drop(in_flight);
        assert_eq!(processor.permits.available_permits(), 1);
    }
}
