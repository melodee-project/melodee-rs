//! Native-v1 playback settings and per-user equalizer presets.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get},
};
use serde::{Deserialize, Serialize, Serializer};
use sqlx::FromRow;
use uuid::Uuid;

use super::{Authenticated, NativeState, dto::PaginationMetadata, error::NativeApiError};

const MAX_PAGE_SIZE: i32 = 100;
const MAX_BANDS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqualizerBand {
    #[serde(serialize_with = "serialize_legacy_number")]
    pub frequency: f64,
    #[serde(serialize_with = "serialize_legacy_number")]
    pub gain: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EqualizerPreset {
    pub id: Uuid,
    pub name: String,
    #[sqlx(json)]
    pub bands: Vec<EqualizerBand>,
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertPresetRequest {
    pub name: String,
    pub bands: Vec<EqualizerBand>,
    pub is_default: bool,
}

#[derive(Debug, Serialize)]
pub struct EqualizerPresetsResponse {
    pub presets: Vec<EqualizerPreset>,
    pub meta: PaginationMetadata,
    /// Compatibility alias (ADR-0187). The frozen response names the collection
    /// `presets`, while clients built on the generic paged envelope read `data`;
    /// this field carries the same rows so both readers work. The frozen keys are
    /// unchanged and remain authoritative.
    pub data: Vec<EqualizerPreset>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PageParams {
    pub page: Option<i32>,
    pub limit: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSettings {
    #[serde(serialize_with = "serialize_legacy_number")]
    pub crossfade_duration: f64,
    pub gapless_playback: bool,
    pub volume_normalization: bool,
    pub replay_gain: String,
    pub audio_quality: String,
    pub equalizer_preset: Option<String>,
    pub last_used_device: Option<String>,
}

fn serialize_legacy_number<S>(value: &f64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if value.fract() == 0.0 && *value >= i64::MIN as f64 && *value <= i64::MAX as f64 {
        serializer.serialize_i64(*value as i64)
    } else {
        serializer.serialize_f64(*value)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlaybackSettings {
    pub crossfade_duration: Option<f64>,
    pub gapless_playback: Option<bool>,
    pub volume_normalization: Option<bool>,
    pub replay_gain: Option<String>,
    pub audio_quality: Option<String>,
    pub equalizer_preset: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
}

pub fn router() -> Router<NativeState> {
    Router::new()
        .route(
            "/api/v1/playback/settings",
            get(get_playback_settings).post(update_playback_settings),
        )
        .route(
            "/api/v1/equalizer/presets",
            get(list_equalizer_presets).post(upsert_equalizer_preset),
        )
        .route(
            "/api/v1/equalizer/presets/{id}",
            delete(delete_equalizer_preset),
        )
}

async fn get_playback_settings(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
) -> Result<Json<PlaybackSettings>, NativeApiError> {
    let settings = sqlx::query_as::<_, PlaybackSettings>(
        "SELECT crossfade_duration, gapless_playback, volume_normalization, replay_gain, \
         audio_quality, equalizer_preset, last_used_device \
         FROM native_playback_settings WHERE user_id = $1",
    )
    .bind(principal.user_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or_else(default_playback_settings);
    Ok(Json(settings))
}

fn default_playback_settings() -> PlaybackSettings {
    PlaybackSettings {
        crossfade_duration: 0.0,
        gapless_playback: true,
        volume_normalization: false,
        replay_gain: "none".to_owned(),
        audio_quality: "high".to_owned(),
        equalizer_preset: None,
        last_used_device: None,
    }
}

async fn update_playback_settings(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<UpdatePlaybackSettings>,
) -> Result<Json<SuccessResponse>, NativeApiError> {
    validate_playback_settings(&request)?;
    let defaults = default_playback_settings();
    sqlx::query(
        "INSERT INTO native_playback_settings (\
             user_id, crossfade_duration, gapless_playback, volume_normalization, replay_gain, \
             audio_quality, equalizer_preset, created_at, updated_at\
         ) VALUES ($1, COALESCE($2, $8), COALESCE($3, $9), COALESCE($4, $10), \
             COALESCE($5, $11), COALESCE($6, $12), $7, now(), now()) \
         ON CONFLICT (user_id) DO UPDATE SET \
             crossfade_duration = COALESCE($2, native_playback_settings.crossfade_duration), \
             gapless_playback = COALESCE($3, native_playback_settings.gapless_playback), \
             volume_normalization = COALESCE($4, native_playback_settings.volume_normalization), \
             replay_gain = COALESCE($5, native_playback_settings.replay_gain), \
             audio_quality = COALESCE($6, native_playback_settings.audio_quality), \
             equalizer_preset = COALESCE($7, native_playback_settings.equalizer_preset), \
             updated_at = now()",
    )
    .bind(principal.user_id)
    .bind(request.crossfade_duration)
    .bind(request.gapless_playback)
    .bind(request.volume_normalization)
    .bind(request.replay_gain)
    .bind(request.audio_quality)
    .bind(request.equalizer_preset)
    .bind(defaults.crossfade_duration)
    .bind(defaults.gapless_playback)
    .bind(defaults.volume_normalization)
    .bind(defaults.replay_gain)
    .bind(defaults.audio_quality)
    .execute(&state.pool)
    .await?;
    Ok(Json(SuccessResponse { success: true }))
}

fn validate_playback_settings(request: &UpdatePlaybackSettings) -> Result<(), NativeApiError> {
    if request
        .crossfade_duration
        .is_some_and(|value| !value.is_finite() || !(0.0..=30.0).contains(&value))
    {
        return Err(NativeApiError::BadRequest);
    }
    if request
        .replay_gain
        .as_deref()
        .is_some_and(|value| !matches!(value, "none" | "track" | "album"))
        || request
            .audio_quality
            .as_deref()
            .is_some_and(|value| !matches!(value, "low" | "medium" | "high" | "lossless"))
        || request
            .equalizer_preset
            .as_ref()
            .is_some_and(|value| value.chars().count() > 500)
    {
        return Err(NativeApiError::BadRequest);
    }
    Ok(())
}

async fn list_equalizer_presets(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Query(params): Query<PageParams>,
) -> Result<Json<EqualizerPresetsResponse>, NativeApiError> {
    let (page, limit, offset) = page(params.page, params.limit)?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM native_equalizer_presets WHERE user_id = $1",
    )
    .bind(principal.user_id)
    .fetch_one(&state.pool)
    .await?;
    let presets = sqlx::query_as::<_, EqualizerPreset>(
        "SELECT public_api_id AS id, name, bands, is_default \
         FROM native_equalizer_presets WHERE user_id = $1 \
         ORDER BY created_at, public_api_id LIMIT $2 OFFSET $3",
    )
    .bind(principal.user_id)
    .bind(i64::from(limit))
    .bind(offset)
    .fetch_all(&state.pool)
    .await?;
    let meta = PaginationMetadata::new(
        total,
        u32::try_from(page).map_err(|_| NativeApiError::BadRequest)?,
        u16::try_from(limit).map_err(|_| NativeApiError::BadRequest)?,
    )
    .map_err(|_| NativeApiError::BadRequest)?;
    Ok(Json(EqualizerPresetsResponse {
        data: presets.clone(),
        presets,
        meta,
    }))
}

async fn upsert_equalizer_preset(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Json(request): Json<UpsertPresetRequest>,
) -> Result<Json<EqualizerPreset>, NativeApiError> {
    validate_preset(&request)?;
    let mut tx = state.pool.begin().await?;
    if request.is_default {
        sqlx::query(
            "UPDATE native_equalizer_presets SET is_default = false, updated_at = now() \
             WHERE user_id = $1 AND is_default",
        )
        .bind(principal.user_id)
        .execute(&mut *tx)
        .await?;
    }
    let id = Uuid::now_v7();
    let preset = sqlx::query_as::<_, EqualizerPreset>(
        "INSERT INTO native_equalizer_presets (\
             id, public_api_id, user_id, name, bands, is_default, created_at, updated_at\
         ) VALUES ($1, $1, $2, $3, $4, $5, now(), now()) \
         ON CONFLICT (user_id, name) DO UPDATE SET \
             bands = EXCLUDED.bands, is_default = EXCLUDED.is_default, updated_at = now() \
         RETURNING public_api_id AS id, name, bands, is_default",
    )
    .bind(id)
    .bind(principal.user_id)
    .bind(request.name.trim())
    .bind(sqlx::types::Json(&request.bands))
    .bind(request.is_default)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(preset))
}

fn validate_preset(request: &UpsertPresetRequest) -> Result<(), NativeApiError> {
    if request.name.trim().is_empty()
        || request.name.chars().count() > 500
        || request.bands.is_empty()
        || request.bands.len() > MAX_BANDS
        || request.bands.iter().any(|band| {
            !band.frequency.is_finite() || band.frequency <= 0.0 || !band.gain.is_finite()
        })
    {
        return Err(NativeApiError::BadRequest);
    }
    Ok(())
}

async fn delete_equalizer_preset(
    State(state): State<NativeState>,
    Authenticated(principal, _): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, NativeApiError> {
    let result = sqlx::query(
        "DELETE FROM native_equalizer_presets WHERE user_id = $1 AND public_api_id = $2",
    )
    .bind(principal.user_id)
    .bind(id)
    .execute(&state.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(NativeApiError::NotFound);
    }
    Ok(StatusCode::OK)
}

fn page(page: Option<i32>, limit: Option<i32>) -> Result<(i32, i32, i64), NativeApiError> {
    let page = page.unwrap_or(1);
    let limit = limit.unwrap_or(20);
    if page < 1 || !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(NativeApiError::BadRequest);
    }
    Ok((page, limit, i64::from(page - 1) * i64::from(limit)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_bounds_untrusted_audio_settings() {
        let invalid = UpdatePlaybackSettings {
            crossfade_duration: Some(f64::INFINITY),
            ..UpdatePlaybackSettings::default()
        };
        assert!(validate_playback_settings(&invalid).is_err());
        assert!(page(Some(0), Some(20)).is_err());
        assert!(page(Some(1), Some(101)).is_err());
    }

    #[test]
    fn preset_validation_rejects_empty_and_non_finite_bands() {
        let invalid = UpsertPresetRequest {
            name: "test".to_owned(),
            bands: vec![EqualizerBand {
                frequency: 60.0,
                gain: f64::NAN,
            }],
            is_default: false,
        };
        assert!(validate_preset(&invalid).is_err());
    }
}
