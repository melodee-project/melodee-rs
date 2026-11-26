//! Deterministic PostgreSQL serialization proofs for first decisions and revocations.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use std::time::Duration as StdDuration;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_domain::{
    ArtistId, ArtistMatchDecision, ArtistMatchDecisionId, ArtistMatchEvidence,
    ArtistMatchEvidenceId, ArtistMatchOutcome, ArtistNegativeAssertion, ArtistNegativeAssertionId,
    AudioIdentityDecisionId, AudioIdentityEvidence, AudioIdentityEvidenceId,
    AudioIdentityEvidenceKind, AudioIdentityId, AudioIdentityMatchDecision,
    AudioIdentityMatchOutcome, Confidence, DecisionAuthority, EvidenceKind, EvidencePolarity,
    NewArtistMatchDecision, NewAudioIdentityMatchDecision, ObservedArtistId,
};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, CatalogStoreError, ProviderNegativeAssertion, WorkerId,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn wait_for_two_database_lock_waiters(db: &TestDb) {
    tokio::time::timeout(StdDuration::from_secs(10), async {
        loop {
            let waiters: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname=current_database() AND pid<>pg_backend_pid()
                   AND wait_event_type='Lock'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("query deterministic lock waiters");
            if waiters >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("both concurrent writers reached a database lock boundary");
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
            .expect("claim concurrency fixture")
            .len(),
        1
    );
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("concurrency-fixture-v1", "catalog-concurrency-test"),
            now,
        )
        .await
        .expect("import concurrency fixture");
    fixture
}

#[tokio::test]
async fn concurrent_first_artist_and_audio_decisions_form_single_predecessor_chains() {
    let db = TestDb::create("catalog_first_decision_serialization").await;
    let first = import_fixture(
        &db,
        111,
        CatalogFixturePresentation {
            release_title: "Concurrent Artist Decision",
            release_artist: "Concurrent Candidate",
            track_title: "Concurrent Audio Source",
            track_artist: "Concurrent Candidate",
            release_date: "2011-01-01",
        },
    )
    .await;
    let second = import_fixture(
        &db,
        112,
        CatalogFixturePresentation {
            release_title: "Concurrent Audio Target",
            release_artist: "Other Candidate",
            track_title: "Concurrent Audio Target",
            track_artist: "Other Candidate",
            release_date: "2012-01-01",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let candidate_artist: Uuid =
        sqlx::query_scalar("SELECT artist_id FROM release_artists WHERE release_id=$1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query Artist decision candidate");
    let observed_artist = ObservedArtistId::new();
    sqlx::query(
        "INSERT INTO observed_artists (
            id,release_id,track_id,scope,source_ordinal,raw_value,normalized_value,
            display_value,source_pointer,source_payload,created_at
         ) VALUES ($1,$2,NULL,'release',99,'Concurrent Candidate','CONCURRENTCANDIDATE',
                   'Concurrent Candidate','test://concurrent-artist','{}'::jsonb,$3)",
    )
    .bind(observed_artist.into_uuid())
    .bind(first.release_id.into_uuid())
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert observation with no first decision");

    let artist_evidence = [
        ArtistMatchEvidence {
            id: ArtistMatchEvidenceId::new(),
            observed_artist_id: observed_artist,
            candidate_artist_id: ArtistId::from_uuid(candidate_artist),
            kind: EvidenceKind::Manual,
            polarity: EvidencePolarity::Supports,
            confidence: Confidence::new(10_000).expect("valid confidence"),
            source: "concurrent-artist-one".to_owned(),
        },
        ArtistMatchEvidence {
            id: ArtistMatchEvidenceId::new(),
            observed_artist_id: observed_artist,
            candidate_artist_id: ArtistId::from_uuid(candidate_artist),
            kind: EvidenceKind::Manual,
            polarity: EvidencePolarity::Supports,
            confidence: Confidence::new(10_000).expect("valid confidence"),
            source: "concurrent-artist-two".to_owned(),
        },
    ];
    for evidence in &artist_evidence {
        CatalogStore
            .record_artist_match_evidence(
                &db.pool,
                evidence,
                serde_json::json!({"fixture": "concurrent-first"}),
                now,
            )
            .await
            .expect("persist concurrent Artist evidence");
    }
    let artist_decisions = artist_evidence
        .iter()
        .enumerate()
        .map(|(index, evidence)| {
            ArtistMatchDecision::new(
                NewArtistMatchDecision {
                    id: ArtistMatchDecisionId::new(),
                    observed_artist_id: observed_artist,
                    artist_id: ArtistId::from_uuid(candidate_artist),
                    outcome: ArtistMatchOutcome::Confirmed,
                    authority: DecisionAuthority::Administrator {
                        actor: format!("concurrent-admin-{index}"),
                    },
                    locked: false,
                    reason: format!("concurrent first Artist decision {index}"),
                    decided_at: now + Duration::seconds(1),
                },
                std::slice::from_ref(evidence),
            )
            .expect("valid concurrent Artist decision")
        })
        .collect::<Vec<_>>();

    sqlx::raw_sql(
        "CREATE FUNCTION test_block_artist_decision_insert() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(74101);
             RETURN NEW;
         END $$;
         CREATE TRIGGER test_block_artist_decision_insert
         BEFORE INSERT ON artist_match_decisions
         FOR EACH ROW EXECUTE FUNCTION test_block_artist_decision_insert();",
    )
    .execute(&db.pool)
    .await
    .expect("install Artist decision barrier");
    let mut artist_blocker = db.pool.begin().await.expect("begin Artist blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(74101)")
        .execute(&mut *artist_blocker)
        .await
        .expect("hold Artist decision barrier");
    let artist_handles = artist_decisions
        .iter()
        .cloned()
        .map(|decision| {
            let pool = db.pool.clone();
            tokio::spawn(async move {
                CatalogStore
                    .record_artist_match_decision(
                        &pool,
                        &decision,
                        Confidence::new(10_000).expect("valid confidence"),
                        serde_json::json!({"fixture": "concurrent-first"}),
                    )
                    .await
            })
        })
        .collect::<Vec<_>>();
    wait_for_two_database_lock_waiters(&db).await;
    artist_blocker
        .commit()
        .await
        .expect("release Artist decision barrier");
    for handle in artist_handles {
        handle
            .await
            .expect("Artist decision task joins")
            .expect("concurrent Artist decision succeeds");
    }
    let artist_rows: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id,predecessor_decision_id FROM artist_match_decisions
         WHERE id=ANY($1) ORDER BY id",
    )
    .bind(
        artist_decisions
            .iter()
            .map(|decision| decision.id().into_uuid())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&db.pool)
    .await
    .expect("query Artist decision chain");
    assert_eq!(artist_rows.len(), 2);
    let artist_root = artist_rows
        .iter()
        .find(|row| row.1.is_none())
        .expect("exactly one Artist decision root");
    let artist_child = artist_rows
        .iter()
        .find(|row| row.1.is_some())
        .expect("exactly one Artist decision successor");
    assert_eq!(artist_child.1, Some(artist_root.0));
    let artist_current: Uuid = sqlx::query_scalar(
        "SELECT decision_id FROM artist_match_current WHERE observed_artist_id=$1",
    )
    .bind(observed_artist.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query Artist current decision");
    assert_eq!(artist_current, artist_child.0);

    let source_audio: Uuid = sqlx::query_scalar(
        "SELECT track.audio_identity_id FROM tracks track WHERE track.release_id=$1",
    )
    .bind(first.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query source AudioIdentity");
    let candidate_audio: Uuid = sqlx::query_scalar(
        "SELECT track.audio_identity_id FROM tracks track WHERE track.release_id=$1",
    )
    .bind(second.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query candidate AudioIdentity");
    let audio_evidence = [
        AudioIdentityEvidence {
            id: AudioIdentityEvidenceId::new(),
            source_audio_identity_id: AudioIdentityId::from_uuid(source_audio),
            candidate_audio_identity_id: AudioIdentityId::from_uuid(candidate_audio),
            kind: AudioIdentityEvidenceKind::AcousticFingerprint,
            polarity: EvidencePolarity::Supports,
            confidence: Confidence::new(9_000).expect("valid confidence"),
            source: "concurrent-audio-one".to_owned(),
        },
        AudioIdentityEvidence {
            id: AudioIdentityEvidenceId::new(),
            source_audio_identity_id: AudioIdentityId::from_uuid(source_audio),
            candidate_audio_identity_id: AudioIdentityId::from_uuid(candidate_audio),
            kind: AudioIdentityEvidenceKind::Duration,
            polarity: EvidencePolarity::Supports,
            confidence: Confidence::new(8_000).expect("valid confidence"),
            source: "concurrent-audio-two".to_owned(),
        },
    ];
    for evidence in &audio_evidence {
        CatalogStore
            .record_audio_identity_evidence(
                &db.pool,
                evidence,
                serde_json::json!({"fixture": "concurrent-first"}),
                now,
            )
            .await
            .expect("persist concurrent Audio evidence");
    }
    let audio_decisions = audio_evidence
        .iter()
        .enumerate()
        .map(|(index, evidence)| {
            AudioIdentityMatchDecision::new(
                NewAudioIdentityMatchDecision {
                    id: AudioIdentityDecisionId::new(),
                    source_audio_identity_id: AudioIdentityId::from_uuid(source_audio),
                    candidate_audio_identity_id: AudioIdentityId::from_uuid(candidate_audio),
                    outcome: AudioIdentityMatchOutcome::Proposed,
                    authority: DecisionAuthority::Automated {
                        resolver_version: format!("concurrent-audio-{index}"),
                    },
                    locked: false,
                    reason: format!("concurrent first Audio decision {index}"),
                    decided_at: now + Duration::seconds(2),
                },
                std::slice::from_ref(evidence),
            )
            .expect("valid concurrent Audio decision")
        })
        .collect::<Vec<_>>();
    sqlx::raw_sql(
        "CREATE FUNCTION test_block_audio_decision_insert() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(74102);
             RETURN NEW;
         END $$;
         CREATE TRIGGER test_block_audio_decision_insert
         BEFORE INSERT ON audio_identity_match_decisions
         FOR EACH ROW EXECUTE FUNCTION test_block_audio_decision_insert();",
    )
    .execute(&db.pool)
    .await
    .expect("install Audio decision barrier");
    let mut audio_blocker = db.pool.begin().await.expect("begin Audio blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(74102)")
        .execute(&mut *audio_blocker)
        .await
        .expect("hold Audio decision barrier");
    let audio_handles = audio_decisions
        .iter()
        .cloned()
        .map(|decision| {
            let pool = db.pool.clone();
            tokio::spawn(async move {
                CatalogStore
                    .record_audio_identity_match_decision(
                        &pool,
                        &decision,
                        serde_json::json!({"fixture": "concurrent-first"}),
                    )
                    .await
            })
        })
        .collect::<Vec<_>>();
    wait_for_two_database_lock_waiters(&db).await;
    audio_blocker
        .commit()
        .await
        .expect("release Audio decision barrier");
    for handle in audio_handles {
        handle
            .await
            .expect("Audio decision task joins")
            .expect("concurrent Audio decision succeeds");
    }
    let audio_rows: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT id,predecessor_decision_id FROM audio_identity_match_decisions
         WHERE id=ANY($1) ORDER BY id",
    )
    .bind(
        audio_decisions
            .iter()
            .map(|decision| decision.id().into_uuid())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&db.pool)
    .await
    .expect("query Audio decision chain");
    assert_eq!(audio_rows.len(), 2);
    let audio_root = audio_rows
        .iter()
        .find(|row| row.1.is_none())
        .expect("exactly one Audio decision root");
    let audio_child = audio_rows
        .iter()
        .find(|row| row.1.is_some())
        .expect("exactly one Audio decision successor");
    assert_eq!(audio_child.1, Some(audio_root.0));
    let audio_current: Uuid = sqlx::query_scalar(
        "SELECT decision_id FROM audio_identity_match_current
         WHERE source_audio_identity_id=$1 AND candidate_audio_identity_id=$2",
    )
    .bind(source_audio)
    .bind(candidate_audio)
    .fetch_one(&db.pool)
    .await
    .expect("query Audio current decision");
    assert_eq!(audio_current, audio_child.0);

    db.drop().await;
}

#[tokio::test]
async fn concurrent_negative_revocation_has_exactly_one_durable_successor() {
    let db = TestDb::create("catalog_negative_revocation_serialization").await;
    let fixture = import_fixture(
        &db,
        113,
        CatalogFixturePresentation {
            release_title: "Concurrent Negative",
            release_artist: "Observed Candidate",
            track_title: "Concurrent Reversal",
            track_artist: "Observed Candidate",
            release_date: "2013-01-01",
        },
    )
    .await;
    let now = OffsetDateTime::now_utc();
    let (observed, candidate): (Uuid, Uuid) = sqlx::query_as(
        "SELECT observed.id,billing.artist_id
         FROM observed_artists observed
         JOIN release_artists billing ON billing.observed_artist_id=observed.id
         WHERE observed.release_id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query negative assertion scope");
    let predecessor = ArtistNegativeAssertionId::new();
    CatalogStore
        .record_artist_negative_assertion(
            &db.pool,
            &ArtistNegativeAssertion {
                id: predecessor,
                observed_artist_id: ObservedArtistId::from_uuid(observed),
                rejected_artist_id: ArtistId::from_uuid(candidate),
                authority: DecisionAuthority::Administrator {
                    actor: "catalog-admin".to_owned(),
                },
                locked: true,
                reason: "concurrent reversal fixture".to_owned(),
                decided_at: now,
            },
        )
        .await
        .expect("persist predecessor negative");
    assert!(matches!(
        CatalogStore
            .record_artist_negative_assertion(
                &db.pool,
                &ArtistNegativeAssertion {
                    id: ArtistNegativeAssertionId::new(),
                    observed_artist_id: ObservedArtistId::from_uuid(observed),
                    rejected_artist_id: ArtistId::from_uuid(candidate),
                    authority: DecisionAuthority::Administrator {
                        actor: "other-admin".to_owned(),
                    },
                    locked: true,
                    reason: "duplicate exact local root must fail".to_owned(),
                    decided_at: now + Duration::seconds(1),
                },
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    assert!(
        CatalogStore
            .artist_candidate_is_suppressed(
                &db.pool,
                ObservedArtistId::from_uuid(observed),
                ArtistId::from_uuid(candidate),
            )
            .await
            .expect("sole local root remains authoritatively active")
    );
    let provider_predecessor = Uuid::now_v7();
    CatalogStore
        .record_provider_negative_assertion(
            &db.pool,
            &ProviderNegativeAssertion {
                id: provider_predecessor,
                observed_artist_id: ObservedArtistId::from_uuid(observed),
                provider: "duplicate-provider".to_owned(),
                external_id: "candidate-key".to_owned(),
                authority: DecisionAuthority::Administrator {
                    actor: "catalog-admin".to_owned(),
                },
                locked: true,
                reason: "sole provider root".to_owned(),
                decided_at: now,
            },
        )
        .await
        .expect("persist provider predecessor negative");
    assert!(matches!(
        CatalogStore
            .record_provider_negative_assertion(
                &db.pool,
                &ProviderNegativeAssertion {
                    id: Uuid::now_v7(),
                    observed_artist_id: ObservedArtistId::from_uuid(observed),
                    provider: "duplicate-provider".to_owned(),
                    external_id: "candidate-key".to_owned(),
                    authority: DecisionAuthority::Administrator {
                        actor: "other-admin".to_owned(),
                    },
                    locked: true,
                    reason: "duplicate exact provider root must fail".to_owned(),
                    decided_at: now + Duration::seconds(1),
                },
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    assert!(
        CatalogStore
            .provider_candidate_is_suppressed(
                &db.pool,
                ObservedArtistId::from_uuid(observed),
                "duplicate-provider",
                "candidate-key",
            )
            .await
            .expect("sole provider root remains authoritatively active")
    );

    sqlx::raw_sql(
        "CREATE FUNCTION test_block_negative_revoke_insert() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             IF NEW.assertion_kind = 'revoke' THEN
                 PERFORM pg_advisory_xact_lock(74103);
             END IF;
             RETURN NEW;
         END $$;
         CREATE TRIGGER test_block_negative_revoke_insert
         BEFORE INSERT ON artist_negative_assertions
         FOR EACH ROW EXECUTE FUNCTION test_block_negative_revoke_insert();",
    )
    .execute(&db.pool)
    .await
    .expect("install negative-revocation barrier");
    let mut blocker = db.pool.begin().await.expect("begin revocation blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(74103)")
        .execute(&mut *blocker)
        .await
        .expect("hold revocation barrier");
    let handles = ["first concurrent reversal", "second concurrent reversal"]
        .into_iter()
        .map(|reason| {
            let pool = db.pool.clone();
            tokio::spawn(async move {
                CatalogStore
                    .revoke_artist_negative_assertion(
                        &pool,
                        predecessor.into_uuid(),
                        &DecisionAuthority::Administrator {
                            actor: "catalog-admin".to_owned(),
                        },
                        reason,
                        now + Duration::seconds(1),
                    )
                    .await
            })
        })
        .collect::<Vec<_>>();
    wait_for_two_database_lock_waiters(&db).await;
    blocker.commit().await.expect("release revocation barrier");
    let mut successes = 0;
    let mut conflicts = 0;
    for handle in handles {
        match handle.await.expect("revocation task joins") {
            Ok(_) => successes += 1,
            Err(CatalogStoreError::MutationConflict) => conflicts += 1,
            Err(other) => panic!("unexpected concurrent revocation error: {other}"),
        }
    }
    assert_eq!((successes, conflicts), (1, 1));
    let successor_rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT id,predecessor_assertion_id,assertion_kind
         FROM artist_negative_assertions WHERE predecessor_assertion_id=$1",
    )
    .bind(predecessor.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("query unique negative successor");
    assert_eq!(successor_rows.len(), 1);
    assert_eq!(successor_rows[0].1, predecessor.into_uuid());
    assert_eq!(successor_rows[0].2, "revoke");
    assert!(
        !CatalogStore
            .artist_candidate_is_suppressed(
                &db.pool,
                ObservedArtistId::from_uuid(observed),
                ArtistId::from_uuid(candidate),
            )
            .await
            .expect("single reversal supersedes negative")
    );
    CatalogStore
        .revoke_artist_negative_assertion(
            &db.pool,
            provider_predecessor,
            &DecisionAuthority::Administrator {
                actor: "catalog-admin".to_owned(),
            },
            "revoke the sole provider root",
            now + Duration::seconds(2),
        )
        .await
        .expect("revoke provider predecessor");
    assert!(
        !CatalogStore
            .provider_candidate_is_suppressed(
                &db.pool,
                ObservedArtistId::from_uuid(observed),
                "duplicate-provider",
                "candidate-key",
            )
            .await
            .expect("sole provider revocation deterministically unsuppresses key")
    );
    let root_and_successor_counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
            count(*) FILTER (WHERE candidate_artist_id=$2 AND assertion_kind='do_not_match'),
            count(*) FILTER (
                WHERE candidate_provider='duplicate-provider'
                  AND candidate_external_id='candidate-key' AND assertion_kind='do_not_match'),
            count(*) FILTER (WHERE predecessor_assertion_id=$3),
            count(*) FILTER (WHERE predecessor_assertion_id=$4)
         FROM artist_negative_assertions WHERE observed_artist_id=$1",
    )
    .bind(observed)
    .bind(candidate)
    .bind(predecessor.into_uuid())
    .bind(provider_predecessor)
    .fetch_one(&db.pool)
    .await
    .expect("query sole roots and successors");
    assert_eq!(root_and_successor_counts, (1, 1, 1, 1));

    db.drop().await;
}

#[tokio::test]
async fn concurrent_direct_redirect_cycles_serialize_to_one_acyclic_terminal() {
    let db = TestDb::create("catalog_redirect_cycle_serialization").await;
    let now = OffsetDateTime::now_utc();
    let artist_a = Uuid::now_v7();
    let artist_b = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO artists (
            id,public_api_id,canonical_name,normalized_name,sort_name,
            artist_type,status,created_at,updated_at
         ) VALUES
            ($1,$1,'Cycle Artist A','CYCLEARTISTA','Cycle Artist A',
             'unknown','verified',$3,$3),
            ($2,$2,'Cycle Artist B','CYCLEARTISTB','Cycle Artist B',
             'unknown','verified',$3,$3)",
    )
    .bind(artist_a)
    .bind(artist_b)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed Artist redirect-cycle pair");
    sqlx::raw_sql(
        "CREATE FUNCTION zzz_test_block_artist_redirect() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(74104);
             RETURN NEW;
         END $$;
         CREATE TRIGGER zzz_test_block_artist_redirect
         BEFORE UPDATE OF merged_into_artist_id ON artists
         FOR EACH ROW EXECUTE FUNCTION zzz_test_block_artist_redirect();",
    )
    .execute(&db.pool)
    .await
    .expect("install post-graph-lock Artist barrier");
    let mut artist_blocker = db.pool.begin().await.expect("begin Artist cycle blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(74104)")
        .execute(&mut *artist_blocker)
        .await
        .expect("hold Artist cycle barrier");
    let artist_handles = [(artist_a, artist_b), (artist_b, artist_a)]
        .into_iter()
        .map(|(source, target)| {
            let pool = db.pool.clone();
            tokio::spawn(async move {
                sqlx::query(
                    "UPDATE artists
                     SET status='merged',merged_into_artist_id=$2,
                         revision=revision+1,updated_at=$3
                     WHERE id=$1",
                )
                .bind(source)
                .bind(target)
                .bind(now + Duration::seconds(1))
                .execute(&pool)
                .await
            })
        })
        .collect::<Vec<_>>();
    wait_for_two_database_lock_waiters(&db).await;
    artist_blocker
        .commit()
        .await
        .expect("release Artist cycle barrier");
    let mut artist_commits = 0;
    let mut artist_rejections = 0;
    for handle in artist_handles {
        match handle.await.expect("Artist cycle task joins") {
            Ok(_) => artist_commits += 1,
            Err(_) => artist_rejections += 1,
        }
    }
    assert_eq!((artist_commits, artist_rejections), (1, 1));
    let artist_terminal: (Uuid, Uuid, i64) = sqlx::query_as(
        "SELECT resolve_artist_redirect($1),resolve_artist_redirect($2),
                (SELECT count(*) FROM artists WHERE id=ANY($3) AND status='merged')",
    )
    .bind(artist_a)
    .bind(artist_b)
    .bind([artist_a, artist_b])
    .fetch_one(&db.pool)
    .await
    .expect("Artist resolver remains total and acyclic");
    assert_eq!(artist_terminal.0, artist_terminal.1);
    assert_eq!(artist_terminal.2, 1);

    let audio_a = Uuid::now_v7();
    let audio_b = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO audio_identities (id,status,created_at,updated_at)
         VALUES ($1,'provisional',$3,$3),($2,'provisional',$3,$3)",
    )
    .bind(audio_a)
    .bind(audio_b)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("seed AudioIdentity redirect-cycle pair");
    sqlx::raw_sql(
        "CREATE FUNCTION zzz_test_block_audio_redirect() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             PERFORM pg_advisory_xact_lock(74105);
             RETURN NEW;
         END $$;
         CREATE TRIGGER zzz_test_block_audio_redirect
         BEFORE UPDATE OF merged_into_audio_identity_id ON audio_identities
         FOR EACH ROW EXECUTE FUNCTION zzz_test_block_audio_redirect();",
    )
    .execute(&db.pool)
    .await
    .expect("install post-graph-lock Audio barrier");
    let mut audio_blocker = db.pool.begin().await.expect("begin Audio cycle blocker");
    sqlx::query("SELECT pg_advisory_xact_lock(74105)")
        .execute(&mut *audio_blocker)
        .await
        .expect("hold Audio cycle barrier");
    let audio_handles = [(audio_a, audio_b), (audio_b, audio_a)]
        .into_iter()
        .map(|(source, target)| {
            let pool = db.pool.clone();
            tokio::spawn(async move {
                sqlx::query(
                    "UPDATE audio_identities
                     SET status='merged',merged_into_audio_identity_id=$2,
                         revision=revision+1,updated_at=$3
                     WHERE id=$1",
                )
                .bind(source)
                .bind(target)
                .bind(now + Duration::seconds(2))
                .execute(&pool)
                .await
            })
        })
        .collect::<Vec<_>>();
    wait_for_two_database_lock_waiters(&db).await;
    audio_blocker
        .commit()
        .await
        .expect("release Audio cycle barrier");
    let mut audio_commits = 0;
    let mut audio_rejections = 0;
    for handle in audio_handles {
        match handle.await.expect("Audio cycle task joins") {
            Ok(_) => audio_commits += 1,
            Err(_) => audio_rejections += 1,
        }
    }
    assert_eq!((audio_commits, audio_rejections), (1, 1));
    let audio_terminal: (Uuid, Uuid, i64) = sqlx::query_as(
        "SELECT resolve_audio_identity_redirect($1),resolve_audio_identity_redirect($2),
                (SELECT count(*) FROM audio_identities
                 WHERE id=ANY($3) AND status='merged')",
    )
    .bind(audio_a)
    .bind(audio_b)
    .bind([audio_a, audio_b])
    .fetch_one(&db.pool)
    .await
    .expect("AudioIdentity resolver remains total and acyclic");
    assert_eq!(audio_terminal.0, audio_terminal.1);
    assert_eq!(audio_terminal.2, 1);

    db.drop().await;
}

#[tokio::test]
async fn same_name_imports_serialize_on_the_artist_name_lock() {
    // The reuse decision is check-then-insert, so the deterministic proof is that a
    // concurrent importer of the same name waits for the in-flight one instead of
    // racing it, and then reuses the committed Artist.
    let db = TestDb::create("catalog_artist_name_lock").await;
    let now = OffsetDateTime::now_utc();
    let worker = WorkerId::new();
    let first = seed_published_release(
        &db,
        123,
        CatalogFixturePresentation {
            release_title: "Serialized One",
            release_artist: "Serialized Reuse Artist",
            track_title: "Serialized One",
            track_artist: "Serialized Reuse Artist",
            release_date: "2026-03-03",
        },
    )
    .await;
    let second = seed_published_release(
        &db,
        124,
        CatalogFixturePresentation {
            release_title: "Serialized Two",
            release_artist: "Serialized Reuse Artist",
            track_title: "Serialized Two",
            track_artist: "Serialized Reuse Artist",
            release_date: "2026-04-04",
        },
    )
    .await;
    assert_eq!(
        CatalogStore
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim both serialized fixtures")
            .len(),
        2
    );

    let mut holder = db.pool.begin().await.expect("open the lock holder");
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))")
            .bind("serialized reuse artist")
            .fetch_one(&mut *holder)
            .await
            .expect("take the Artist name lock");
    assert!(locked, "the fixture must own the name lock");

    let pool = db.pool.clone();
    let import_worker = worker.clone();
    let import = tokio::spawn(async move {
        CatalogStore
            .import_manifest_v3(
                &pool,
                first.work_item_id,
                &import_worker,
                &CatalogImportRequest::new("serialized-reuse-v1", "catalog-concurrency-test"),
                now,
            )
            .await
    });
    tokio::time::timeout(StdDuration::from_secs(10), async {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname=current_database() AND pid<>pg_backend_pid()
                   AND wait_event_type='Lock'
                   AND query LIKE '%pg_advisory_xact_lock%'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("query the blocked importer");
            if waiting >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the importer waits on the Artist name lock");
    assert!(!import.is_finished(), "the importer is still blocked");
    holder.rollback().await.expect("release the name lock");
    import
        .await
        .expect("serialized import task joins")
        .expect("first import completes after the lock is released");
    CatalogStore
        .import_manifest_v3(
            &db.pool,
            second.work_item_id,
            &worker,
            &CatalogImportRequest::new("serialized-reuse-v1", "catalog-concurrency-test"),
            now,
        )
        .await
        .expect("second import reuses the committed Artist");

    let artists: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM artists WHERE normalized_name='serialized reuse artist'",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read serialized reuse Artists");
    assert_eq!(artists.len(), 1, "the name lock guarantees one Artist");
    assert_eq!(artists[0].1, "active");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(DISTINCT release_id) FROM release_artists WHERE artist_id=$1",
        )
        .bind(artists[0].0)
        .fetch_one(&db.pool)
        .await
        .expect("count credits on the reused Artist"),
        2
    );
    db.drop().await;
}

#[tokio::test]
async fn concurrent_imports_of_one_artist_name_reuse_one_artist() {
    // ADR-0189: reuse is a read-then-insert decision, so two imports of the same
    // name running at the same time must not both insert. They serialize on the
    // name, and the loser reuses the winner's Artist instead of duplicating it.
    let db = TestDb::create("catalog_concurrent_artist_name_reuse").await;
    let now = OffsetDateTime::now_utc();
    let worker = WorkerId::new();
    let mut fixtures = Vec::new();
    for (ordinal, release_title) in [(121, "Concurrent Reuse One"), (122, "Concurrent Reuse Two")] {
        fixtures.push(
            seed_published_release(
                &db,
                ordinal,
                CatalogFixturePresentation {
                    release_title,
                    release_artist: "Concurrent Reuse Artist",
                    track_title: release_title,
                    track_artist: "Concurrent Reuse Artist",
                    release_date: "2026-02-02",
                },
            )
            .await,
        );
    }
    assert_eq!(
        CatalogStore
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim both concurrent fixtures")
            .len(),
        2
    );

    let mut tasks = tokio::task::JoinSet::new();
    for fixture in &fixtures {
        let pool = db.pool.clone();
        let worker = worker.clone();
        let work_item_id = fixture.work_item_id;
        tasks.spawn(async move {
            CatalogStore
                .import_manifest_v3(
                    &pool,
                    work_item_id,
                    &worker,
                    &CatalogImportRequest::new("concurrent-reuse-v1", "catalog-concurrency-test"),
                    now,
                )
                .await
        });
    }
    while let Some(joined) = tasks.join_next().await {
        joined
            .expect("concurrent import task joins")
            .expect("concurrent import succeeds");
    }

    let artists: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, status FROM artists WHERE normalized_name='concurrent reuse artist'
         ORDER BY created_at",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read concurrent reuse Artists");
    assert_eq!(
        artists.len(),
        1,
        "concurrent imports must not create a second Artist for one name"
    );
    assert_eq!(artists[0].1, "active");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(DISTINCT release_id) FROM release_artists WHERE artist_id=$1",
        )
        .bind(artists[0].0)
        .fetch_one(&db.pool)
        .await
        .expect("count credits on the reused Artist"),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artist_merge_events")
            .fetch_one(&db.pool)
            .await
            .expect("count merge events"),
        0,
        "serialized reuse needs no merge history"
    );
    db.drop().await;
}

async fn wait_for_one_database_lock_waiter(db: &TestDb) {
    tokio::time::timeout(StdDuration::from_secs(10), async {
        loop {
            let waiters: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity
                 WHERE datname=current_database() AND pid<>pg_backend_pid()
                   AND wait_event_type='Lock'",
            )
            .fetch_one(&db.pool)
            .await
            .expect("query genre lock waiter");
            if waiters >= 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("concurrent genre writer reached the aggregate lock boundary");
}

/// PERF-003 / RM-04 permanent regression.
///
/// Two concurrent transactions contribute the same new genre display value on
/// two different Releases. The old statement-trigger protocol counted each
/// transaction's own snapshot and the second overwrote the first, storing
/// `(1,1)` while authoritative terms held `(2,2)`. With the deterministic
/// advisory keys plus a fresh recount statement, the later transaction
/// observes the earlier commit and stores the exact counts.
#[tokio::test]
async fn concurrent_new_genre_contributions_are_not_lost_and_repair_converges() {
    let db = TestDb::create("catalog_genre_concurrency").await;
    let first = import_fixture(
        &db,
        211,
        CatalogFixturePresentation {
            release_title: "Genre Lock Release A",
            release_artist: "Genre Lock Artist A",
            track_title: "Genre Lock Track A",
            track_artist: "Genre Lock Artist A",
            release_date: "2012-01-01",
        },
    )
    .await;
    let second = import_fixture(
        &db,
        212,
        CatalogFixturePresentation {
            release_title: "Genre Lock Release B",
            release_artist: "Genre Lock Artist B",
            track_title: "Genre Lock Track B",
            track_artist: "Genre Lock Artist B",
            release_date: "2012-01-01",
        },
    )
    .await;

    let genre = format!("Concurrent Genre {}", Uuid::now_v7());
    let insert_sql = "INSERT INTO catalog_genre_terms (\
            track_id, release_id, surface, display_value, source_order\
         ) SELECT track.id, track.release_id, 'native', $1, 0 \
             FROM tracks track WHERE track.release_id = $2 \
             ORDER BY track.id LIMIT 1 ON CONFLICT DO NOTHING";

    let mut tx_a = db.pool.begin().await.expect("begin genre transaction A");
    sqlx::query(insert_sql)
        .bind(&genre)
        .bind(first.release_id.into_uuid())
        .execute(&mut *tx_a)
        .await
        .expect("insert genre contribution A");

    let pool_b = db.pool.clone();
    let genre_b = genre.clone();
    let release_b = second.release_id.into_uuid();
    let concurrent = tokio::spawn(async move {
        let mut tx = pool_b.begin().await.expect("begin genre transaction B");
        sqlx::query(insert_sql)
            .bind(&genre_b)
            .bind(release_b)
            .execute(&mut *tx)
            .await
            .expect("insert genre contribution B");
        tx.commit().await.expect("commit genre transaction B");
    });

    wait_for_one_database_lock_waiter(&db).await;
    tx_a.commit().await.expect("commit genre transaction A");
    concurrent.await.expect("concurrent genre writer");

    let stored: (i64, i64) = sqlx::query_as(
        "SELECT song_count, release_count FROM catalog_genre_aggregates
          WHERE surface = 'native' AND display_value = $1",
    )
    .bind(&genre)
    .fetch_one(&db.pool)
    .await
    .expect("stored aggregate row");
    assert_eq!(
        stored,
        (2, 2),
        "both concurrent contributions must be counted exactly once"
    );

    let authoritative: (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(DISTINCT release_id) FROM catalog_genre_terms
          WHERE surface = 'native' AND display_value = $1",
    )
    .bind(&genre)
    .fetch_one(&db.pool)
    .await
    .expect("authoritative term counts");
    assert_eq!(stored, authoritative);

    let clean: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_genre_aggregate_discrepancies(100)")
            .fetch_one(&db.pool)
            .await
            .expect("read-only repair check");
    assert_eq!(clean, 0, "a correct projection reports no discrepancies");

    // Deliberately corrupt the stored counts, then prove the repair operation
    // converges and is idempotent. Apply mode is exercised only on disposable
    // test data, never against a real library.
    sqlx::query(
        "UPDATE catalog_genre_aggregates SET song_count = 1, release_count = 1
          WHERE surface = 'native' AND display_value = $1",
    )
    .bind(&genre)
    .execute(&db.pool)
    .await
    .expect("corrupt stored aggregate");
    let (checked, repaired): (i32, i32) =
        sqlx::query_as("SELECT * FROM repair_catalog_genre_aggregates(true, 100)")
            .fetch_one(&db.pool)
            .await
            .expect("apply genre repair");
    assert!(checked >= 1);
    assert_eq!(checked, repaired);
    let repaired_row: (i64, i64) = sqlx::query_as(
        "SELECT song_count, release_count FROM catalog_genre_aggregates
          WHERE surface = 'native' AND display_value = $1",
    )
    .bind(&genre)
    .fetch_one(&db.pool)
    .await
    .expect("repaired aggregate row");
    assert_eq!(repaired_row, (2, 2));

    let after: (i32, i32) =
        sqlx::query_as("SELECT * FROM repair_catalog_genre_aggregates(true, 100)")
            .fetch_one(&db.pool)
            .await
            .expect("idempotent repair rerun");
    assert_eq!(after, (0, 0), "repair is idempotent on repaired data");

    // A real status transition to zero active contributors must remove the
    // stale aggregate row rather than leaving a zero-contributor key.
    sqlx::query(
        "UPDATE releases SET status = 'deprecated', revision = revision + 1, updated_at = now()
          WHERE id = ANY($1)",
    )
    .bind(vec![
        first.release_id.into_uuid(),
        second.release_id.into_uuid(),
    ])
    .execute(&db.pool)
    .await
    .expect("deprecate both genre contributors");
    let stale: Option<(i64, i64)> = sqlx::query_as(
        "SELECT song_count, release_count FROM catalog_genre_aggregates
          WHERE surface = 'native' AND display_value = $1",
    )
    .bind(&genre)
    .fetch_optional(&db.pool)
    .await
    .expect("read stale aggregate row");
    assert!(
        stale.is_none(),
        "a key with no active contributors must be removed, got {stale:?}"
    );

    // Overlay-only updates (no status change) must not corrupt counts.
    sqlx::query(
        "UPDATE releases SET title = title || ' (overlay)', revision = revision + 1, updated_at = now()
          WHERE id = $1",
    )
    .bind(second.release_id.into_uuid())
    .execute(&db.pool)
    .await
    .expect("overlay-only release update");
    let still_clean: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_genre_aggregate_discrepancies(100)")
            .fetch_one(&db.pool)
            .await
            .expect("post-overlay repair check");
    assert_eq!(
        still_clean, 0,
        "overlay-only updates must not change aggregates"
    );

    db.drop().await;
}
