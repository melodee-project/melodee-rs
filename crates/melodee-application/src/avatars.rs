//! Bounded avatar decoding shared by API uploads and legacy migration.
use image::{ImageFormat, ImageReader, Limits, imageops::FilterType};
use std::{
    io::Cursor,
    sync::{Arc, OnceLock},
};
use tokio::sync::Semaphore;

const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DIMENSION: u32 = 4096;
static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarError {
    Invalid,
    Busy,
}

pub fn decode_image(input: &[u8]) -> Result<image::DynamicImage, AvatarError> {
    if input.is_empty() || input.len() > MAX_INPUT_BYTES {
        return Err(AvatarError::Invalid);
    }
    let reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| AvatarError::Invalid)?;
    let (width, height) = reader.into_dimensions().map_err(|_| AvatarError::Invalid)?;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(AvatarError::Invalid);
    }
    let mut reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| AvatarError::Invalid)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    reader.decode().map_err(|_| AvatarError::Invalid)
}

/// Blocking primitive; callers own bounded CPU admission.
pub fn normalize_avatar_bytes(input: &[u8], format: ImageFormat) -> Result<Vec<u8>, AvatarError> {
    let image = decode_image(input)?;
    let resized = if image.width() > 1024 || image.height() > 1024 {
        image.resize(1024, 1024, FilterType::Lanczos3)
    } else {
        image
    };
    let mut output = Cursor::new(Vec::new());
    resized
        .write_to(&mut output, format)
        .map_err(|_| AvatarError::Invalid)?;
    let bytes = output.into_inner();
    if bytes.is_empty() || bytes.len() > 1024 * 1024 {
        return Err(AvatarError::Invalid);
    }
    Ok(bytes)
}

/// Applies the same validation as native upload, with PNG for durable migration.
pub async fn normalize_legacy_avatar_image(input: Vec<u8>) -> Result<Vec<u8>, AvatarError> {
    let permit = PERMITS
        .get_or_init(|| Arc::new(Semaphore::new(4)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| AvatarError::Busy)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        normalize_avatar_bytes(&input, ImageFormat::Png)
    })
    .await
    .map_err(|_| AvatarError::Invalid)?
}
