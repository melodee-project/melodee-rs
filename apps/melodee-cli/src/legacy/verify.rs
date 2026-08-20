//! `legacy verify`: source/target parity checks with durable mismatch issues
//! and exact bounded samples (ADR-0148 §D.5, R3.7).

use sqlx::PgPool;
use uuid::Uuid;

use super::runs;
use super::source::SourceDb;

/// One verify comparison: source count, mapped target count, and the sample
/// of unmatched source primary keys.
#[derive(Debug, serde::Serialize)]
pub struct DomainVerify {
    pub domain: String,
    pub source_rows: i64,
    pub mapped_rows: i64,
    pub target_rows: i64,
    pub missing_samples: Vec<i64>,
}

/// Verifies one run against its source. Transitions `importing` → `verifying`
/// → `delta-ready` when the run is clean; mismatches are durable issues and
/// the run stays in `verifying` for operator review.
///
/// # Errors
/// Returns fencing/source/target errors; verification mismatches themselves
/// are reported, not returned as errors.
pub async fn verify_run(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    sample_limit: i64,
    json: bool,
    media_validation: Option<&super::media::CanonicalRootValidator>,
) -> Result<(), String> {
    let owner = super::default_owner();
    if !runs::claim_run(target, run_id, &owner).await? {
        return Err(format!(
            "run {run_id} is held by another live owner or is terminal"
        ));
    }
    if !runs::transition(
        target,
        run_id,
        &owner,
        &["importing", "verifying"],
        "verifying",
    )
    .await?
    {
        // A state-gate rejection is a handled, clean exit — release the
        // claim so the operator's next command is not blocked for the lease
        // duration. Only a process crash relies on lease expiry.
        runs::release_lease(target, run_id, &owner).await?;
        return Err(format!(
            "run {run_id} is not in a verifiable state (expected importing/verifying)"
        ));
    }

    let gate = compute_gate(source, target, run_id, sample_limit, media_validation).await?;
    let results = &gate.results;
    let deep = &gate.deep;
    let total_mismatches = gate.total_mismatches;
    let duplicate_public_ids = gate.duplicate_public_ids;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "run_id": run_id,
                "domains": results,
                "duplicate_public_ids": duplicate_public_ids,
                "deep_checks": deep,
                "total_mismatch_gaps": total_mismatches,
            })
        );
    } else {
        println!("run {run_id} verification:");
        for result in results {
            let flag =
                if result.missing_samples.is_empty() && result.source_rows == result.mapped_rows {
                    "ok"
                } else {
                    "MISMATCH"
                };
            println!(
                "  {:<22} source={:<8} mapped={:<8} target={:<8} {}",
                result.domain, result.source_rows, result.mapped_rows, result.target_rows, flag
            );
            for sample in &result.missing_samples {
                println!("      unmatched source pk: {sample}");
            }
        }
        println!("  duplicate public ids in target: {duplicate_public_ids}");
        for check in deep {
            let flag = if check.mismatches == 0 {
                "ok"
            } else {
                "MISMATCH"
            };
            println!(
                "  {:<28} checked={:<8} mismatches={:<6} {}",
                check.check, check.checked, check.mismatches, flag
            );
            for sample in &check.samples {
                println!("      {sample}");
            }
        }
    }

    if total_mismatches == 0 && duplicate_public_ids == 0 {
        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, &owner).await? {
            return Err(format!("run {run_id}: lost lease during verify completion"));
        }
        tx.commit().await.map_err(|e| e.to_string())?;
        if runs::transition(target, run_id, &owner, &["verifying"], "delta-ready").await? {
            if json {
                eprintln!("run {run_id}: verified clean; state is now delta-ready");
            } else {
                println!("run {run_id}: verified clean; state is now delta-ready");
            }
        }
        runs::release_lease(target, run_id, &owner).await?;
    } else {
        let mut tx = target.begin().await.map_err(|e| e.to_string())?;
        if !runs::is_fenced_in(&mut tx, run_id, &owner).await? {
            return Err(format!(
                "run {run_id}: lost lease during verify issue recording"
            ));
        }
        runs::record_issue(
            &mut tx,
            run_id,
            "verify",
            None,
            "error",
            "verify.mismatch",
            "verification found source/target gaps; see per-domain samples",
            Some(serde_json::json!({
                "total_mismatch_gaps": total_mismatches,
                "duplicate_public_ids": duplicate_public_ids,
            })),
        )
        .await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        let message = format!(
            "run {run_id}: {total_mismatches} mismatch gap(s); state stays verifying \
             until gaps are resolved and verify is re-run"
        );
        if json {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
        runs::release_lease(target, run_id, &owner).await?;
    }
    Ok(())
}

