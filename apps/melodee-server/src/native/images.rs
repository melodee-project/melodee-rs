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
    time::Duration,
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
/// How long an artwork read waits for a processing permit before the endpoint
/// reports `503`. A catalogue page requests one cover per row at once, so an
/// instant rejection turned an ordinary browse into a wall of fallback initials
/// (measured: 20 of 24 concurrent reads refused). The wait is bounded so a
/// stalled client can never pin the pipeline indefinitely.
const IMAGE_ADMISSION_WAIT: Duration = Duration::from_secs(10);

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
    gauge: Option<Arc<crate::resource_bounds::ResourceGauge>>,
}

impl ImageProcessor {
    fn new(max_concurrency: usize) -> Self {
        let max = max_concurrency.max(1);
        Self {
            permits: Arc::new(Semaphore::new(max)),
            // image processing reports utilization and non-blocking
            // rejections through the process resource-bounds registry when
            // startup installed one.
            gauge: crate::resource_bounds::ResourceBounds::global()
                .map(|registry| registry.register("images.processing", max)),
        }
    }

    fn try_admit(&self) -> Result<ImageAdmission, ImageProcessingError> {
        match self.permits.clone().try_acquire_owned() {
            Ok(permit) => Ok(ImageAdmission {
                _semaphore: permit,
                _gauge: self
                    .gauge
                    .as_ref()
                    .map(crate::resource_bounds::ResourceGauge::admit_immediate),
            }),
            Err(error) => {
                if let Some(gauge) = &self.gauge {
                    gauge.reject();
                }
                let _ = error;
                Err(ImageProcessingError)
            }
        }
    }

    /// Waits up to `wait` for a processing permit instead of refusing at once.
    ///
    /// Mutations (uploads) keep the non-blocking [`Self::try_admit`] contract:
    /// refusing an upload under load is a clear answer the caller can retry.
    /// A read has no such option — the browser simply gives up and shows an
    /// initial — so reads queue for a bounded moment.
    async fn admit_bounded(&self, wait: Duration) -> Result<ImageAdmission, ImageProcessingError> {
        if let Ok(permit) = self.permits.clone().try_acquire_owned() {
            return Ok(ImageAdmission {
                _semaphore: permit,
                _gauge: self.gauge.as_ref().map(|gauge| gauge.admit_immediate()),
            });
        }
        if let Some(gauge) = &self.gauge {
            gauge.begin_wait();
        }
        let awaited = tokio::time::timeout(wait, self.permits.clone().acquire_owned()).await;
        match awaited {
            Ok(Ok(permit)) => Ok(ImageAdmission {
                _semaphore: permit,
                _gauge: self.gauge.as_ref().map(|gauge| gauge.admit()),
            }),
            Ok(Err(_)) | Err(_) => {
                if let Some(gauge) = &self.gauge {
                    gauge.wait_failed();
                }
                Err(ImageProcessingError)
            }
        }
    }

