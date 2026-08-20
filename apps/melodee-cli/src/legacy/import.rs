//! Import driver: claims a run and imports every domain in dependency order
//! (ADR-0148 §D.6). Each importer module owns its batch loop; this driver owns
//! run creation/claiming and the state transitions around the full pass.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::LibraryPathMap;
use super::runs;
use super::source::SourceDb;

/// Runs the full import for one run: creates/claims it, imports every domain
/// in dependency order, and leaves the run in `importing` for `verify`.
///
/// # Errors
/// Returns an actionable message on fencing loss, source errors, or target
/// constraint violations; unrepresentable rows become issues, not aborts.
#[allow(clippy::too_many_arguments)]
pub async fn import_all(
    source: &SourceDb,
    target: &PgPool,
    run: Option<Uuid>,
    batch_size: i64,
    maps: &[LibraryPathMap],
    release_lease: bool,
    media_validation: Option<&super::media::CanonicalRootValidator>,
    legacy_secret_key: Option<&str>,
) -> Result<(), String> {
    let owner = super::default_owner();
    let run_id = match run {
        Some(id) => id,
        None => {
            let inventory = super::source::collect_inventory(source).await?;
            let identity = source.source_identity().await?;
            let marker = source.snapshot_marker().await?;
            runs::record_plan(target, &inventory, maps, &identity, &marker, false).await?
        }
    };
    if !runs::claim_run(target, run_id, &owner).await? {
        return Err(format!(
            "run {run_id} is held by another live owner or is terminal; \
             wait for lease expiry or start a new run"
        ));
    }
    // State handling is monotonic (ADR-0148 §D.6): a fresh planned run
    // advances to importing; a resumed run keeps its state — a delta-ready
    // run must never regress to importing, or the verified-clean marker the
    // delta command relies on would be lost.
    let state: String = sqlx::query_scalar("SELECT state FROM legacy_migration.runs WHERE id = $1")
        .bind(run_id)
        .fetch_one(target)
        .await
        .map_err(|e| format!("cannot read run {run_id} state: {e}"))?;
    match state.as_str() {
        "planned" => {
            if !runs::transition(target, run_id, &owner, &["planned"], "importing").await? {
                return Err(format!("run {run_id}: lost fencing before import start"));
            }
        }
        "importing" | "delta-ready" => {}
        other => {
            // Handled rejection, not a crash: release so the operator's next
            // command is not blocked for the remaining lease.
            runs::release_lease(target, run_id, &owner).await?;
            return Err(format!(
                "run {run_id} is {other}; import requires planned, importing, or delta-ready"
            ));
        }
    }

    super::import_users::import_users(source, target, run_id, &owner, batch_size).await?;
    super::import_state4::import_opensubsonic_secrets(
        source,
        target,
        run_id,
        &owner,
        batch_size,
        legacy_secret_key,
    )
    .await?;
    super::import_catalog::import_artists(source, target, run_id, &owner, batch_size).await?;
    super::import_catalog::import_releases(source, target, run_id, &owner, batch_size).await?;
    super::import_tracks::import_tracks(
        source,
        target,
        run_id,
        &owner,
        batch_size,
        maps,
        media_validation,
    )
    .await?;
    super::import_contrib::import_contributions(source, target, run_id, &owner, batch_size).await?;
    super::import_state::import_playlists(source, target, run_id, &owner, batch_size).await?;
    super::import_state::import_playlist_items(source, target, run_id, &owner, batch_size).await?;
    super::import_state::import_shares(source, target, run_id, &owner, batch_size).await?;
    super::import_state::import_bookmarks(source, target, run_id, &owner, batch_size).await?;
    super::import_state2::import_user_states(source, target, run_id, &owner, batch_size).await?;
    super::import_state2::import_track_plays(source, target, run_id, &owner, batch_size).await?;
    super::import_state2::import_podcasts(source, target, run_id, &owner, batch_size).await?;
    super::import_state2::import_radio(source, target, run_id, &owner, batch_size).await?;
    super::import_state2::import_settings(source, target, run_id, &owner, batch_size).await?;
    super::import_state3::import_user_groups(source, target, run_id, &owner, batch_size).await?;
    super::import_state3::import_smart_playlists(source, target, run_id, &owner, batch_size)
        .await?;
    super::import_state3::import_podcast_episodes(source, target, run_id, &owner, batch_size)
        .await?;
    super::import_state4::import_playback_settings(source, target, run_id, &owner, batch_size)
        .await?;
    super::import_state4::import_equalizer_presets(source, target, run_id, &owner, batch_size)
        .await?;
    super::import_state4::import_social_logins(source, target, run_id, &owner, batch_size).await?;
    super::import_state4::import_lastfm_links(source, target, run_id, &owner, batch_size).await?;
    super::import_state4::import_user_pins(source, target, run_id, &owner, batch_size).await?;
    super::import_state4::import_requests(source, target, run_id, &owner, batch_size).await?;
    super::import_state4::import_device_profiles_reference_only(
        source, target, run_id, &owner, batch_size,
    )
    .await?;
    super::import_state4::record_excluded_summaries(source, target, run_id, &owner).await?;

    // The migration writes the normalized catalog in set-sized batches, so
    // no ordinary catalog mutation has had a chance to maintain the derived
    // participation and aggregate projections. Rebuild those projections
    // from authoritative rows before verification or server cutover. The
    // statements are additive for an existing target and use conflict-safe
    // upserts, which keeps a resumed migration from deleting unrelated data.
    rebuild_catalog_projections(target, run_id, &owner).await?;

    if release_lease {
        runs::release_lease(target, run_id, &owner).await?;
        println!("run {run_id}: import batches complete; run `legacy verify --run {run_id}` next");
    }
    Ok(())
}