/// Full programmatic verification result (ADR-0149 freeze reuses it).
#[derive(Debug)]
pub struct GateComputation {
    pub results: Vec<DomainVerify>,
    pub deep: Vec<DeepCheck>,
    pub duplicate_public_ids: i64,
    pub total_mismatches: i64,
}

/// Computes every domain and deep-check comparison without printing,
/// recording issues, or transitioning state — the shared gate for `verify`
/// and the final `freeze` verification (ADR-0149 §D.1).
///
/// # Errors
/// Returns source/target database errors.
pub async fn compute_gate(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    sample_limit: i64,
    media_validation: Option<&super::media::CanonicalRootValidator>,
) -> Result<GateComputation, String> {
    // (domain, source table, target table, catalog entity type or "", source key SQL)
    // The source key matches the importer's mapping key; join tables use the
    // synthetic composite from source.rs.
    let domains: &[(&str, &str, &str, &str, &str)] = &[
        ("users", "Users", "native_users", "", "\"Id\"::bigint"),
        ("artists", "Artists", "artists", "artist", "\"Id\"::bigint"),
        (
            "releases",
            "Albums",
            "releases",
            "release",
            "\"Id\"::bigint",
        ),
        ("tracks", "Songs", "tracks", "track", "\"Id\"::bigint"),
        (
            "contributions",
            "Contributors",
            "contributions",
            "",
            "\"Id\"::bigint",
        ),
        (
            "playlists",
            "Playlists",
            "native_playlists",
            "",
            "\"Id\"::bigint",
        ),
        ("shares", "Shares", "native_shares", "", "\"Id\"::bigint"),
        (
            "bookmarks",
            "Bookmarks",
            "opensubsonic_bookmarks",
            "",
            "\"Id\"::bigint",
        ),
        (
            "user_track_states",
            "UserSongs",
            "native_track_user_states",
            "",
            "\"Id\"::bigint",
        ),
        (
            "user_release_states",
            "UserAlbums",
            "native_release_user_states",
            "",
            "\"Id\"::bigint",
        ),
        (
            "user_artist_states",
            "UserArtists",
            "native_artist_user_states",
            "",
            "\"Id\"::bigint",
        ),
        (
            "track_plays",
            "UserSongPlayHistories",
            "native_track_plays",
            "",
            "\"Id\"::bigint",
        ),
        (
            "podcast_channels",
            "PodcastChannels",
            "native_podcast_channels",
            "",
            "\"Id\"::bigint",
        ),
        (
            "podcast_episodes",
            "PodcastEpisodes",
            "native_podcast_episodes",
            "",
            "\"Id\"::bigint",
        ),
        (
            "radio_stations",
            "RadioStations",
            "opensubsonic_radio_stations",
            "",
            "\"Id\"::bigint",
        ),
        (
            "user_groups",
            "UserGroups",
            "native_library_access_groups",
            "",
            "\"Id\"::bigint",
        ),
        (
            "user_group_members",
            "UserGroupMembers",
            "native_user_library_groups",
            "",
            "\"Id\"::bigint",
        ),
        (
            "smart_playlists",
            "SmartPlaylists",
            "native_smart_playlists",
            "",
            "\"Id\"::bigint",
        ),
        (
            "playback_settings",
            "UserPlaybackSettings",
            "native_playback_settings",
            "",
            "\"Id\"::bigint",
        ),
        (
            "equalizer_presets",
            "UserEqualizerPresets",
            "native_equalizer_presets",
            "",
            "\"Id\"::bigint",
        ),
        (
            "social_logins",
            "UserSocialLogins",
            "native_user_social_logins",
            "",
            "\"Id\"::bigint",
        ),
        (
            "requests",
            "Requests",
            "native_requests",
            "",
            "\"Id\"::bigint",
        ),
        (
            "request_comments",
            "RequestComments",
            "native_request_comments",
            "",
            "\"Id\"::bigint",
        ),
        (
            "request_seen",
            "RequestUserStates",
            "native_request_seen",
            "",
            super::source::REQUEST_SEEN_KEY_SQL,
        ),
        (
            "podcast_episode_bookmarks",
            "PodcastEpisodeBookmarks",
            "native_podcast_bookmarks",
            "",
            "\"Id\"::bigint",
        ),
        (
            "podcast_play_history",
            "UserPodcastEpisodePlayHistories",
            "native_podcast_play_history",
            "",
            "\"Id\"::bigint",
        ),
    ];

    let mut results = Vec::with_capacity(domains.len());
    let mut total_mismatches = 0_i64;
    for (domain, source_table, target_table, entity, key_sql) in domains {
        // Source and target are separate databases; counts and missing-key
        // samples are computed by paging both sides and diffing in memory.
        let source_rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            r#"SELECT COUNT(*) FROM "{source_table}""#
        )))
        .fetch_one(source.pool())
        .await
        .map_err(|e| format!("verify count failed for {source_table}: {e}"))?;
        let state_shape = user_state_shape(domain);
        let mapped_rows: i64 = if let Some((entity_type, _, entity_fk)) = state_shape {
            count_user_state_targets(target, run_id, target_table, entity_type, entity_fk).await?
        } else if entity.is_empty() {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM legacy_migration.entity_map
                  WHERE run_id = $1 AND domain = $2",
            )
            .bind(run_id)
            .bind(domain)
            .fetch_one(target)
            .await
            .map_err(|e| format!("verify mapping count failed for {domain}: {e}"))?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM legacy_catalog_id_mappings
                  WHERE migration_run_id = $1 AND entity_type = $2",
            )
            .bind(run_id)
            .bind(entity)
            .fetch_one(target)
            .await
            .map_err(|e| format!("verify mapping count failed for {domain}: {e}"))?
        };
        let target_rows: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM {target_table}"
        )))
        .fetch_one(target)
        .await
        .map_err(|e| format!("verify target count failed for {target_table}: {e}"))?;
        let missing_samples =
            if let Some((entity_type, source_entity_column, entity_fk)) = state_shape {
                missing_user_state_samples(
                    source,
                    target,
                    run_id,
                    domain,
                    source_table,
                    source_entity_column,
                    entity_type,
                    target_table,
                    entity_fk,
                    sample_limit,
                )
                .await?
            } else {
                missing_samples(
                    source,
                    target,
                    run_id,
                    domain,
                    source_table,
                    key_sql,
                    entity,
                    sample_limit,
                )
                .await?
            };
        let gap = source_rows - mapped_rows;
        if gap != 0 || !missing_samples.is_empty() {
            total_mismatches += gap
                .abs()
                .max(i64::try_from(missing_samples.len()).unwrap_or(0));
        }
        results.push(DomainVerify {
            domain: (*domain).to_string(),
            source_rows,
            mapped_rows,
            target_rows,
            missing_samples,
        });
    }

    // Cross-cutting invariants: duplicate public IDs and orphan mappings.
    let duplicate_public_ids: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
           SELECT public_api_id FROM artists GROUP BY public_api_id HAVING COUNT(*) > 1
           UNION ALL
           SELECT public_api_id FROM releases GROUP BY public_api_id HAVING COUNT(*) > 1
           UNION ALL
           SELECT public_api_id FROM tracks GROUP BY public_api_id HAVING COUNT(*) > 1
         ) duplicates",
    )
    .fetch_one(target)
    .await
    .map_err(|e| format!("duplicate public id check failed: {e}"))?;

    // R3.7 deep checks: public-ID equality, relationship totals, playlist
    // ordering, orphan mappings, and adopted-media references.
    let mut deep = Vec::new();
    let public_id = verify_public_id_equality(source, target, run_id, sample_limit).await?;
    total_mismatches += public_id.mismatches;
    deep.push(public_id);
    let relationships = verify_relationship_totals(source, target, run_id).await?;
    total_mismatches += relationships.mismatches;
    deep.push(relationships);
    let ordering = verify_playlist_ordering(source, target, run_id, sample_limit).await?;
    total_mismatches += ordering.mismatches;
    deep.push(ordering);
    let orphans = verify_orphan_mappings(target, run_id, sample_limit).await?;
    total_mismatches += orphans.mismatches;
    deep.push(orphans);
    let media = verify_media_references(target, run_id, media_validation, sample_limit).await?;
    total_mismatches += media.mismatches;
    deep.push(media);

    Ok(GateComputation {
        results,
        deep,
        duplicate_public_ids,
        total_mismatches,
    })
}