    async fn process(
        &self,
        input: Vec<u8>,
        requested_size: u32,
        format: ImageFormat,
        no_upscale: bool,
        admission: ImageAdmission,
    ) -> Result<(Vec<u8>, ImageAdmission), ImageProcessingError> {
        if input.is_empty()
            || u64::try_from(input.len()).unwrap_or(u64::MAX) > MAX_IMAGE_INPUT_BYTES
        {
            return Err(ImageProcessingError);
        }
        let (result, admission) = tokio::task::spawn_blocking(move || {
            let result = process_image(&input, requested_size, format, no_upscale);
            (result, admission)
        })
        .await
        .map_err(|_| ImageProcessingError)?;
        result.map(|bytes| (bytes, admission))
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("image cannot be processed within compatibility bounds")]
struct ImageProcessingError;

/// RAII admission token for the bounded image pipeline: bundles the
/// concurrency semaphore permit with the optional resource-bounds gauge
/// permit so both release together on drop, including client disconnect while
/// the response body is still in flight.
#[derive(Debug)]
struct ImageAdmission {
    _semaphore: OwnedSemaphorePermit,
    _gauge: Option<crate::resource_bounds::GaugePermit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageUploadValidationError {
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

/// Resizes a catalog image through the shared bounded image processor for a
/// non-native compatibility adapter. The adapter retains ownership of its
/// wire response and MIME semantics; this function only enforces bounded
/// decode/encode work and the process-wide image concurrency limit.
pub(crate) async fn resize_compatibility_image(
    input: Vec<u8>,
    requested_size: u32,
    format: ImageFormat,
) -> Result<Vec<u8>, ImageUploadValidationError> {
    let processor =
        IMAGE_PROCESSOR.get_or_init(|| ImageProcessor::new(IMAGE_PROCESSING_CONCURRENCY));
    let permit = processor
        .try_admit()
        .map_err(|_| ImageUploadValidationError::Busy)?;
    let (bytes, _permit) = processor
        .process(input, requested_size, format, false, permit)
        .await
        .map_err(|_| ImageUploadValidationError::Invalid)?;
    Ok(bytes)
}

/// OpenSubsonic stored-artwork sizing is a maximum dimension and explicitly
/// forbids upscaling. Native-v1 and Jellyfin continue to use their captured
/// exact-size behavior through `resize_compatibility_image`.
pub(crate) async fn resize_compatibility_image_no_upscale(
    input: Vec<u8>,
    requested_size: u32,
    format: ImageFormat,
) -> Result<Vec<u8>, ImageUploadValidationError> {
    let processor =
        IMAGE_PROCESSOR.get_or_init(|| ImageProcessor::new(IMAGE_PROCESSING_CONCURRENCY));
    let permit = processor
        .try_admit()
        .map_err(|_| ImageUploadValidationError::Busy)?;
    let (bytes, _permit) = processor
        .process(input, requested_size, format, true, permit)
        .await
        .map_err(|_| ImageUploadValidationError::Invalid)?;
    Ok(bytes)
}

fn process_image(
    input: &[u8],
    requested_size: u32,
    format: ImageFormat,
    no_upscale: bool,
) -> Result<Vec<u8>, ImageProcessingError> {
    if !(1..=MAX_IMAGE_DIMENSION).contains(&requested_size) {
        return Err(ImageProcessingError);
    }
    let image = decode_image(input)?;
    let target = if no_upscale {
        image.width().max(image.height()).min(requested_size)
    } else {
        requested_size
    };
    let resized = image.resize(target, target, FilterType::Lanczos3);
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

/// Decoded, dimension-bounded, re-encoded avatar bytes for the native
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

/// Normalizes a legacy User Images avatar through the same bounded pipeline
/// as native uploads, with PNG as the durable migration representation.
pub async fn normalize_legacy_avatar_image(
    input: Vec<u8>,
) -> Result<Vec<u8>, ImageUploadValidationError> {
    normalize_avatar_image(input, ImageFormat::Png).await
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

/// A resolved image request size.
///
/// `Original` exists because the Admin UI (and native clients) address
/// `/images/album_<id>/original` for the untransformed source. Before this
/// existed the literal token failed `u32` parsing, so the handler answered
/// `200` with an empty body and no content type — the request never reached
/// artwork resolution at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestedSize {
    /// Longest-edge target; decoded, resized, and re-encoded.
    Pixels(u32),
    /// Source bytes served untouched (no decode, no re-encode).
    Original,
}

fn image_size(value: &str) -> Option<RequestedSize> {
    if value.len() > 16 || value.is_empty() {
        return None;
    }
    match value.to_ascii_lowercase().as_str() {
        "thumbnail" => Some(RequestedSize::Pixels(80)),
        "small" => Some(RequestedSize::Pixels(160)),
        "medium" => Some(RequestedSize::Pixels(640)),
        "large" => Some(RequestedSize::Pixels(1024)),
        "original" | "source" => Some(RequestedSize::Original),
        _ => value
            .parse::<u32>()
            .ok()
            .filter(|size| (1..=MAX_IMAGE_DIMENSION).contains(size))
            .map(RequestedSize::Pixels),
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
    // `original` serves stored bytes without decoding, so it neither needs nor
    // should consume a decode permit; sized reads queue for one (bounded).
    let admission = match size {
        RequestedSize::Original => None,
        RequestedSize::Pixels(_) => match processor.admit_bounded(IMAGE_ADMISSION_WAIT).await {
            Ok(admission) => Some(admission),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        },
    };
    match load_image(&state, kind, public_id, size, &headers, admission).await {
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
    size: RequestedSize,
    headers: &HeaderMap,
    admission: Option<ImageAdmission>,
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
                    processed_response(kind, bytes, size, headers, admission).await?,
                )),
                Some((_, None)) => Ok(Some(byte_response(
                    kind.fallback().to_vec(),
                    kind.content_type(),
                    headers,
                    admission,
                ))),
                None => Ok(None),
            }
        }
        ImageKind::User => {
            // `native_user_avatars` is the avatar authority; the
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
                    processed_response(kind, bytes, size, headers, admission).await?,
                )),
                Some((_, None)) => Ok(Some(byte_response(
                    kind.fallback().to_vec(),
                    kind.content_type(),
                    headers,
                    admission,
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
                tracing::debug!(
                    ?kind,
                    %public_id,
                    bytes = bytes.len(),
                    "artwork resolved from the operator override"
                );
                return Ok(Some(
                    processed_response(kind, bytes, size, headers, admission).await?,
                ));
            }
            let release = resolve_release(&state.pool, kind, public_id).await?;
            let Some(release_id) = release else {
                tracing::debug!(?kind, %public_id, "artwork lookup found no active entity");
                return Ok(None);
            };
            if let Some(bytes) = canonical_artwork_bytes(state, release_id).await? {
                tracing::debug!(
                    ?kind,
                    %public_id,
                    %release_id,
                    bytes = bytes.len(),
                    "artwork resolved from the published plan"
                );
                return Ok(Some(
                    processed_response(kind, bytes, size, headers, admission).await?,
                ));
            }
            if let Some(bytes) = provider_artwork_bytes(&state.pool, kind, public_id).await? {
                tracing::debug!(
                    ?kind,
                    %public_id,
                    %release_id,
                    bytes = bytes.len(),
                    "artwork resolved from a provider candidate"
                );
                return Ok(Some(
                    processed_response(kind, bytes, size, headers, admission).await?,
                ));
            }
            tracing::debug!(
                ?kind,
                %public_id,
                %release_id,
                bytes = kind.fallback().len(),
                "artwork resolved to the captured placeholder"
            );
            Ok(Some(byte_response(
                kind.fallback().to_vec(),
                kind.content_type(),
                headers,
                admission,
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

/// Provider-sourced artwork (`catalog_provider_images`) for the addressed
/// entity. ADR-0143 §2 lists the override, the publication plan, and the
/// placeholder; provider bytes rank *between* plan artwork and the
/// placeholder, so a cover the operator actually imported is never silently
/// replaced by a provider guess, while a Release with no artwork still shows
/// the best available cover.
async fn provider_artwork_bytes(
    pool: &sqlx::PgPool,
    kind: ImageKind,
    public_id: Uuid,
) -> Result<Option<Vec<u8>>, sqlx::Error> {
    let provider_query = match kind {
        ImageKind::Album => {
            "SELECT image.image_bytes FROM catalog_provider_images image
             JOIN releases release ON release.id=image.entity_id
             WHERE image.entity_type='release' AND release.public_api_id=$1
               AND release.status='active'
             ORDER BY image.confidence DESC, image.observed_at DESC, image.id DESC LIMIT 1"
        }
        ImageKind::Song => {
            "SELECT image.image_bytes FROM catalog_provider_images image
             JOIN releases release ON release.id=image.entity_id
             JOIN tracks track ON track.release_id=release.id
             WHERE image.entity_type='release' AND track.public_api_id=$1
               AND release.status='active'
             ORDER BY image.confidence DESC, image.observed_at DESC, image.id DESC LIMIT 1"
        }
        ImageKind::Artist => {
            "SELECT image.image_bytes FROM catalog_provider_images image
             JOIN artists artist ON artist.id=image.entity_id
             WHERE image.entity_type='artist' AND artist.public_api_id=$1
               AND artist.status NOT IN ('merged','deprecated')
             ORDER BY image.confidence DESC, image.observed_at DESC, image.id DESC LIMIT 1"
        }
        ImageKind::Playlist | ImageKind::User => return Ok(None),
    };
    sqlx::query_scalar(provider_query)
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
        .read_asset_bounded(&asset, MAX_IMAGE_INPUT_BYTES, None)
        .await
    else {
        // Silence here is undiagnosable in production: a plan asset that cannot
        // be read is served as the placeholder, so record why.
        tracing::debug!(
            %release_id,
            path = %asset.canonical_relative_path,
            byte_size = asset.byte_size,
            "canonical artwork asset could not be read"
        );
        return Ok(None);
    };
    // Integrity fence: the recorded publication hash must match the bytes on
    // disk, so swapped or corrupted storage is treated as absent artwork
    // (the captured 404 fallback) rather than served. BLAKE3 over the
    // already-bounded in-memory bytes.
    let source_hash: String = row.try_get(2)?;
    if blake3::hash(&bytes).to_hex().as_str() != source_hash {
        tracing::warn!(
            release = %release_id,
            path = %asset.canonical_relative_path,
            "canonical artwork byte-hash mismatch; serving captured no-artwork fallback"
        );
        return Ok(None);
    }
    Ok(Some(bytes))
}

async fn processed_response(
    kind: ImageKind,
    bytes: Vec<u8>,
    size: RequestedSize,
    headers: &HeaderMap,
    admission: Option<ImageAdmission>,
) -> Result<Response<Body>, sqlx::Error> {
    // ADR-0143 §2: `original` is the untransformed publication asset. Serving
    // it must not pass through the decoder, because a source the pipeline
    // accepted may still exceed the decoder's own bounds; it is bounded by the
    // input cap that produced it instead.
    if size == RequestedSize::Original {
        if bytes.is_empty()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IMAGE_INPUT_BYTES
        {
            return Err(sqlx::Error::Protocol(
                "original image exceeds compatibility bounds".to_string(),
            ));
        }
        let content_type = kind.content_type();
        return Ok(byte_response(bytes, content_type, headers, admission));
    }
    let RequestedSize::Pixels(size) = size else {
        unreachable!("original handled above")
    };
    // Decoding requires the bounded permit; `original` above does not.
    let Some(admission) = admission else {
        return Err(sqlx::Error::Protocol(
            "cannot decode an image without an admission permit".to_string(),
        ));
    };
    let processor =
        IMAGE_PROCESSOR.get_or_init(|| ImageProcessor::new(IMAGE_PROCESSING_CONCURRENCY));
    processor
        .process(bytes, size, kind.output_format(), false, admission)
        .await
        .map(|(bytes, admission)| {
            byte_response(bytes, kind.content_type(), headers, Some(admission))
        })
        .map_err(|_| {
            sqlx::Error::Protocol("image processing failed within compatibility bounds".to_string())
        })
}

fn byte_response(
    bytes: Vec<u8>,
    content_type: &'static str,
    request_headers: &HeaderMap,
    admission: Option<ImageAdmission>,
) -> Response<Body> {
    let etag = hex_digest(&bytes);
    if not_modified(request_headers, &etag) {
        return not_modified_response(&etag);
    }
    let length = bytes.len();
    let body = match admission {
        Some(admission) => Body::from(Bytes::from_owner(PermittedBytes {
            bytes,
            _admission: admission,
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
    insert_image_headers(response.headers_mut(), &etag);
    response
}

struct PermittedBytes {
    bytes: Vec<u8>,
    _admission: ImageAdmission,
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
    insert_image_headers(response.headers_mut(), etag);
    response
}

fn insert_image_headers(headers: &mut HeaderMap, etag: &str) {
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
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
        assert_eq!(image_size("80"), Some(RequestedSize::Pixels(80)));
        assert_eq!(image_size("large"), Some(RequestedSize::Pixels(1024)));
        assert_eq!(image_size("original"), Some(RequestedSize::Original));
        assert_eq!(image_size("ORIGINAL"), Some(RequestedSize::Original));
        assert_eq!(image_size("0"), None);
        assert_eq!(image_size("99999999999999999"), None);
        assert_eq!(image_size("bogus"), None);
    }

    /// The Admin UI addresses artwork as `/images/album_<id>/original`; that
    /// token used to fall through to the empty-200 path. It must also work with
    /// no admission permit at all, since it never decodes.
    #[tokio::test]
    async fn original_size_serves_source_bytes_without_reencoding() {
        let source = DEFAULT_ALBUM.to_vec();
        let response = processed_response(
            ImageKind::Album,
            source.clone(),
            RequestedSize::Original,
            &HeaderMap::new(),
            None,
        )
        .await
        .expect("original response");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("image/jpeg"))
        );
        let body = axum::body::to_bytes(response.into_body(), MAX_IMAGE_OUTPUT_BYTES)
            .await
            .expect("body");
        assert_eq!(body.as_ref(), source.as_slice());
    }

    /// A catalogue page asks for every cover at once. Reads must queue for a
    /// decode permit instead of being refused on sight — a browse of 24 covers
    /// produced 20 `503`s before this — while the process-wide bound still
    /// holds and the wait itself stays bounded.
    #[tokio::test]
    async fn artwork_reads_queue_for_a_permit_without_exceeding_the_bound() {
        let processor = ImageProcessor::new(4);
        let mut held = Vec::new();
        for _ in 0..4 {
            held.push(
                processor
                    .admit_bounded(Duration::from_millis(50))
                    .await
                    .expect("admit"),
            );
        }

        // No permit free: the wait expires instead of accepting unbounded work.
        let timed_out = processor.admit_bounded(Duration::from_millis(50)).await;
        assert!(
            timed_out.is_err(),
            "a saturated pipeline must not admit past its bound"
        );

        // Freeing one permit lets a queued read proceed within the same bound.
        held.pop();
        let queued = processor.admit_bounded(Duration::from_secs(1)).await;
        assert!(
            queued.is_ok(),
            "a read must be served once capacity returns"
        );
        drop(queued);
        drop(held);
    }

    #[test]
    fn image_processing_is_deterministic_and_honors_requested_dimensions() {
        let first =
            process_image(DEFAULT_USER, 80, ImageFormat::Jpeg, false).expect("process avatar");
        let second =
            process_image(DEFAULT_USER, 80, ImageFormat::Jpeg, false).expect("process avatar");
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
        assert!(process_image(b"not an image", 80, ImageFormat::Jpeg, false).is_err());
        let image =
            image::DynamicImage::ImageRgba8(image::RgbaImage::new(MAX_IMAGE_DIMENSION + 1, 1));
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode bounded bomb header");
        assert!(process_image(&encoded.into_inner(), 80, ImageFormat::Jpeg, false).is_err());
    }

    #[tokio::test]
    async fn encoded_input_byte_limit_is_checked_before_decode() {
        let processor = ImageProcessor::new(1);
        let too_large = vec![0_u8; (MAX_IMAGE_INPUT_BYTES + 1) as usize];
        let permit = processor.try_admit().expect("image admission");
        assert!(
            processor
                .process(too_large, 80, ImageFormat::Jpeg, false, permit)
                .await
                .is_err()
        );
        assert_eq!(processor.permits.available_permits(), 1);
    }

    #[test]
    fn anonymous_image_processing_concurrency_is_explicitly_bounded() {
        let processor = ImageProcessor::new(1);
        let admission = processor.try_admit().expect("first permit");
        let admitted = Bytes::from_owner(PermittedBytes {
            bytes: vec![1, 2, 3],
            _admission: admission,
        });
        let in_flight = admitted.clone();
        assert!(processor.try_admit().is_err());
        drop(admitted);
        assert!(processor.try_admit().is_err());
        drop(in_flight);
        assert_eq!(processor.permits.available_permits(), 1);
    }
}
