//! Bounded Artist correction, redirect, lifecycle, and observation-only proofs.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::{
    Artist, ArtistAlias, ArtistAliasId, ArtistAliasType, ArtistId, ArtistMatchDecision,
    ArtistMatchDecisionId, ArtistMatchOutcome, ArtistMerge, ArtistMergeId, ArtistNegativeAssertion,
    ArtistNegativeAssertionId, ArtistPublicId, ArtistSplit, ArtistSplitId, ArtistStatus,
    ArtistType, AudioIdentityId, AudioIdentityMerge, AudioIdentityMergeId, Confidence,
    DecisionAuthority, ExternalIdentity, ExternalIdentityId, ExternalIdentityOwner, NewArtist,
    NewArtistMatchDecision, ObservedArtistId, SplitAssignment,
};
use melodee_persistence::{
    CatalogCorrectionDimension, CatalogImportRequest, CatalogStore, CatalogStoreError,
    MAX_CATALOG_CORRECTION_RELEASES, MAX_CATALOG_CORRECTION_TRACKS, WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

fn test_artist(name: &str, status: ArtistStatus, now: OffsetDateTime) -> Artist {
    Artist::new(NewArtist {
        id: ArtistId::new(),
        public_api_id: ArtistPublicId::from_uuid(Uuid::now_v7()),
        canonical_name: name.to_owned(),
        normalized_name: name.to_uppercase().replace(' ', ""),
        sort_name: name.to_owned(),
        artist_type: ArtistType::Unknown,
        disambiguation: None,
        active_dates: None,
        status,
        created_at: now,
    })
    .expect("valid test Artist")
}

async fn import_fixture(
    db: &TestDb,
    ordinal: u32,
    presentation: CatalogFixturePresentation<'_>,
) -> catalog_common::PublishedCatalogFixture {
    let fixture = seed_published_release(db, ordinal, presentation).await;
    let now = OffsetDateTime::now_utc();
    let worker = WorkerId::new();
    assert_eq!(
        CatalogStore
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim correction fixture")
            .len(),
        1
    );
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("correction-regression-v1", "correction-test"),
            now,
        )
        .await
        .expect("import correction fixture");
    fixture
}