/// One R3.7 deep-check outcome: rows examined, mismatches found, and a
/// bounded human-actionable sample list.
#[derive(Debug, serde::Serialize)]
pub struct DeepCheck {
    pub check: String,
    pub checked: i64,
    pub mismatches: i64,
    pub samples: Vec<String>,
}

/// Public-ID equality: every catalog mapping's target `public_api_id` must
/// equal the source row's `ApiKey`. Two-step across the databases in bounded
/// keyset pages (ADR-0148 §D.2).
async fn verify_public_id_equality(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    sample_limit: i64,
) -> Result<DeepCheck, String> {
    let mut checked = 0_i64;
    let mut mismatches = 0_i64;
    let mut samples = Vec::new();
    for (entity, source_table, target_table) in [
        ("artist", "Artists", "artists"),
        ("release", "Albums", "releases"),
        ("track", "Songs", "tracks"),
    ] {
        let mut key = 0_i64;
        loop {
            let mappings: Vec<(i64, Uuid, Uuid)> = sqlx::query_as(
                "SELECT legacy_numeric_id, internal_id, public_api_id
                   FROM legacy_catalog_id_mappings
                  WHERE migration_run_id = $1 AND entity_type = $2
                    AND legacy_numeric_id > $3
                  ORDER BY legacy_numeric_id LIMIT 1000",
            )
            .bind(run_id)
            .bind(entity)
            .bind(key)
            .fetch_all(target)
            .await
            .map_err(|e| format!("public-id verify mapping read failed for {entity}: {e}"))?;
            if mappings.is_empty() {
                break;
            }
            key = mappings.last().map_or(key, |m| m.0);
            checked += i64::try_from(mappings.len()).unwrap_or(0);
            let pks: Vec<i64> = mappings.iter().map(|m| m.0).collect();
            // Source/target tables come from the hardcoded catalog allowlist
            // above; only key arrays are bound parameters.
            let source_ids: Vec<(i32, Uuid)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                r#"SELECT "Id", "ApiKey" FROM "{source_table}" WHERE "Id" = ANY($1)"#
            )))
            .bind(&pks)
            .fetch_all(source.pool())
            .await
            .map_err(|e| format!("public-id verify source read failed for {entity}: {e}"))?;
            let source_by_pk: std::collections::HashMap<i64, Uuid> = source_ids
                .into_iter()
                .map(|(pk, id)| (i64::from(pk), id))
                .collect();
            for (pk, _internal, mapped_public) in &mappings {
                match source_by_pk.get(pk) {
                    Some(source_public) if source_public != mapped_public => {
                        mismatches += 1;
                        if i64::try_from(samples.len()).unwrap_or(0) < sample_limit {
                            samples.push(format!(
                                "{entity} source pk {pk}: source ApiKey {source_public} != target public_api_id {mapped_public}"
                            ));
                        }
                    }
                    _ => {}
                }
            }
            if mappings.len() < 1000 {
                break;
            }
        }
        // The target table name is used only for the aggregate below.
        let _ = target_table;
    }
    Ok(DeepCheck {
        check: "public_id_equality".to_string(),
        checked,
        mismatches,
        samples,
    })
}