async fn rebuild_catalog_projections(
    target: &PgPool,
    run_id: Uuid,
    owner: &str,
) -> Result<(), String> {
    let rebuilt_at = OffsetDateTime::now_utc();
    let mut tx = target.begin().await.map_err(|e| e.to_string())?;
    if !runs::is_fenced_in(&mut tx, run_id, owner).await? {
        return Err(format!(
            "run {run_id}: lost lease before catalog projection rebuild"
        ));
    }
    sqlx::query(
        "INSERT INTO artist_track_associations (
            artist_id, track_id, association_type, participation_kind,
            source_key, source_artist_id, confidence, is_performance, created_at
         )
         SELECT track_artist.artist_id, track_artist.track_id,
                CASE track_artist.role
                    WHEN 'primary' THEN 'primary_artist'
                    WHEN 'featured' THEN 'featured_artist'
                    WHEN 'guest' THEN 'guest_artist'
                    ELSE 'other'
                END, 'direct', track_artist.id, NULL::uuid, 10000,
                track_artist.role IN ('primary','featured','guest'), track_artist.created_at
           FROM track_artists track_artist
         UNION ALL
         SELECT release_artist.artist_id, track.id,
                CASE release_artist.role
                    WHEN 'primary' THEN 'primary_artist'
                    WHEN 'featured' THEN 'featured_artist'
                    WHEN 'guest' THEN 'guest_artist'
                    ELSE 'other'
                END, 'direct', release_artist.id, NULL::uuid, 10000,
                release_artist.role IN ('primary','featured','guest'), release_artist.created_at
           FROM release_artists release_artist
           JOIN tracks track ON track.release_id = release_artist.release_id
         UNION ALL
         SELECT contribution.artist_id, contribution.track_id,
                contribution.role, 'direct', contribution.id, NULL::uuid, 10000,
                contribution.is_performance, contribution.created_at
           FROM contributions contribution
          WHERE contribution.track_id IS NOT NULL
         UNION ALL
         SELECT contribution.artist_id, track.id,
                contribution.role, 'direct', contribution.id, NULL::uuid, 10000,
                contribution.is_performance, contribution.created_at
           FROM contributions contribution
           JOIN tracks track ON track.release_id = contribution.release_id
          WHERE contribution.track_id IS NULL
         ON CONFLICT DO NOTHING",
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("catalog participation rebuild failed: {e}"))?;

    sqlx::query(
        "INSERT INTO release_catalog_aggregates (
            release_id, track_count, duration_ms, media_bytes, revision, updated_at
         )
         SELECT release.id, count(track.id), COALESCE(sum(track.duration_ms), 0),
                COALESCE((
                    SELECT sum(distinct_asset.byte_size)
                      FROM (
                          SELECT DISTINCT media_asset.id, media_asset.byte_size
                            FROM tracks asset_track
                            JOIN track_media_assets asset_link
                              ON asset_link.track_id = asset_track.id
                            JOIN media_assets media_asset
                              ON media_asset.id = asset_link.media_asset_id
                           WHERE asset_track.release_id = release.id
                      ) distinct_asset
                ), 0), 1, $1
           FROM releases release
           LEFT JOIN tracks track ON track.release_id = release.id
          GROUP BY release.id
         ON CONFLICT (release_id) DO UPDATE
           SET track_count = EXCLUDED.track_count,
               duration_ms = EXCLUDED.duration_ms,
               media_bytes = EXCLUDED.media_bytes,
               revision = release_catalog_aggregates.revision + 1,
               updated_at = EXCLUDED.updated_at",
    )
    .bind(rebuilt_at)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("release aggregate rebuild failed: {e}"))?;

    sqlx::query(
        "INSERT INTO artist_catalog_aggregates (
            artist_id, release_count, billed_track_count,
            performance_track_count, all_credit_track_count, revision, updated_at
         )
         SELECT artist.id,
                count(DISTINCT track.release_id),
                count(DISTINCT association.track_id) FILTER (
                    WHERE association.association_type IN (
                        'primary_artist','featured_artist','guest_artist')),
                count(DISTINCT association.track_id) FILTER (
                    WHERE association.is_performance),
                count(DISTINCT association.track_id), 1, $1
           FROM artists artist
           LEFT JOIN artist_track_associations association
             ON association.artist_id = artist.id
           LEFT JOIN tracks track ON track.id = association.track_id
          GROUP BY artist.id
         ON CONFLICT (artist_id) DO UPDATE
           SET release_count = EXCLUDED.release_count,
               billed_track_count = EXCLUDED.billed_track_count,
               performance_track_count = EXCLUDED.performance_track_count,
               all_credit_track_count = EXCLUDED.all_credit_track_count,
               revision = artist_catalog_aggregates.revision + 1,
               updated_at = EXCLUDED.updated_at",
    )
    .bind(rebuilt_at)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("artist aggregate rebuild failed: {e}"))?;
    tx.commit().await.map_err(|e| e.to_string())
}