#[tokio::test]
async fn correction_release_cap_plus_one_is_typed_and_rolls_back_without_semantic_writes() {
    let db = TestDb::create("catalog_correction_release_cap").await;
    let now = OffsetDateTime::now_utc();
    let store = CatalogStore;
    let source = test_artist("Large Correction Source", ArtistStatus::Verified, now);
    let target = test_artist("Large Correction Target", ArtistStatus::Verified, now);
    store
        .create_artist(&db.pool, &source)
        .await
        .expect("create large correction source");
    store
        .create_artist(&db.pool, &target)
        .await
        .expect("create large correction target");

    let scope_size = MAX_CATALOG_CORRECTION_RELEASES + 1;
    let release_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let observation_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let decision_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    sqlx::query(
        "INSERT INTO releases (
            id,public_api_id,title,normalized_title,sort_title,display_artist,
            created_at,updated_at
         )
         SELECT release_id,release_id,'Bounded Correction','BOUNDEDCORRECTION',
                'Bounded Correction','Large Correction Source',$2,$2
         FROM unnest($1::uuid[]) release_id",
    )
    .bind(&release_ids)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("set-wise seed bounded Releases");
    sqlx::query(
        "INSERT INTO observed_artists (
            id,release_id,track_id,scope,source_ordinal,raw_value,normalized_value,
            display_value,source_pointer,source_payload,created_at
         )
         SELECT fixture.observation_id,fixture.release_id,NULL,'release',1,
                'Large Correction Source','LARGECORRECTIONSOURCE','Large Correction Source',
                'test://large-correction','{}'::jsonb,$3
         FROM unnest($1::uuid[],$2::uuid[])
              fixture(observation_id,release_id)",
    )
    .bind(&observation_ids)
    .bind(&release_ids)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("set-wise seed bounded observations");
    sqlx::query(
        "INSERT INTO artist_match_decisions (
            id,observed_artist_id,artist_id,outcome,authority,confidence,reason,
            is_locked,predecessor_decision_id,payload,created_at
         )
         SELECT fixture.decision_id,fixture.observation_id,$3,'confirmed','admin',10000,
                'large correction fixture',false,NULL,'{}'::jsonb,$4
         FROM unnest($1::uuid[],$2::uuid[])
              fixture(decision_id,observation_id)",
    )
    .bind(&decision_ids)
    .bind(&observation_ids)
    .bind(source.id().into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("set-wise seed current decisions");
    sqlx::query(
        "INSERT INTO artist_match_current (observed_artist_id,decision_id,updated_at)
         SELECT fixture.observation_id,fixture.decision_id,$3
         FROM unnest($1::uuid[],$2::uuid[])
              fixture(observation_id,decision_id)",
    )
    .bind(&observation_ids)
    .bind(&decision_ids)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("set-wise seed current pointers");

    let merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: source.id(),
        source_public_api_id: source.public_api_id(),
        target_artist_id: target.id(),
        actor: "catalog-admin".to_owned(),
        reason: "must use bounded maintenance".to_owned(),
        occurred_at: now + Duration::seconds(1),
    };
    match store
        .merge_artist(&db.pool, &merge, "bounded-correction-v1")
        .await
    {
        Err(CatalogStoreError::CorrectionScopeExceeded { dimension, limit }) => {
            assert_eq!(dimension, CatalogCorrectionDimension::Releases);
            assert_eq!(limit, MAX_CATALOG_CORRECTION_RELEASES);
        }
        other => panic!("expected typed Release correction bound, got {other:?}"),
    }
    let rollback: (String, Option<Uuid>, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT source.status,source.merged_into_artist_id,
                (SELECT count(*) FROM artist_merge_events WHERE id=$2),
                (SELECT count(*) FROM artist_match_decisions
                 WHERE predecessor_decision_id=ANY($3)),
                (SELECT count(*) FROM artist_match_current current
                 JOIN artist_match_decisions decision ON decision.id=current.decision_id
                 WHERE decision.artist_id=$4),
                (SELECT count(*) FROM releases WHERE revision<>1)
         FROM artists source WHERE source.id=$1",
    )
    .bind(source.id().into_uuid())
    .bind(merge.id.into_uuid())
    .bind(&decision_ids)
    .bind(source.id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query bounded correction rollback");
    assert_eq!(
        rollback,
        (
            "verified".to_owned(),
            None,
            0,
            0,
            i64::try_from(scope_size).expect("scope fits i64"),
            0,
        )
    );

    db.drop().await;
}