/// Relationship totals: every imported release has exactly one imported
/// primary release-artist link, every imported track has exactly one playable
/// media link, and playlist item totals match the mappable source rows.
async fn verify_relationship_totals(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
) -> Result<DeepCheck, String> {
    let mut checked = 0_i64;
    let mut mismatches = 0_i64;
    let mut samples = Vec::new();

    let imported_releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_catalog_id_mappings
          WHERE migration_run_id = $1 AND entity_type = 'release'",
    )
    .bind(run_id)
    .fetch_one(target)
    .await
    .map_err(|e| format!("release mapping count failed: {e}"))?;
    let release_links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM release_artists ra
          JOIN legacy_catalog_id_mappings m
            ON m.internal_id = ra.release_id AND m.entity_type = 'release'
          WHERE m.migration_run_id = $1 AND ra.role = 'primary'",
    )
    .bind(run_id)
    .fetch_one(target)
    .await
    .map_err(|e| format!("release-artist count failed: {e}"))?;
    checked += 1;
    if release_links != imported_releases {
        mismatches += (imported_releases - release_links).abs();
        samples.push(format!(
            "imported releases={imported_releases} but primary release-artist links={release_links}"
        ));
    }

    let imported_tracks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_catalog_id_mappings
          WHERE migration_run_id = $1 AND entity_type = 'track'",
    )
    .bind(run_id)
    .fetch_one(target)
    .await
    .map_err(|e| format!("track mapping count failed: {e}"))?;
    let playable_links: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM track_media_assets tma
          JOIN legacy_catalog_id_mappings m
            ON m.internal_id = tma.track_id AND m.entity_type = 'track'
          WHERE m.migration_run_id = $1 AND tma.purpose = 'playable'",
    )
    .bind(run_id)
    .fetch_one(target)
    .await
    .map_err(|e| format!("playable link count failed: {e}"))?;
    checked += 1;
    if playable_links != imported_tracks {
        mismatches += (imported_tracks - playable_links).abs();
        samples.push(format!(
            "imported tracks={imported_tracks} but playable media links={playable_links}"
        ));
    }

    let source_items: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "PlaylistSong""#)
        .fetch_one(source.pool())
        .await
        .map_err(|e| format!("playlist item source count failed: {e}"))?;
    let mapped_items: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM legacy_migration.entity_map
          WHERE run_id = $1 AND domain = 'playlist_items'",
    )
    .bind(run_id)
    .fetch_one(target)
    .await
    .map_err(|e| format!("playlist item mapping count failed: {e}"))?;
    checked += 1;
    if mapped_items != source_items {
        mismatches += (source_items - mapped_items).abs();
        samples.push(format!(
            "source playlist items={source_items} but mapped target items={mapped_items}"
        ));
    }

    Ok(DeepCheck {
        check: "relationship_totals".to_string(),
        checked,
        mismatches,
        samples,
    })
}

