//! Import driver: claims a run and imports every domain in dependency order
//! (ADR-0148 §D.6). Each importer module owns its batch loop; this driver owns
//! run creation/claiming and the state transitions around the full pass.

use sqlx::PgPool;
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
            runs::record_plan(target, &inventory, maps, &identity, false).await?
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
    super::import_state4::record_excluded_summaries(source, target, run_id).await?;

    if release_lease {
        runs::release_lease(target, run_id, &owner).await?;
        println!("run {run_id}: import batches complete; run `legacy verify --run {run_id}` next");
    }
    Ok(())
}
