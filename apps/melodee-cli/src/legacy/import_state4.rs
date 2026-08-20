//! Remaining user/operational importers (ADR-0148 §D.8 amendment, R3.5):
//! playback settings, equalizer presets, social logins, Last.fm session-key
//! re-sealing, user pins, requests with comments and seen state, per-user
//! device profiles (reference-only: the 1.0 target is global), and summary
//! exclusions for telemetry/session legacy tables.

use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::import_state3::user_target;
use super::map;
use super::runs;
use super::source::SourceDb;

/// Playback settings: one row per user. Out-of-vocabulary legacy values fall
/// back to the target defaults with an informational issue, never a silent
/// rewrite.
pub async fn import_playback_settings(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "playback_settings").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserId", "AudioQuality", "CrossfadeDuration",
                      "EqualizerPreset", "GaplessPlayback", "LastUsedDevice",
                      "ReplayGain", "VolumeNormalization", "CreatedAt", "LastUpdatedAt"
               FROM "UserPlaybackSettings" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("playback settings source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during playback settings import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "playback_settings", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let Some(user_target) = user_target(&mut tx, run_id, user_pk).await? else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "playback_settings",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "playback settings reference an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let mut note: Option<String> = None;
            let quality = match row
                .get::<Option<String>, _>("AudioQuality")
                .map(|q| q.trim().to_lowercase())
                .as_deref()
            {
                Some("low") => "low",
                Some("medium") => "medium",
                Some("lossless") => "lossless",
                Some("high") | None => "high",
                Some(other) => {
                    note = Some(format!(
                        "audio quality {other:?} has no target value; using 'high'"
                    ));
                    "high"
                }
            };
            let replay_gain = match row
                .get::<Option<String>, _>("ReplayGain")
                .map(|v| v.trim().to_lowercase())
                .as_deref()
            {
                Some("track") => "track",
                Some("album") => "album",
                Some("none") | None => "none",
                Some(other) => {
                    note = Some(format!(
                        "replay gain {other:?} has no target value; using 'none'"
                    ));
                    "none"
                }
            };
            if let Some(message) = note {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "playback_settings",
                    Some(source_pk),
                    "info",
                    "playback.value_mapped",
                    &message,
                    None,
                )
                .await?;
            }
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            sqlx::query(
                "INSERT INTO native_playback_settings (
                    user_id, crossfade_duration, gapless_playback, volume_normalization,
                    replay_gain, audio_quality, equalizer_preset, last_used_device,
                    created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
                 ON CONFLICT (user_id) DO NOTHING",
            )
            .bind(user_target)
            .bind(row.get::<f64, _>("CrossfadeDuration").clamp(0.0, 30.0))
            .bind(row.get::<bool, _>("GaplessPlayback"))
            .bind(row.get::<bool, _>("VolumeNormalization"))
            .bind(replay_gain)
            .bind(quality)
            .bind(
                row.get::<Option<String>, _>("EqualizerPreset")
                    .filter(|v| v.len() <= 500),
            )
            .bind(
                row.get::<Option<String>, _>("LastUsedDevice")
                    .filter(|v| v.len() <= 500),
            )
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("playback settings {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "playback_settings",
                source_pk,
                None,
                user_target,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "playback_settings",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// Equalizer presets: legacy `BandsJson` must parse as a JSON array of 1..64
/// bands; anything else is an error issue, not a corrupted insert.
pub async fn import_equalizer_presets(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "equalizer_presets").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "UserId", "Name", "BandsJson", "IsDefault",
                      "CreatedAt", "LastUpdatedAt"
               FROM "UserEqualizerPresets" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("equalizer presets source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during equalizer presets import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "equalizer_presets", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let Some(user_target) = user_target(&mut tx, run_id, user_pk).await? else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "equalizer_presets",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "equalizer preset references an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let bands_raw: Option<String> = row.get("BandsJson");
            let bands = bands_raw
                .as_deref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                .filter(|v| v.is_array());
            let band_len = bands
                .as_ref()
                .and_then(|v| v.as_array())
                .map_or(0, Vec::len);
            let Some(bands) = bands.filter(|_| (1..=64).contains(&band_len)) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "equalizer_presets",
                    Some(source_pk),
                    "error",
                    "equalizer.malformed_bands",
                    "equalizer bands are not a JSON array of 1..64 entries; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let public_id: Uuid = row.get("ApiKey");
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated = map::clamp_updated(created, row.get("LastUpdatedAt"));
            let written = sqlx::query(
                "INSERT INTO native_equalizer_presets (
                    id, public_api_id, user_id, name, bands, is_default, created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(Uuid::now_v7())
            .bind(public_id)
            .bind(user_target)
            .bind(
                row.get::<Option<String>, _>("Name")
                    .filter(|n| !n.trim().is_empty() && n.len() <= 500)
                    .unwrap_or_else(|| "Migrated preset".to_string()),
            )
            .bind(&bands)
            .bind(row.get::<bool, _>("IsDefault"))
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("equalizer preset {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "equalizer_presets",
                    Some(source_pk),
                    "error",
                    "target.public_id_conflict",
                    "public ApiKey already exists in the target outside this run",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            map::insert_mapping(
                &mut tx,
                run_id,
                "equalizer_presets",
                source_pk,
                Some(public_id),
                user_target,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "equalizer_presets",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// Social logins: provider names are normalized to the target lowercase
/// vocabulary; malformed providers/subjects are issues.
pub async fn import_social_logins(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "social_logins").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserId", "Provider", "Subject", "Email",
                      "DisplayName", "HostedDomain", "LastLoginAt", "CreatedAt"
               FROM "UserSocialLogins" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("social logins source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during social logins import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "social_logins", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let Some(user_target) = user_target(&mut tx, run_id, user_pk).await? else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "social_logins",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "social login references an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let provider_raw: Option<String> = row.get("Provider");
            let subject: Option<String> = row.get("Subject");
            let provider = provider_raw.map(|p| p.trim().to_lowercase());
            let valid = provider.as_deref().is_some_and(|p| {
                !p.is_empty()
                    && p.len() <= 50
                    && p.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                    && p.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
            }) && subject
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 500);
            if !valid {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "social_logins",
                    Some(source_pk),
                    "error",
                    "social.malformed_identity",
                    "social login has a malformed provider or subject; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            let created: OffsetDateTime = row.get("CreatedAt");
            let last_login = row
                .get::<Option<OffsetDateTime>, _>("LastLoginAt")
                .filter(|l| *l >= created)
                .unwrap_or(created);
            sqlx::query(
                "INSERT INTO native_user_social_logins (
                    user_id, provider, provider_subject, verified_email, display_name,
                    hosted_domain, linked_at, last_login_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                 ON CONFLICT (provider, provider_subject) DO NOTHING",
            )
            .bind(user_target)
            .bind(provider.as_deref().unwrap_or_default())
            .bind(subject.as_deref().unwrap_or_default())
            .bind(row.get::<Option<String>, _>("Email"))
            .bind(row.get::<Option<String>, _>("DisplayName"))
            .bind(row.get::<Option<String>, _>("HostedDomain"))
            .bind(created)
            .bind(last_login)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("social login {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "social_logins",
                source_pk,
                None,
                user_target,
                last_login,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "social_logins",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// Last.fm session keys are re-sealed into the Rust secret contract
/// (ADR-0148 §D.8): ChaCha20-Poly1305 under the key derived from the target
/// deployment's `security.secretKey`, with the SHA-256 digest for lookup. The
/// legacy protection blob is never copied. Without a configured server
/// secret, affected users are reported for re-linking.
pub async fn import_lastfm_links(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    // Resolve the sealing cipher from the target's configured server secret.
    let secret: Option<String> = sqlx::query_scalar(
        "SELECT value #>> '{}' FROM server_settings WHERE key = 'security.secretKey'",
    )
    .fetch_optional(target)
    .await
    .map_err(|e| format!("cannot read server secret setting: {e}"))?
    .flatten();
    let cipher = secret
        .filter(|s| s.len() >= 32)
        .map(melodee_auth::SecretString::new)
        .map(|s| melodee_auth::derive_sealing_key(melodee_auth::LASTFM_SEALING_KEY_DOMAIN, &s))
        .and_then(|key| melodee_auth::SecretCipher::new(&key).ok());

    let mut key = runs::checkpoint_key(target, run_id, "lastfm_links").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "LastFmSessionKey"
               FROM "Users"
               WHERE "LastFmSessionKey" IS NOT NULL
                 AND btrim("LastFmSessionKey") <> '' AND "Id" > $1
               ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("lastfm source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during lastfm import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "lastfm_links", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let Some(user_target) = user_target(&mut tx, run_id, source_pk).await? else {
                continue;
            };
            let session_key: String = row.get("LastFmSessionKey");
            let Some(cipher) = cipher.as_ref() else {
                runs::record_issue(
                    &mut tx, run_id, "lastfm_links", Some(source_pk), "error",
                    "lastfm.unsealable",
                    "no target security.secretKey configured (>= 32 bytes); user must re-link Last.fm",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let sealed = cipher
                .seal(session_key.as_bytes())
                .map_err(|e| format!("cannot seal session key: {e}"))?;
            let digest = melodee_auth::lastfm_session_key_digest(session_key.as_bytes());
            sqlx::query(
                "INSERT INTO native_lastfm_links (
                    user_id, lastfm_username, session_key_digest, session_key_ciphertext,
                    linked_at
                 ) VALUES ($1, NULL, $2, $3, now())
                 ON CONFLICT (user_id) DO NOTHING",
            )
            .bind(user_target)
            .bind(digest.as_slice())
            .bind(&sealed)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("lastfm link {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "lastfm_links",
                source_pk,
                None,
                user_target,
                OffsetDateTime::now_utc(),
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "lastfm_links",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// User pins (Artist/Album/Song) set the `pinned` flag on the matching user
/// state row, creating a minimal state row when none exists. Playlist and
/// PodcastChannel pins have no 1.0 target flag and are reference-only issues.
pub async fn import_user_pins(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "user_pins").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "UserId", "PinId", "PinType", "CreatedAt"
               FROM "UserPins" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("user pins source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during user pins import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "user_pins", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let pin_pk = i64::from(row.get::<i32, _>("PinId"));
            let pin_type = row.get::<i32, _>("PinType");
            let Some(user_target) = user_target(&mut tx, run_id, user_pk).await? else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "user_pins",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "pin references an unmapped user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let created: OffsetDateTime = row.get("CreatedAt");
            // UserPinType: 1 Artist, 2 Album, 3 Song, 4 Playlist,
            // 5 PodcastChannel (legacy UserPinType).
            let (entity, table, column) = match pin_type {
                1 => ("artist", "native_artist_user_states", "artist_id"),
                2 => ("release", "native_release_user_states", "release_id"),
                3 => ("track", "native_track_user_states", "track_id"),
                other => {
                    runs::record_issue(
                        &mut tx, run_id, "user_pins", Some(source_pk), "info",
                        "pin.reference_only",
                        "playlist/podcast-channel pins have no 1.0 target flag; recorded for review",
                        Some(serde_json::json!({ "pin_type": other, "pin_id": pin_pk })),
                    )
                    .await?;
                    issues += 1;
                    continue;
                }
            };
            let Some(entity_target) = map::catalog_mapping(&mut tx, run_id, entity, pin_pk).await?
            else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "user_pins",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "pin references an unmapped catalog entity; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            // Table/column names come from the hardcoded pin-type match above.
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "INSERT INTO {table} (user_id, {column}, pinned, created_at, updated_at)
                 VALUES ($1, $2, true, $3, $3)
                 ON CONFLICT (user_id, {column}) DO UPDATE SET pinned = true"
            )))
            .bind(user_target)
            .bind(entity_target)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("pin {source_pk} apply failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "user_pins",
                source_pk,
                None,
                entity_target,
                created,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "user_pins",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// Per-user legacy device profiles are reference-only issues: the 1.0 target
/// `device_profiles` table is global metadata (ADR-0134) with no per-user
/// ownership, so importing them would silently invent shared semantics.
pub async fn import_device_profiles_reference_only(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "device_profiles").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "Name", "TargetCodec", "MaxBitrate", "DirectPlay"
               FROM "UserDeviceProfiles" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("device profiles source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during device profiles pass"
            ));
        }
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            runs::record_issue(
                &mut tx, run_id, "device_profiles", Some(source_pk), "info",
                "device_profile.reference_only",
                "per-user legacy device profile has no per-user 1.0 target; recreate in the admin UI if still needed",
                Some(serde_json::json!({
                    "name": row.get::<Option<String>, _>("Name"),
                    "target_codec": row.get::<Option<String>, _>("TargetCodec"),
                    "max_bitrate": row.get::<Option<i32>, _>("MaxBitrate"),
                    "direct_play": row.get::<bool, _>("DirectPlay"),
                })),
            )
            .await?;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "device_profiles",
            last_pk,
            count,
            0,
            0,
            count,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// Requests, their comments, and seen state. The legacy category/status
/// vocabularies are numerically identical to the target constraints
/// (category 1..=5, status 1..=4); NotSet/out-of-range values are issues.
/// Legacy target links are already public UUIDs and carry over verbatim.
pub async fn import_requests(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "requests").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "Category", "Status", "Description",
                      "ArtistName", "AlbumTitle", "SongTitle", "ReleaseYear",
                      "ExternalUrl", "Notes", "CreatedByUserId", "UpdatedByUserId",
                      "LastActivityAt", "LastActivityType", "LastActivityUserId",
                      "TargetArtistApiKey", "TargetAlbumApiKey", "TargetSongApiKey",
                      "CreatedAt", "UpdatedAt"
               FROM "Requests" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("requests source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!("run {run_id}: lost lease during requests import"));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "requests", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let creator_pk = i64::from(row.get::<i32, _>("CreatedByUserId"));
            let Some(creator_target) = user_target(&mut tx, run_id, creator_pk).await? else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "requests",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_user",
                    "request references an unmapped creator; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let category = row.get::<i32, _>("Category");
            let status = row.get::<i32, _>("Status");
            let description: Option<String> = row.get("Description");
            let valid = (1..=5).contains(&category)
                && (1..=4).contains(&status)
                && description
                    .as_deref()
                    .is_some_and(|d| !d.trim().is_empty() && d.len() <= 10000);
            if !valid {
                runs::record_issue(
                    &mut tx, run_id, "requests", Some(source_pk), "error",
                    "request.unmappable",
                    "request has NotSet/out-of-range category/status or an empty description; skipped",
                    Some(serde_json::json!({ "category": category, "status": status })),
                )
                .await?;
                issues += 1;
                continue;
            }
            let activity_type = match row.get::<i32, _>("LastActivityType") {
                2 => "SystemComment",
                3 => "StatusChanged",
                4 => "Edited",
                _ => "UserComment",
            };
            let last_activity_user = row
                .get::<Option<i32>, _>("LastActivityUserId")
                .map(i64::from);
            let last_activity_target = match last_activity_user {
                Some(pk) => user_target(&mut tx, run_id, pk).await?,
                None => None,
            };
            let created: OffsetDateTime = row.get("CreatedAt");
            let updated =
                map::clamp_updated(created, row.get::<Option<OffsetDateTime>, _>("UpdatedAt"));
            let last_activity = row.get::<OffsetDateTime, _>("LastActivityAt").max(created);
            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO native_requests (
                    id, public_api_id, created_by_user_id, category, status,
                    description, artist_name, target_artist_public_api_id,
                    release_title, target_release_public_api_id,
                    track_title, target_track_public_api_id,
                    release_year, external_url, notes,
                    last_activity_at, last_activity_type, last_activity_user_id,
                    created_at, updated_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(row.get::<Uuid, _>("ApiKey"))
            .bind(creator_target)
            .bind(category as i16)
            .bind(status as i16)
            .bind(description.as_deref().unwrap_or_default())
            .bind(
                row.get::<Option<String>, _>("ArtistName")
                    .filter(|v| v.len() <= 500),
            )
            .bind(row.get::<Option<Uuid>, _>("TargetArtistApiKey"))
            .bind(
                row.get::<Option<String>, _>("AlbumTitle")
                    .filter(|v| v.len() <= 500),
            )
            .bind(row.get::<Option<Uuid>, _>("TargetAlbumApiKey"))
            .bind(
                row.get::<Option<String>, _>("SongTitle")
                    .filter(|v| v.len() <= 500),
            )
            .bind(row.get::<Option<Uuid>, _>("TargetSongApiKey"))
            .bind(row.get::<Option<i32>, _>("ReleaseYear"))
            .bind(
                row.get::<Option<String>, _>("ExternalUrl")
                    .filter(|v| v.len() <= 4096),
            )
            .bind(
                row.get::<Option<String>, _>("Notes")
                    .filter(|v| v.len() <= 10000),
            )
            .bind(last_activity)
            .bind(activity_type)
            .bind(last_activity_target)
            .bind(created)
            .bind(updated)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("request {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "requests",
                    Some(source_pk),
                    "error",
                    "target.public_id_conflict",
                    "public ApiKey already exists in the target outside this run",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            map::insert_mapping(
                &mut tx,
                run_id,
                "requests",
                source_pk,
                row.get::<Option<Uuid>, _>("ApiKey"),
                target_id,
                updated,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx, run_id, "requests", last_pk, count, inserted, 0, issues, 0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }

    import_request_comments(source, target, run_id, owner, batch_size).await?;
    import_request_seen(source, target, run_id, owner, batch_size).await?;
    Ok(())
}

async fn import_request_comments(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "request_comments").await?;
    loop {
        // Keyset by Id ascending: parents are always created before replies
        // in the legacy model, so parent mappings exist when replies import.
        let rows = sqlx::query(
            r#"SELECT "Id", "ApiKey", "RequestId", "ParentCommentId", "Body",
                      "CreatedByUserId", "IsSystem", "CreatedAt"
               FROM "RequestComments" WHERE "Id" > $1 ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("request comments source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during request comments import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "request_comments", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let request_pk = i64::from(row.get::<i32, _>("RequestId"));
            let Some(request_target) =
                map::mapped_target(&mut tx, run_id, "requests", request_pk).await?
            else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "request_comments",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "comment references an unmapped request; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let parent_target = match row.get::<Option<i32>, _>("ParentCommentId") {
                Some(parent_pk) => {
                    map::mapped_target(&mut tx, run_id, "request_comments", i64::from(parent_pk))
                        .await?
                }
                None => None,
            };
            let author = match row.get::<Option<i32>, _>("CreatedByUserId") {
                Some(pk) => user_target(&mut tx, run_id, i64::from(pk)).await?,
                None => None,
            };
            let body: Option<String> = row.get("Body");
            let Some(body) = body.filter(|b| !b.trim().is_empty() && b.len() <= 10000) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "request_comments",
                    Some(source_pk),
                    "error",
                    "request.empty_comment",
                    "comment body is empty or over 10000 characters; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let created: OffsetDateTime = row.get("CreatedAt");
            let target_id = Uuid::now_v7();
            let written = sqlx::query(
                "INSERT INTO native_request_comments (
                    id, public_api_id, request_id, parent_comment_id,
                    created_by_user_id, body, is_system, created_at
                 ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                 ON CONFLICT (public_api_id) DO NOTHING",
            )
            .bind(target_id)
            .bind(row.get::<Uuid, _>("ApiKey"))
            .bind(request_target)
            .bind(parent_target)
            .bind(author)
            .bind(&body)
            .bind(row.get::<bool, _>("IsSystem"))
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("request comment {source_pk} insert failed: {e}"))?
            .rows_affected();
            if written == 0 {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "request_comments",
                    Some(source_pk),
                    "error",
                    "target.public_id_conflict",
                    "public ApiKey already exists in the target outside this run",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            }
            map::insert_mapping(
                &mut tx,
                run_id,
                "request_comments",
                source_pk,
                row.get::<Option<Uuid>, _>("ApiKey"),
                target_id,
                created,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "request_comments",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

async fn import_request_seen(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
) -> Result<(), String> {
    let mut key = runs::checkpoint_key(target, run_id, "request_seen").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT ("RequestId"::bigint * 2147483648::bigint + "UserId") AS synth_id,
                      "RequestId", "UserId", "LastSeenAt"
               FROM "RequestUserStates"
               WHERE ("RequestId"::bigint * 2147483648::bigint + "UserId") > $1
               ORDER BY synth_id LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("request seen source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows.last().map_or(key, |r| r.get::<i64, _>("synth_id"));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during request seen import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = row.get::<i64, _>("synth_id");
            if map::mapped_target(&mut tx, run_id, "request_seen", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let request_pk = i64::from(row.get::<i32, _>("RequestId"));
            let user_pk = i64::from(row.get::<i32, _>("UserId"));
            let request_target =
                map::mapped_target(&mut tx, run_id, "requests", request_pk).await?;
            let user_target = user_target(&mut tx, run_id, user_pk).await?;
            let (Some(request_target), Some(user_target)) = (request_target, user_target) else {
                runs::record_issue(
                    &mut tx,
                    run_id,
                    "request_seen",
                    Some(source_pk),
                    "error",
                    "ownership.unmapped_reference",
                    "seen state references an unmapped request or user; skipped",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            sqlx::query(
                "INSERT INTO native_request_seen (request_id, user_id, seen_at)
                 VALUES ($1,$2,$3) ON CONFLICT (request_id, user_id) DO NOTHING",
            )
            .bind(request_target)
            .bind(user_target)
            .bind(row.get::<OffsetDateTime, _>("LastSeenAt"))
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("request seen {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "request_seen",
                source_pk,
                None,
                request_target,
                row.get::<OffsetDateTime, _>("LastSeenAt"),
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "request_seen",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// OpenSubsonic per-user secrets: the legacy `v1:gcm:` protection blob
/// (AES-256-GCM, key = SHA-256 of the legacy `security.secretKey`,
/// nonce‖tag‖ciphertext) is decrypted in memory with the operator-supplied
/// legacy key and stored in the Rust per-user secret contract. The raw
/// protection blob is never copied, the key is never persisted or logged,
/// and any failure is a `credential.secret_reseal_failed` issue — the user
/// re-links before cutover.
pub async fn import_opensubsonic_secrets(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
    batch_size: i64,
    legacy_secret_key: Option<&str>,
) -> Result<(), String> {
    use ring::aead::{AES_256_GCM, LessSafeKey, Nonce, UnboundKey};

    let cipher = legacy_secret_key.and_then(|key| {
        if key.len() < 32 {
            return None;
        }
        let digest = {
            use sha2::Digest;
            sha2::Sha256::digest(key.as_bytes())
        };
        UnboundKey::new(&AES_256_GCM, &digest)
            .ok()
            .map(LessSafeKey::new)
    });

    let mut key = runs::checkpoint_key(target, run_id, "opensubsonic_secrets").await?;
    loop {
        let rows = sqlx::query(
            r#"SELECT "Id", "OpenSubsonicSecretProtected"
               FROM "Users"
               WHERE "OpenSubsonicSecretProtected" IS NOT NULL
                 AND btrim("OpenSubsonicSecretProtected") <> '' AND "Id" > $1
               ORDER BY "Id" LIMIT $2"#,
        )
        .bind(key)
        .bind(batch_size)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("opensubsonic secrets source read failed: {e}"))?;
        if rows.is_empty() {
            break;
        }
        let last_pk = rows
            .last()
            .map_or(key, |r| i64::from(r.get::<i32, _>("Id")));
        let count = i64::try_from(rows.len()).unwrap_or(0);

        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during opensubsonic secrets import"
            ));
        }
        let mut inserted = 0_i64;
        let mut issues = 0_i64;
        for row in &rows {
            let source_pk: i64 = i64::from(row.get::<i32, _>("Id"));
            if map::mapped_target(&mut tx, run_id, "opensubsonic_secrets", source_pk)
                .await?
                .is_some()
            {
                continue;
            }
            let Some(user_target) = user_target(&mut tx, run_id, source_pk).await? else {
                continue;
            };
            let protected: String = row.get("OpenSubsonicSecretProtected");
            let plaintext = (|| -> Option<Vec<u8>> {
                let cipher = cipher.as_ref()?;
                let combined = protected.strip_prefix("v1:gcm:").and_then(|b64| {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.decode(b64).ok()
                })?;
                if combined.len() < 12 + 16 + 1 {
                    return None;
                }
                let nonce =
                    Nonce::try_assume_unique_for_key(combined[..12].try_into().ok()?).ok()?;
                // ring expects ciphertext‖tag; legacy packs nonce‖tag‖ct.
                let mut in_out = combined[28..].to_vec();
                in_out.extend_from_slice(&combined[12..28]);
                cipher
                    .open_in_place(nonce, ring::aead::Aad::empty(), &mut in_out)
                    .ok()?;
                in_out.truncate(in_out.len() - 16);
                Some(in_out)
            })();
            let Some(plaintext) = plaintext else {
                runs::record_issue(
                    &mut tx, run_id, "opensubsonic_secrets", Some(source_pk), "error",
                    "credential.secret_reseal_failed",
                    "OpenSubsonic secret could not be re-sealed (missing/incorrect --legacy-secret-key or malformed blob); user must reset it",
                    None,
                )
                .await?;
                issues += 1;
                continue;
            };
            let secret = match String::from_utf8(plaintext) {
                Ok(s) if !s.trim().is_empty() && s.len() <= 256 => s,
                _ => {
                    runs::record_issue(
                        &mut tx, run_id, "opensubsonic_secrets", Some(source_pk), "error",
                        "credential.secret_reseal_failed",
                        "decrypted OpenSubsonic secret is not valid bounded UTF-8; user must reset it",
                        None,
                    )
                    .await?;
                    issues += 1;
                    continue;
                }
            };
            let now = OffsetDateTime::now_utc();
            sqlx::query(
                "INSERT INTO opensubsonic_user_secrets (user_id, secret, created_at, updated_at)
                 VALUES ($1,$2,$3,$3)
                 ON CONFLICT (user_id) DO NOTHING",
            )
            .bind(user_target)
            .bind(&secret)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("opensubsonic secret {source_pk} insert failed: {e}"))?;
            map::insert_mapping(
                &mut tx,
                run_id,
                "opensubsonic_secrets",
                source_pk,
                None,
                user_target,
                now,
            )
            .await?;
            inserted += 1;
        }
        runs::advance_checkpoint(
            &mut tx,
            run_id,
            "opensubsonic_secrets",
            last_pk,
            count,
            inserted,
            0,
            issues,
            0,
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        key = last_pk;
        if count < batch_size {
            break;
        }
    }
    Ok(())
}

/// Telemetry/session-artifact tables are summarized once per run instead of
/// per row: search history, share activity, job history, scan history,
/// legacy charts (Rust charts are regenerated aggregates), uploaded playlist
/// staging files, refresh/Jellyfin tokens (sessions never migrate — users
/// re-authenticate), library access bindings (libraries have no Rust
/// entity), request participants (derived from comments/seen state), the
/// legacy player device registry, and all retired Party/Jukebox tables
/// (ADR-0133/0140/0141).
pub async fn record_excluded_summaries(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
) -> Result<(), String> {
    const EXCLUDED: &[(&str, &str)] = &[
        ("SearchHistories", "telemetry"),
        ("ShareActivities", "telemetry"),
        ("JobHistories", "telemetry"),
        ("LibraryScanHistories", "telemetry"),
        ("Charts", "rust_regenerated_aggregate"),
        ("ChartItems", "rust_regenerated_aggregate"),
        ("PlaylistUploadedFiles", "staging_artifact"),
        ("PlaylistUploadedFileItems", "staging_artifact"),
        ("RefreshTokens", "session_artifact"),
        ("JellyfinAccessTokens", "session_artifact"),
        ("LibraryAccessControls", "no_rust_library_entity"),
        ("RequestParticipants", "derived_from_comments_and_seen"),
        ("Players", "device_registry_reestablished"),
        ("PartySessions", "retired_adr_0133"),
        ("PartyQueueItems", "retired_adr_0133"),
        ("PartyPlaybackStates", "retired_adr_0133"),
        ("PartySessionEndpoints", "retired_adr_0133"),
        ("PartySessionParticipants", "retired_adr_0133"),
        ("PartyAuditEvents", "retired_adr_0133"),
        ("PlayQues", "retired_adr_0133"),
    ];
    let mut tx = target.begin().await.map_err(|e| e.to_string())?;
    if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
        return Err(format!(
            "run {run_id}: lost lease before excluded-table reconciliation"
        ));
    }
    for (table, reason) in EXCLUDED {
        // Table names come from the hardcoded exclusion allowlist above.
        let rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            r#"SELECT COUNT(*) FROM "{table}""#
        )))
        .fetch_one(source.pool())
        .await
        .map_err(|e| format!("excluded count failed for {table}: {e}"))?;
        if rows == 0 {
            continue;
        }
        let already: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM legacy_migration.issues
              WHERE run_id = $1 AND code = 'excluded.summary'
                AND detail->>'table' = $2)",
        )
        .bind(run_id)
        .bind(table)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| format!("exclusion check failed for {table}: {e}"))?;
        if already {
            continue;
        }
        runs::record_issue(
            &mut tx,
            run_id,
            "excluded",
            None,
            "info",
            "excluded.summary",
            "legacy table excluded from import by the accepted migration contract",
            Some(serde_json::json!({ "table": table, "rows": rows, "reason": reason })),
        )
        .await?;
    }
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}