/// Playlist ordering: for every mapped playlist, the source's
/// SongApiKey sequence must equal the target's track public-ID sequence.
async fn verify_playlist_ordering(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    sample_limit: i64,
) -> Result<DeepCheck, String> {
    let mut checked = 0_i64;
    let mut mismatches = 0_i64;
    let mut samples = Vec::new();
    let mut key = 0_i64;
    loop {
        let playlists: Vec<(i64, Uuid)> = sqlx::query_as(
            "SELECT source_pk, target_id FROM legacy_migration.entity_map
              WHERE run_id = $1 AND domain = 'playlists' AND source_pk > $2
              ORDER BY source_pk LIMIT 500",
        )
        .bind(run_id)
        .bind(key)
        .fetch_all(target)
        .await
        .map_err(|e| format!("playlist mapping read failed: {e}"))?;
        if playlists.is_empty() {
            break;
        }
        key = playlists.last().map_or(key, |p| p.0);
        for (source_pk, target_id) in &playlists {
            checked += 1;
            let source_order: Vec<Uuid> = sqlx::query_scalar(
                r#"SELECT "SongApiKey" FROM "PlaylistSong"
                    WHERE "PlaylistId" = $1 ORDER BY "PlaylistOrder""#,
            )
            .bind(*source_pk)
            .fetch_all(source.pool())
            .await
            .map_err(|e| format!("source playlist order read failed: {e}"))?;
            let target_order: Vec<Uuid> = sqlx::query_scalar(
                "SELECT t.public_api_id FROM native_playlist_items i
                  JOIN tracks t ON t.id = i.track_id
                  WHERE i.playlist_id = $1 ORDER BY i.position",
            )
            .bind(*target_id)
            .fetch_all(target)
            .await
            .map_err(|e| format!("target playlist order read failed: {e}"))?;
            if source_order != target_order {
                mismatches += 1;
                if i64::try_from(samples.len()).unwrap_or(0) < sample_limit {
                    samples.push(format!(
                        "playlist source pk {source_pk}: ordering differs \
                         (source {} items, target {} items)",
                        source_order.len(),
                        target_order.len()
                    ));
                }
            }
        }
        if playlists.len() < 500 {
            break;
        }
    }
    Ok(DeepCheck {
        check: "playlist_ordering".to_string(),
        checked,
        mismatches,
        samples,
    })
}