#[tokio::test]
async fn audio_merge_track_cap_precedes_release_derivation_and_rolls_back() {
    let db = TestDb::create("catalog_audio_merge_track_preflight").await;
    let now = OffsetDateTime::now_utc();
    let source_audio = AudioIdentityId::new();
    let target_audio = AudioIdentityId::new();
    sqlx::query(
        "INSERT INTO audio_identities (id,status,revision,created_at,updated_at)
         VALUES ($1,'provisional',1,$3,$3),($2,'provisional',1,$3,$3)",
    )
    .bind(source_audio.into_uuid())
    .bind(target_audio.into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed AudioIdentity preflight pair");

    let scope_size = MAX_CATALOG_CORRECTION_TRACKS + 1;
    let release_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let track_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let media_asset_ids = (0..scope_size).map(|_| Uuid::now_v7()).collect::<Vec<_>>();
    let media_paths = media_asset_ids
        .iter()
        .map(|id| format!("audio-cap-fixture/{id}.mp3"))
        .collect::<Vec<_>>();
    let mut fixture_tx = db
        .pool
        .begin()
        .await
        .expect("begin set-wise Audio cap fixture");
    sqlx::query(
        "INSERT INTO releases (
            id,public_api_id,title,normalized_title,sort_title,display_artist,
            created_at,updated_at
         )
         SELECT release_id,release_id,'Audio Cap Release','AUDIOCAPRELEASE',
                'Audio Cap Release','Audio Cap Artist',$2,$2
         FROM unnest($1::uuid[]) release_id",
    )
    .bind(&release_ids)
    .bind(now)
    .execute(&mut *fixture_tx)
    .await
    .expect("set-wise seed more than both Release and Track caps");
    sqlx::query(
        "INSERT INTO media_assets (
            id,canonical_relative_path,byte_hash,byte_size,mime_type,created_at
         )
         SELECT fixture.media_asset_id,fixture.path,$3,42,'audio/mpeg',$4
         FROM unnest($1::uuid[],$2::text[]) fixture(media_asset_id,path)",
    )
    .bind(&media_asset_ids)
    .bind(&media_paths)
    .bind("c".repeat(64))
    .bind(now)
    .execute(&mut *fixture_tx)
    .await
    .expect("set-wise seed Audio cap MediaAssets");
    sqlx::query(
        "INSERT INTO tracks (
            id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,
            ordinal,title,normalized_title,sort_title,display_artist,
            duration_ms,revision,created_at,updated_at
         )
         SELECT fixture.track_id,fixture.track_id,fixture.release_id,$4,
                fixture.media_asset_id,1,'Audio Cap Track','AUDIOCAPTRACK',
                'Audio Cap Track','Audio Cap Artist',180000,1,$5,$5
         FROM unnest($1::uuid[],$2::uuid[],$3::uuid[])
              fixture(track_id,release_id,media_asset_id)",
    )
    .bind(&track_ids)
    .bind(&release_ids)
    .bind(&media_asset_ids)
    .bind(source_audio.into_uuid())
    .bind(now)
    .execute(&mut *fixture_tx)
    .await
    .expect("set-wise seed cap+1 Tracks before their deferred links");
    sqlx::query(
        "INSERT INTO track_media_assets (
            track_id,media_asset_id,purpose,ordinal,created_at
         )
         SELECT fixture.track_id,fixture.media_asset_id,'playable',1,$3
         FROM unnest($1::uuid[],$2::uuid[])
              fixture(track_id,media_asset_id)",
    )
    .bind(&track_ids)
    .bind(&media_asset_ids)
    .bind(now)
    .execute(&mut *fixture_tx)
    .await
    .expect("set-wise attach cap+1 playable links");
    fixture_tx
        .commit()
        .await
        .expect("commit complete Audio cap fixture");

    assert!(release_ids.len() > MAX_CATALOG_CORRECTION_RELEASES);
    let merge = AudioIdentityMerge {
        id: AudioIdentityMergeId::new(),
        source_id: source_audio,
        target_id: target_audio,
        reason: "Track bound must win before Release materialization".to_owned(),
        occurred_at: now + Duration::seconds(1),
    };
    match CatalogStore
        .merge_audio_identity(&db.pool, &merge, "catalog-admin", "audio-cap-preflight-v1")
        .await
    {
        Err(CatalogStoreError::CorrectionScopeExceeded { dimension, limit }) => {
            assert_eq!(dimension, CatalogCorrectionDimension::Tracks);
            assert_eq!(limit, MAX_CATALOG_CORRECTION_TRACKS);
        }
        other => panic!("expected typed Track preflight bound, got {other:?}"),
    }
    let rollback: (String, Option<Uuid>, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT source.status,source.merged_into_audio_identity_id,source.revision,
                (SELECT revision FROM audio_identities WHERE id=$2),
                (SELECT count(*) FROM tracks
                 WHERE id=ANY($3) AND (audio_identity_id<>$1 OR revision<>1)),
                (SELECT count(*) FROM audio_identity_merge_events WHERE id=$4),
                (SELECT count(*) FROM releases WHERE id=ANY($5) AND revision<>1)
         FROM audio_identities source WHERE source.id=$1",
    )
    .bind(source_audio.into_uuid())
    .bind(target_audio.into_uuid())
    .bind(&track_ids)
    .bind(merge.id.into_uuid())
    .bind(&release_ids)
    .fetch_one(&db.pool)
    .await
    .expect("query Audio Track-cap rollback");
    assert_eq!(rollback, ("provisional".to_owned(), None, 1, 1, 0, 0, 0));

    db.drop().await;
}