/// Orphan mappings: every mapping row's target must still exist. A mapping
/// without a target is internal corruption — the import asserted both in one
/// transaction — so any occurrence fails verification.
async fn verify_orphan_mappings(
    target: &PgPool,
    run_id: Uuid,
    sample_limit: i64,
) -> Result<DeepCheck, String> {
    // domain -> (target table, target column). Bookmarks map to their catalog
    // entity; composite user-state facts are verified against their
    // (user, catalog entity) key above and have no entity_map target.
    let domain_targets: &[(&str, &str, &str)] = &[
        ("users", "native_users", "id"),
        ("playlists", "native_playlists", "id"),
        ("shares", "native_shares", "id"),
        ("contributions", "contributions", "id"),
        ("bookmarks", "tracks", "id"),
        ("track_plays", "native_track_plays", "id"),
        (
            "podcast_channels",
            "native_podcast_channels",
            "public_api_id",
        ),
        ("radio_stations", "opensubsonic_radio_stations", "id"),
    ];
    let mut checked = 0_i64;
    let mut mismatches = 0_i64;
    let mut samples = Vec::new();
    for (domain, table, column) in domain_targets {
        // Identifiers come from the hardcoded allowlist above.
        let target_exists = if *domain == "contributions" {
            "(EXISTS (SELECT 1 FROM contributions t WHERE t.id = m.target_id)
             OR EXISTS (SELECT 1 FROM observed_artists t WHERE t.id = m.target_id))"
                .to_string()
        } else {
            format!("EXISTS (SELECT 1 FROM {table} t WHERE t.{column} = m.target_id)")
        };
        let orphans: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM legacy_migration.entity_map m
              WHERE m.run_id = $1 AND m.domain = '{domain}'
                AND NOT {target_exists}"
        )))
        .bind(run_id)
        .fetch_one(target)
        .await
        .map_err(|e| format!("orphan mapping check failed for {domain}: {e}"))?;
        let domain_checked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM legacy_migration.entity_map
              WHERE run_id = $1 AND domain = $2",
        )
        .bind(run_id)
        .bind(domain)
        .fetch_one(target)
        .await
        .map_err(|e| format!("mapping count failed for {domain}: {e}"))?;
        checked += domain_checked;
        if orphans > 0 {
            mismatches += orphans;
            if i64::try_from(samples.len()).unwrap_or(0) < sample_limit {
                samples.push(format!(
                    "{domain}: {orphans} mapping(s) without a target row"
                ));
            }
        }
    }
    // Catalog mappings: internal_id must exist in the entity table.
    for (entity, table) in [
        ("artist", "artists"),
        ("release", "releases"),
        ("track", "tracks"),
    ] {
        let orphans: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT COUNT(*) FROM legacy_catalog_id_mappings m
              WHERE m.migration_run_id = $1 AND m.entity_type = '{entity}'
                AND NOT EXISTS (SELECT 1 FROM {table} t WHERE t.id = m.internal_id)"
        )))
        .bind(run_id)
        .fetch_one(target)
        .await
        .map_err(|e| format!("catalog orphan check failed for {entity}: {e}"))?;
        let entity_checked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM legacy_catalog_id_mappings
              WHERE migration_run_id = $1 AND entity_type = $2",
        )
        .bind(run_id)
        .bind(entity)
        .fetch_one(target)
        .await
        .map_err(|e| format!("catalog mapping count failed for {entity}: {e}"))?;
        checked += entity_checked;
        if orphans > 0 {
            mismatches += orphans;
            if i64::try_from(samples.len()).unwrap_or(0) < sample_limit {
                samples.push(format!(
                    "catalog {entity}: {orphans} mapping(s) without a target row"
                ));
            }
        }
    }
    Ok(DeepCheck {
        check: "orphan_mappings".to_string(),
        checked,
        mismatches,
        samples,
    })
}

/// Adopted-media references: count media assets reachable from this run's
/// tracks, and — when the canonical root is mounted — validate existence and
/// expected size read-only in bounded pages.
async fn verify_media_references(
    target: &PgPool,
    run_id: Uuid,
    validator: Option<&super::media::CanonicalRootValidator>,
    sample_limit: i64,
) -> Result<DeepCheck, String> {
    let mut checked = 0_i64;
    let mut mismatches = 0_i64;
    let mut samples = Vec::new();
    let mut key: Option<uuid::Uuid> = None;
    loop {
        let assets: Vec<(uuid::Uuid, String, i64)> = match key {
            Some(after) => sqlx::query_as(
                "SELECT DISTINCT ma.id, ma.canonical_relative_path, ma.byte_size
                   FROM media_assets ma
                   JOIN track_media_assets tma
                     ON tma.media_asset_id = ma.id AND tma.purpose = 'playable'
                   JOIN legacy_catalog_id_mappings m
                     ON m.internal_id = tma.track_id AND m.entity_type = 'track'
                  WHERE m.migration_run_id = $1 AND ma.id > $2
                  ORDER BY ma.id LIMIT 500",
            )
            .bind(run_id)
            .bind(after)
            .fetch_all(target)
            .await
            .map_err(|e| format!("media reference page failed: {e}"))?,
            None => sqlx::query_as(
                "SELECT DISTINCT ma.id, ma.canonical_relative_path, ma.byte_size
                   FROM media_assets ma
                   JOIN track_media_assets tma
                     ON tma.media_asset_id = ma.id AND tma.purpose = 'playable'
                   JOIN legacy_catalog_id_mappings m
                     ON m.internal_id = tma.track_id AND m.entity_type = 'track'
                  WHERE m.migration_run_id = $1
                  ORDER BY ma.id LIMIT 500",
            )
            .bind(run_id)
            .fetch_all(target)
            .await
            .map_err(|e| format!("media reference page failed: {e}"))?,
        };
        if assets.is_empty() {
            break;
        }
        key = assets.last().map(|a| a.0);
        for (_id, path, size) in &assets {
            checked += 1;
            if let Some(validator) = validator {
                let outcome = validator.check(path, Some(*size));
                if outcome != super::media::MediaCheck::Ok {
                    mismatches += 1;
                    if i64::try_from(samples.len()).unwrap_or(0) < sample_limit {
                        samples.push(format!("media {path}: {outcome:?}"));
                    }
                }
            }
        }
        if assets.len() < 500 {
            break;
        }
    }
    Ok(DeepCheck {
        check: if validator.is_some() {
            "media_references_validated".to_string()
        } else {
            "media_references_counted".to_string()
        },
        checked,
        mismatches,
        samples,
    })
}

/// Returns the source/target columns needed to verify a composite user-state
/// fact. User-state rows have no standalone target identity and therefore are
/// intentionally absent from `legacy_migration.entity_map`.
fn user_state_shape(domain: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match domain {
        "user_track_states" => Some(("track", "SongId", "track_id")),
        "user_release_states" => Some(("release", "AlbumId", "release_id")),
        "user_artist_states" => Some(("artist", "ArtistId", "artist_id")),
        _ => None,
    }
}

async fn count_user_state_targets(
    target: &PgPool,
    run_id: Uuid,
    target_table: &str,
    entity_type: &str,
    entity_fk: &str,
) -> Result<i64, String> {
    let query = format!(
        "SELECT COUNT(*) FROM {target_table} state
         WHERE EXISTS (
             SELECT 1 FROM legacy_migration.entity_map users
             WHERE users.run_id = $1 AND users.domain = 'users'
               AND users.target_id = state.user_id
         )
           AND EXISTS (
             SELECT 1 FROM legacy_catalog_id_mappings catalog
             WHERE catalog.migration_run_id = $1
               AND catalog.entity_type = $2
               AND catalog.internal_id = state.{entity_fk}
         )"
    );
    sqlx::query_scalar(sqlx::AssertSqlSafe(query))
        .bind(run_id)
        .bind(entity_type)
        .fetch_one(target)
        .await
        .map_err(|error| format!("user-state target count failed for {entity_type}: {error}"))
}