#[tokio::test]
async fn deprecated_targets_and_split_identities_are_rejected() {
    let db = TestDb::create("catalog_deprecated_corrections").await;
    let now = OffsetDateTime::now_utc();
    let store = CatalogStore;
    let deprecated = test_artist("Deprecated Identity", ArtistStatus::Verified, now);
    let verified = test_artist("Verified Identity", ArtistStatus::Verified, now);
    let other = test_artist("Other Verified Identity", ArtistStatus::Verified, now);
    for artist in [&deprecated, &verified, &other] {
        store
            .create_artist(&db.pool, artist)
            .await
            .expect("create lifecycle fixture Artist");
    }
    let deprecated_alias = ArtistAliasId::new();
    store
        .add_artist_alias(
            &db.pool,
            &ArtistAlias {
                id: deprecated_alias,
                artist_id: deprecated.id(),
                value: "Deprecated Alias".to_owned(),
                normalized_value: "DEPRECATEDALIAS".to_owned(),
                alias_type: ArtistAliasType::Search,
                locale: None,
                evidence_source: "deprecated-fixture".to_owned(),
                confidence: Confidence::new(10_000).expect("valid confidence"),
                locked: true,
            },
            now,
        )
        .await
        .expect("seed deprecated split assignment");
    sqlx::query("UPDATE artists SET status='deprecated',updated_at=$2 WHERE id=$1")
        .bind(deprecated.id().into_uuid())
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("deprecate lifecycle fixture after seeding its immutable evidence");
    let verified_alias = ArtistAliasId::new();
    store
        .add_artist_alias(
            &db.pool,
            &ArtistAlias {
                id: verified_alias,
                artist_id: verified.id(),
                value: "Verified Alias".to_owned(),
                normalized_value: "VERIFIEDALIAS".to_owned(),
                alias_type: ArtistAliasType::Search,
                locale: None,
                evidence_source: "deprecated-fixture".to_owned(),
                confidence: Confidence::new(10_000).expect("valid confidence"),
                locked: true,
            },
            now,
        )
        .await
        .expect("seed verified split assignment");

    let deprecated_source_merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: deprecated.id(),
        source_public_api_id: deprecated.public_api_id(),
        target_artist_id: verified.id(),
        actor: "catalog-admin".to_owned(),
        reason: "deprecated source is not an active correction participant".to_owned(),
        occurred_at: now,
    };
    assert!(matches!(
        store
            .merge_artist(
                &db.pool,
                &deprecated_source_merge,
                "deprecated-rejection-v2",
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let deprecated_target_merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: other.id(),
        source_public_api_id: other.public_api_id(),
        target_artist_id: deprecated.id(),
        actor: "catalog-admin".to_owned(),
        reason: "deprecated target is immutable".to_owned(),
        occurred_at: now,
    };
    assert!(matches!(
        store
            .merge_artist(
                &db.pool,
                &deprecated_target_merge,
                "deprecated-rejection-v2",
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));

    let split_destination = test_artist("Valid Split Destination", ArtistStatus::Verified, now);
    let deprecated_source_split = ArtistSplit {
        id: ArtistSplitId::new(),
        source_artist_id: deprecated.id(),
        new_artist_id: split_destination.id(),
        assignments: vec![SplitAssignment::Alias(deprecated_alias)],
        actor: "catalog-admin".to_owned(),
        reason: "deprecated split source is immutable".to_owned(),
        occurred_at: now,
    };
    assert!(matches!(
        store
            .split_artist(
                &db.pool,
                &deprecated_source_split,
                &split_destination,
                "deprecated-rejection-v1",
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let deprecated_destination = test_artist(
        "Deprecated Split Destination",
        ArtistStatus::Deprecated,
        now,
    );
    let deprecated_destination_split = ArtistSplit {
        id: ArtistSplitId::new(),
        source_artist_id: verified.id(),
        new_artist_id: deprecated_destination.id(),
        assignments: vec![SplitAssignment::Alias(verified_alias)],
        actor: "catalog-admin".to_owned(),
        reason: "deprecated split destination is invalid".to_owned(),
        occurred_at: now,
    };
    assert!(matches!(
        store
            .split_artist(
                &db.pool,
                &deprecated_destination_split,
                &deprecated_destination,
                "deprecated-rejection-v1",
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let unchanged: (Uuid, Uuid, i64, i64) = sqlx::query_as(
        "SELECT
            (SELECT artist_id FROM artist_aliases WHERE id=$1),
            (SELECT artist_id FROM artist_aliases WHERE id=$2),
            (SELECT count(*) FROM artist_merge_events),
            (SELECT count(*) FROM artist_split_events)",
    )
    .bind(deprecated_alias.into_uuid())
    .bind(verified_alias.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query lifecycle rejection rollback");
    assert_eq!(
        unchanged,
        (deprecated.id().into_uuid(), verified.id().into_uuid(), 0, 0)
    );

    db.drop().await;
}

#[tokio::test]
async fn two_hop_redirects_resolve_candidates_and_exact_negative_assertions() {
    let db = TestDb::create("catalog_recursive_artist_redirects").await;
    let fixture = import_fixture(
        &db,
        121,
        CatalogFixturePresentation {
            release_title: "Recursive Redirect",
            release_artist: "Observed Redirect",
            track_title: "Redirect Candidate",
            track_artist: "Observed Redirect",
            release_date: "2021-01-01",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let store = CatalogStore;
    let artist_a = test_artist("Redirect A", ArtistStatus::Verified, now);
    let artist_b = test_artist("Redirect B", ArtistStatus::Verified, now);
    let artist_c = test_artist("Redirect C", ArtistStatus::Verified, now);
    for artist in [&artist_a, &artist_b, &artist_c] {
        store
            .create_artist(&db.pool, artist)
            .await
            .expect("create redirect Artist");
    }
    store
        .add_artist_alias(
            &db.pool,
            &ArtistAlias {
                id: ArtistAliasId::new(),
                artist_id: artist_a.id(),
                value: "Recursive Candidate".to_owned(),
                normalized_value: "RECURSIVECANDIDATE".to_owned(),
                alias_type: ArtistAliasType::Search,
                locale: None,
                evidence_source: "redirect-fixture".to_owned(),
                confidence: Confidence::new(9_000).expect("valid confidence"),
                locked: true,
            },
            now,
        )
        .await
        .expect("seed redirected alias");
    store
        .add_artist_external_identity(
            &db.pool,
            &ExternalIdentity {
                id: ExternalIdentityId::new(),
                owner: ExternalIdentityOwner::Artist(artist_a.id()),
                provider: "redirect-provider".to_owned(),
                external_id: "recursive-artist".to_owned(),
                confidence: Confidence::new(9_000).expect("valid confidence"),
                evidence_source: "redirect-fixture".to_owned(),
                verified_at: Some(now),
                locked: true,
                provider_guarantees_uniqueness: true,
            },
            now,
        )
        .await
        .expect("seed redirected provider evidence");
    let observed: Uuid = sqlx::query_scalar(
        "SELECT id FROM observed_artists WHERE release_id=$1 ORDER BY id LIMIT 1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query redirect observation");
    let negative = ArtistNegativeAssertionId::new();
    store
        .record_artist_negative_assertion(
            &db.pool,
            &ArtistNegativeAssertion {
                id: negative,
                observed_artist_id: ObservedArtistId::from_uuid(observed),
                rejected_artist_id: artist_a.id(),
                authority: DecisionAuthority::Administrator {
                    actor: "catalog-admin".to_owned(),
                },
                locked: true,
                reason: "reject the semantic identity through redirects".to_owned(),
                decided_at: now,
            },
        )
        .await
        .expect("seed redirect-aware negative");
    for (source, target, offset) in [(&artist_a, &artist_b, 1), (&artist_b, &artist_c, 2)] {
        store
            .merge_artist(
                &db.pool,
                &ArtistMerge {
                    id: ArtistMergeId::new(),
                    source_artist_id: source.id(),
                    source_public_api_id: source.public_api_id(),
                    target_artist_id: target.id(),
                    actor: "catalog-admin".to_owned(),
                    reason: "build durable two-hop redirect".to_owned(),
                    occurred_at: now + Duration::seconds(offset),
                },
                "recursive-redirect-v1",
            )
            .await
            .expect("merge redirect hop");
    }
    let redirects: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id,merged_into_artist_id FROM artists WHERE id=ANY($1) ORDER BY id")
            .bind([artist_a.id().into_uuid(), artist_b.id().into_uuid()])
            .fetch_all(&db.pool)
            .await
            .expect("query durable redirect chain");
    assert!(redirects.contains(&(artist_a.id().into_uuid(), Some(artist_b.id().into_uuid()))));
    assert!(redirects.contains(&(artist_b.id().into_uuid(), Some(artist_c.id().into_uuid()))));
    assert!(
        store
            .artist_candidate_is_suppressed(
                &db.pool,
                ObservedArtistId::from_uuid(observed),
                artist_c.id(),
            )
            .await
            .expect("negative follows two-hop redirect")
    );
    assert!(
        store
            .find_artist_candidates_by_name(
                &db.pool,
                ObservedArtistId::from_uuid(observed),
                "RECURSIVECANDIDATE",
                20,
            )
            .await
            .expect("negative filters redirected alias candidate")
            .is_empty()
    );
    let provider_candidates = store
        .find_artist_candidates_by_external_id(
            &db.pool,
            ObservedArtistId::from_uuid(observed),
            "redirect-provider",
            "recursive-artist",
            20,
        )
        .await
        .expect("provider evidence resolves full redirect");
    assert_eq!(provider_candidates.len(), 1);
    assert_eq!(provider_candidates[0].artist_id, artist_c.id());
    store
        .revoke_artist_negative_assertion(
            &db.pool,
            negative.into_uuid(),
            &DecisionAuthority::Administrator {
                actor: "catalog-admin".to_owned(),
            },
            "restore redirected candidate",
            now + Duration::seconds(3),
        )
        .await
        .expect("revoke redirect-aware negative");
    let alias_candidates = store
        .find_artist_candidates_by_name(
            &db.pool,
            ObservedArtistId::from_uuid(observed),
            "RECURSIVECANDIDATE",
            20,
        )
        .await
        .expect("revocation restores redirected alias candidate");
    assert_eq!(alias_candidates.len(), 1);
    assert_eq!(alias_candidates[0].artist_id, artist_c.id());

    db.drop().await;
}

#[tokio::test]
async fn observation_only_artist_merge_appends_overlay_and_retargets_current_decision() {
    let db = TestDb::create("catalog_observation_only_merge").await;
    let fixture = import_fixture(
        &db,
        122,
        CatalogFixturePresentation {
            release_title: "Observation-only Merge",
            release_artist: "Unrelated Billing",
            track_title: "Observation-only Track",
            track_artist: "Unrelated Billing",
            release_date: "2022-02-02",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let store = CatalogStore;
    let source = test_artist("Observed-only Source", ArtistStatus::Verified, now);
    let target = test_artist("Observed-only Target", ArtistStatus::Verified, now);
    store
        .create_artist(&db.pool, &source)
        .await
        .expect("create observation-only source");
    store
        .create_artist(&db.pool, &target)
        .await
        .expect("create observation-only target");
    let observed = ObservedArtistId::new();
    sqlx::query(
        "INSERT INTO observed_artists (
            id,release_id,track_id,scope,source_ordinal,raw_value,normalized_value,
            display_value,source_pointer,source_payload,created_at
         ) VALUES ($1,$2,NULL,'release',77,'Observed-only Source','OBSERVEDONLYSOURCE',
                   'Observed-only Source','test://observation-only','{}'::jsonb,$3)",
    )
    .bind(observed.into_uuid())
    .bind(fixture.release_id.into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert observation-only fixture");
    let initial_decision = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: ArtistMatchDecisionId::new(),
            observed_artist_id: observed,
            artist_id: source.id(),
            outcome: ArtistMatchOutcome::Confirmed,
            authority: DecisionAuthority::Administrator {
                actor: "catalog-admin".to_owned(),
            },
            locked: true,
            reason: "seed observation-only current identity".to_owned(),
            decided_at: now,
        },
        &[],
    )
    .expect("valid observation-only decision");
    store
        .record_artist_match_decision(
            &db.pool,
            &initial_decision,
            Confidence::new(10_000).expect("valid confidence"),
            serde_json::json!({"fixture": "observation-only"}),
        )
        .await
        .expect("seed observation-only current decision");
    let direct_source_links: i64 = sqlx::query_scalar(
        "SELECT
            (SELECT count(*) FROM release_artists WHERE artist_id=$1) +
            (SELECT count(*) FROM track_artists WHERE artist_id=$1) +
            (SELECT count(*) FROM contributions WHERE artist_id=$1)",
    )
    .bind(source.id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("prove source exists only through observation current");
    assert_eq!(direct_source_links, 0);
    let before: (i64, Uuid, i64) = sqlx::query_as(
        "SELECT revision,current_overlay_id,
                (SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=releases.id)
         FROM releases WHERE id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query pre-merge observation overlay");
    let merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: source.id(),
        source_public_api_id: source.public_api_id(),
        target_artist_id: target.id(),
        actor: "catalog-admin".to_owned(),
        reason: "merge an identity reachable only from current observation".to_owned(),
        occurred_at: now + Duration::seconds(1),
    };
    store
        .merge_artist(&db.pool, &merge, "observation-only-merge-v1")
        .await
        .expect("observation-only merge succeeds");
    let after: (i64, Uuid, i64, Option<Uuid>, Uuid, Option<Uuid>, i64) = sqlx::query_as(
        "SELECT release.revision,release.current_overlay_id,
                (SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=release.id),
                overlay.predecessor_overlay_id,
                decision.artist_id,decision.predecessor_decision_id,
                (SELECT count(*) FROM artist_merge_events WHERE id=$3)
         FROM releases release
         JOIN catalog_semantic_overlays overlay ON overlay.id=release.current_overlay_id
         JOIN artist_match_current current ON current.observed_artist_id=$2
         JOIN artist_match_decisions decision ON decision.id=current.decision_id
         WHERE release.id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .bind(observed.into_uuid())
    .bind(merge.id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query observation-only merge evidence");
    assert_eq!(after.0, before.0 + 1);
    assert_ne!(after.1, before.1);
    assert_eq!(after.2, before.2 + 1);
    assert_eq!(after.3, Some(before.1));
    assert_eq!(after.4, target.id().into_uuid());
    assert_eq!(after.5, Some(initial_decision.id().into_uuid()));
    assert_eq!(after.6, 1);

    db.drop().await;
}