/// Returns up to `limit` source primary keys whose composite user-state fact
/// has no corresponding target row. The bounded batch query keeps verification
/// set-based while still reporting source keys that reference unmapped users or
/// catalog entities.
#[allow(clippy::too_many_arguments)]
async fn missing_user_state_samples(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    domain: &str,
    source_table: &str,
    source_entity_column: &str,
    entity_type: &str,
    target_table: &str,
    entity_fk: &str,
    limit: i64,
) -> Result<Vec<i64>, String> {
    let mut missing = Vec::new();
    let mut key = 0_i64;
    loop {
        let source_rows: Vec<(i64, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            r#"SELECT "Id"::bigint, "UserId"::bigint,
                      "{source_entity_column}"::bigint
               FROM "{source_table}" WHERE "Id"::bigint > $1
               ORDER BY "Id" LIMIT 1000"#
        )))
        .bind(key)
        .fetch_all(source.pool())
        .await
        .map_err(|error| format!("user-state source read failed for {domain}: {error}"))?;
        if source_rows.is_empty() {
            break;
        }
        key = source_rows.last().map_or(key, |row| row.0);
        let source_pks: Vec<i64> = source_rows.iter().map(|row| row.0).collect();
        let user_pks: Vec<i64> = source_rows.iter().map(|row| row.1).collect();
        let entity_pks: Vec<i64> = source_rows.iter().map(|row| row.2).collect();
        let query = format!(
            "WITH source_rows AS (
                 SELECT * FROM unnest($2::bigint[], $3::bigint[], $4::bigint[])
                   AS input(source_pk, user_pk, entity_pk)
             )
             SELECT input.source_pk
             FROM source_rows input
             LEFT JOIN legacy_migration.entity_map users
               ON users.run_id = $1 AND users.domain = 'users'
              AND users.source_pk = input.user_pk
             LEFT JOIN legacy_catalog_id_mappings catalog
               ON catalog.migration_run_id = $1
              AND catalog.entity_type = $5
              AND catalog.legacy_numeric_id = input.entity_pk
             WHERE users.target_id IS NULL OR catalog.internal_id IS NULL
                OR NOT EXISTS (
                    SELECT 1 FROM {target_table} state
                    WHERE state.user_id = users.target_id
                      AND state.{entity_fk} = catalog.internal_id
                )
             ORDER BY input.source_pk"
        );
        let unmatched: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .bind(run_id)
            .bind(&source_pks)
            .bind(&user_pks)
            .bind(&entity_pks)
            .bind(entity_type)
            .fetch_all(target)
            .await
            .map_err(|error| format!("user-state target read failed for {domain}: {error}"))?;
        for source_pk in unmatched {
            missing.push(source_pk);
            if i64::try_from(missing.len()).unwrap_or(0) >= limit {
                return Ok(missing);
            }
        }
        if source_rows.len() < 1000 {
            break;
        }
    }
    Ok(missing)
}

/// Returns up to `limit` source primary keys with no mapping for the run.
/// Two-step across the databases: source keys from the read-only source,
/// mapped keys from the target, diffed in memory.
#[allow(clippy::too_many_arguments)]
async fn missing_samples(
    source: &SourceDb,
    target: &PgPool,
    run_id: Uuid,
    domain: &str,
    source_table: &str,
    key_sql: &str,
    entity: &str,
    limit: i64,
) -> Result<Vec<i64>, String> {
    let mut missing = Vec::new();
    let mut key = 0_i64;
    // Page source keys in bounded batches until enough gaps are found or the
    // table is exhausted. Table names and key expressions come from the
    // hardcoded verify-domain allowlist; only key/limit values are bound
    // parameters. Key expressions are bigint by construction.
    loop {
        let source_pks: Vec<i64> = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
            r#"SELECT {key_sql} FROM "{source_table}" WHERE {key_sql} > $1 ORDER BY {key_sql} LIMIT 1000"#
        )))
        .bind(key)
        .fetch_all(source.pool())
        .await
        .map_err(|e| format!("verify sample source read failed for {domain}: {e}"))?;
        if source_pks.is_empty() {
            break;
        }
        key = source_pks.last().copied().unwrap_or(key);
        let mapped: Vec<i64> = if entity.is_empty() {
            sqlx::query_scalar(
                "SELECT source_pk FROM legacy_migration.entity_map
                  WHERE run_id = $1 AND domain = $2 AND source_pk = ANY($3)",
            )
            .bind(run_id)
            .bind(domain)
            .bind(&source_pks)
            .fetch_all(target)
            .await
            .map_err(|e| format!("verify sample mapping read failed for {domain}: {e}"))?
        } else {
            sqlx::query_scalar(
                "SELECT legacy_numeric_id FROM legacy_catalog_id_mappings
                  WHERE migration_run_id = $1 AND entity_type = $2
                    AND legacy_numeric_id = ANY($3)",
            )
            .bind(run_id)
            .bind(entity)
            .bind(&source_pks)
            .fetch_all(target)
            .await
            .map_err(|e| format!("verify sample mapping read failed for {domain}: {e}"))?
        };
        for pk in &source_pks {
            if !mapped.contains(pk) {
                missing.push(*pk);
                if i64::try_from(missing.len()).unwrap_or(0) >= limit {
                    return Ok(missing);
                }
            }
        }
        if source_pks.len() < 1000 {
            break;
        }
    }
    Ok(missing)
}
