//! Deterministic production drainer versus global candidate-oracle regression.
#![allow(clippy::expect_used)]

#[path = "../../melodee-persistence/tests/catalog_common/mod.rs"]
mod catalog_common;
#[path = "../../melodee-persistence/tests/common/mod.rs"]
mod common;

use catalog_common::{
    CatalogFixturePresentation, seed_legacy_repeated_track_artist_identity,
    seed_published_release_with_release_artists,
};
use common::{TestDb, test_config};
use melodee_catalog::{CatalogConfig, reconcile_affected_identities_once, start_catalog_loop};
use melodee_domain::{
    ArtistId, ArtistMatchDecision, ArtistMatchDecisionId, ArtistMatchOutcome,
    ArtistNegativeAssertion, ArtistNegativeAssertionId, Confidence, DecisionAuthority,
    NewArtistMatchDecision, ObservedArtistId,
};
use melodee_persistence::{CatalogImportRequest, CatalogStore, WorkEngineConfig, WorkerId};
use uuid::Uuid;

#[tokio::test]
async fn scoped_drain_matches_global_oracle_and_preserves_review_decisions() {
    let db = TestDb::create("identity_scoped_oracle").await;
    let store = CatalogStore;
    let now = time::OffsetDateTime::now_utc();
    let mut releases = Vec::new();
    let mut legacy = Vec::new();
    for (index, title, guest) in [
        (0, "Oracle Positive", "Positive Guest"),
        (1, "Oracle Negative", "Negative Guest"),
        (2, "Oracle Manual", "Manual Guest"),
    ] {
        let track = format!("Host Artist/{guest}");
        let fixture = seed_published_release_with_release_artists(
            &db,
            800 + index,
            CatalogFixturePresentation {
                release_title: title,
                release_artist: "Host Artist",
                track_title: title,
                track_artist: &track,
                release_date: "2001-01-01",
            },
            &["Host Artist"],
        )
        .await;
        let worker = WorkerId::new();
        store
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim fixture");
        store
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("identity-oracle", "test"),
                now,
            )
            .await
            .expect("import fixture");
        let duplicate = seed_legacy_repeated_track_artist_identity(
            &db,
            fixture.release_id,
            &guest.to_lowercase(),
        )
        .await;
        releases.push(fixture.release_id.into_uuid());
        legacy.push(duplicate);
    }
    let (negative_target, negative_source, negative_observation) = legacy[1];
    store
        .record_artist_negative_assertion(
            &db.pool,
            &ArtistNegativeAssertion {
                id: ArtistNegativeAssertionId::new(),
                observed_artist_id: ObservedArtistId::from_uuid(negative_observation),
                rejected_artist_id: ArtistId::from_uuid(negative_target),
                authority: DecisionAuthority::Administrator {
                    actor: "oracle-test".to_owned(),
                },
                locked: true,
                reason: "independent distinct artist evidence".to_owned(),
                decided_at: now,
            },
        )
        .await
        .expect("record negative assertion");
    let (manual_target, manual_source, manual_observation) = legacy[2];
    let locked = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: ArtistMatchDecisionId::new(),
            observed_artist_id: ObservedArtistId::from_uuid(manual_observation),
            artist_id: ArtistId::from_uuid(manual_source),
            outcome: ArtistMatchOutcome::Confirmed,
            authority: DecisionAuthority::Administrator {
                actor: "oracle-test".to_owned(),
            },
            locked: true,
            reason: "reviewed distinct artist".to_owned(),
            decided_at: now,
        },
        &[],
    )
    .expect("create manual lock");
    store
        .record_artist_match_decision(
            &db.pool,
            &locked,
            Confidence::new(10_000).expect("confidence"),
            serde_json::json!({"fixture":"oracle-manual"}),
        )
        .await
        .expect("record manual lock");

    // Three trusted-identity duplicates, plus same-name artists with conflicting
    // identifiers; names alone must not merge the latter pair.
    let mut artists = Vec::new();
    for (name, external) in [
        ("strong oracle one", "00000000-0000-7000-8000-000000000001"),
        ("strong oracle two", "00000000-0000-7000-8000-000000000001"),
        (
            "strong oracle three",
            "00000000-0000-7000-8000-000000000001",
        ),
        ("same name distinct", "00000000-0000-7000-8000-000000000002"),
        ("same name distinct", "00000000-0000-7000-8000-000000000003"),
    ] {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at) VALUES ($1,$2,$3,$3,$3,'unknown','active',now(),now())")
            .bind(id).bind(Uuid::now_v7()).bind(name).execute(&db.pool).await.expect("insert artist");
        sqlx::query("INSERT INTO artist_external_identities (id,artist_id,entity_type,provider,external_id,provider_unique_scope,confidence,evidence_source,is_locked,created_at) VALUES ($1,$2,'artist','musicbrainz',$3,false,9500,'oracle-test',false,now())")
            .bind(Uuid::now_v7()).bind(id).bind(external).execute(&db.pool).await.expect("insert strong evidence");
        artists.push(id);
    }
    // An unrelated, independently mergeable group is deliberately not seeded.
    // It distinguishes true scoped discovery from a global scan.
    let mut off_scope = Vec::new();
    for name in ["off scope one", "off scope two"] {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at) VALUES ($1,$2,$3,$3,$3,'unknown','active',now(),now())")
            .bind(id).bind(Uuid::now_v7()).bind(name).execute(&db.pool).await.expect("insert unrelated artist");
        sqlx::query("INSERT INTO artist_external_identities (id,artist_id,entity_type,provider,external_id,provider_unique_scope,confidence,evidence_source,is_locked,created_at) VALUES ($1,$2,'artist','musicbrainz','00000000-0000-7000-8000-000000000004',false,9500,'oracle-test',false,now())")
            .bind(Uuid::now_v7()).bind(id).execute(&db.pool).await.expect("insert unrelated evidence");
        off_scope.push(id);
    }
    // Historical repeated primary billing creates real, unlocked observations
    // on distinct Releases and also yields name-only review candidates.
    for release_id in &releases[1..] {
        let artist = Uuid::now_v7();
        let observed = Uuid::now_v7();
        let decision = Uuid::now_v7();
        sqlx::query("INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at) VALUES ($1,$2,'Oracle Billing','oracle billing','Oracle Billing','group','active',now(),now())")
            .bind(artist).bind(Uuid::now_v7()).execute(&db.pool).await.expect("insert billing artist");
        sqlx::query("INSERT INTO observed_artists (id,release_id,scope,source_ordinal,raw_value,normalized_value,display_value,source_pointer,source_payload,created_at) VALUES ($1,$2,'release',1000,'Oracle Billing','oracle billing','Oracle Billing','oracle:billing','{}'::jsonb,now())")
            .bind(observed).bind(release_id).execute(&db.pool).await.expect("insert billing observation");
        sqlx::query("INSERT INTO artist_match_decisions (id,observed_artist_id,artist_id,outcome,authority,confidence,reason,is_locked,payload,created_at) VALUES ($1,$2,$3,'confirmed','automated',9500,'oracle',false,'{}'::jsonb,now())")
            .bind(decision).bind(observed).bind(artist).execute(&db.pool).await.expect("insert billing decision");
        sqlx::query("INSERT INTO artist_match_current (observed_artist_id,decision_id,updated_at) VALUES ($1,$2,now())")
            .bind(observed).bind(decision).execute(&db.pool).await.expect("set billing decision");
        sqlx::query("UPDATE release_artists SET artist_id=$2,observed_artist_id=$3 WHERE release_id=$1 AND role='primary'")
            .bind(release_id).bind(artist).bind(observed).execute(&db.pool).await.expect("set billing");
        sqlx::query("INSERT INTO catalog_identity_reconciliation_seed (artist_id,observed_artist_id,created_at) VALUES ($1,$2,now())")
            .bind(artist).bind(observed).execute(&db.pool).await.expect("seed billing");
    }
    for id in &releases[1..] {
        sqlx::query("INSERT INTO release_external_identities(id,release_id,provider,external_id,trusted,confidence,evidence_source,is_locked,observed_at) VALUES($1,$2,'musicbrainz','10000000-0000-7000-8000-000000000001',true,9500,'oracle-test',false,now())")
            .bind(Uuid::now_v7()).bind(id).execute(&db.pool).await.expect("insert release evidence");
    }
    // Cover every deliberately affected observation and identity with durable
    // seeds; no unrelated catalogue candidates are present in this fixture.
    for (target, source, observation) in &legacy {
        sqlx::query("INSERT INTO catalog_identity_reconciliation_seed (artist_id,observed_artist_id,created_at) VALUES ($1,$2,now()),($3,$2,now())")
            .bind(target).bind(observation).bind(source).execute(&db.pool).await.expect("seed legacy");
    }
    for id in &artists {
        sqlx::query("INSERT INTO catalog_identity_reconciliation_seed (artist_id,created_at) VALUES ($1,now())")
            .bind(id).execute(&db.pool).await.expect("seed artist");
    }
    for id in &releases {
        sqlx::query("INSERT INTO catalog_identity_reconciliation_seed (release_id,created_at) VALUES ($1,now())")
            .bind(id).execute(&db.pool).await.expect("seed release");
    }
    let seeds: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM catalog_identity_reconciliation_seed ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .expect("read durable seeds");
    macro_rules! compare {
        ($scoped:expr, $global:expr) => {{
            let scoped = $scoped.await.expect("scoped candidates");
            let global = $global.await.expect("global candidates");
            assert!(global.len() < 1000, "global oracle must not be truncated");
            assert_eq!(scoped, global);
            global
        }};
    }
    let strong = store
        .seeded_strong_artist_merge_candidates(&db.pool, &seeds, 1000)
        .await
        .expect("scoped strong");
    let strong_global = store
        .strong_artist_merge_candidates(&db.pool, 1000)
        .await
        .expect("global strong");
    assert!(strong_global.len() < 1000);
    assert!(
        strong_global
            .iter()
            .any(|row| off_scope.contains(&row.source_artist_id)
                && off_scope.contains(&row.target_artist_id)),
        "oracle must include unrelated mergeable group"
    );
    assert_eq!(
        strong,
        strong_global
            .into_iter()
            .filter(|row| !off_scope.contains(&row.source_artist_id)
                && !off_scope.contains(&row.target_artist_id))
            .collect::<Vec<_>>(),
        "scope must exclude unrelated group"
    );
    assert!(
        !strong.is_empty(),
        "fixture exercises strong Artist identity"
    );
    let same = compare!(
        store.seeded_same_release_track_artist_merge_candidates(&db.pool, &seeds, 1000),
        store.same_release_track_artist_merge_candidates(&db.pool, 1000)
    );
    assert!(
        same.iter()
            .any(|candidate| candidate.source_artist_id == legacy[0].1)
    );
    assert!(
        !same
            .iter()
            .any(|candidate| candidate.source_artist_id == negative_source
                || candidate.source_artist_id == manual_source)
    );
    let repeated = compare!(
        store.seeded_repeated_artist_credit_merge_candidates(&db.pool, &seeds, 1000),
        store.repeated_artist_credit_merge_candidates(&db.pool, 1000)
    );
    assert!(
        !repeated.is_empty(),
        "fixture must exercise repeated-credit candidates"
    );
    let names = compare!(
        store.seeded_name_only_artist_match_proposal_candidates(&db.pool, &seeds, 1000),
        store.name_only_artist_match_proposal_candidates(&db.pool, 1000)
    );
    assert!(
        !names.is_empty(),
        "fixture must exercise name-only proposals"
    );
    let release = compare!(
        store.seeded_strong_release_merge_candidates(&db.pool, &seeds, 1000),
        store.strong_release_merge_candidates(&db.pool, 1000)
    );
    assert!(
        !release.is_empty(),
        "fixture exercises strong Release identity"
    );

    // A failed production discovery must leave the exact captured IDs durable.
    sqlx::query("ALTER TABLE artist_external_identities RENAME TO oracle_unavailable_identities")
        .execute(&db.pool)
        .await
        .expect("inject discovery interruption");
    assert!(reconcile_affected_identities_once(&store, &db.pool).await);
    let after_failure: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM catalog_identity_reconciliation_seed ORDER BY id")
            .fetch_all(&db.pool)
            .await
            .expect("read seeds after failure");
    assert!(
        seeds.iter().all(|id| after_failure.contains(id)),
        "a failed discovery must retain all captured seeds; independently committed work may add seeds"
    );
    sqlx::query("ALTER TABLE oracle_unavailable_identities RENAME TO artist_external_identities")
        .execute(&db.pool)
        .await
        .expect("restore discovery");
    // Late-committing lower-ID work must survive the active drain.
    let mut late = db.pool.begin().await.expect("late transaction");
    let late_id: i64 = sqlx::query_scalar("INSERT INTO catalog_identity_reconciliation_seed (normalized_name,created_at) VALUES ('late oracle seed',now()) RETURNING id")
        .fetch_one(&mut *late).await.expect("allocate late seed");
    sqlx::query("INSERT INTO catalog_identity_reconciliation_seed (normalized_name,created_at) VALUES ('visible oracle seed',now())")
        .execute(&db.pool).await.expect("commit visible seed");
    assert!(reconcile_affected_identities_once(&store, &db.pool).await);
    late.commit().await.expect("commit late seed");
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM catalog_identity_reconciliation_seed WHERE id=$1)"
        )
        .bind(late_id)
        .fetch_one(&db.pool)
        .await
        .expect("read late seed")
    );
    for _ in 0..10 {
        if !reconcile_affected_identities_once(&store, &db.pool).await {
            break;
        }
    }
    for id in [
        negative_target,
        negative_source,
        manual_target,
        manual_source,
        artists[3],
        artists[4],
        off_scope[0],
        off_scope[1],
    ] {
        let status: String = sqlx::query_scalar("SELECT status FROM artists WHERE id=$1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .expect("read protected artist");
        assert_eq!(
            status, "active",
            "manual, negative and conflicting identity evidence must remain distinct"
        );
    }
    let positive_status: String = sqlx::query_scalar("SELECT status FROM artists WHERE id=$1")
        .bind(legacy[0].1)
        .fetch_one(&db.pool)
        .await
        .expect("read positive artist");
    assert_eq!(
        positive_status, "merged",
        "unlocked historical credit should converge"
    );
    let redirect: Uuid = sqlx::query_scalar("SELECT resolve_artist_redirect($1)")
        .bind(legacy[0].1)
        .fetch_one(&db.pool)
        .await
        .expect("resolve reconciled historical artist");
    assert_eq!(
        redirect, legacy[0].0,
        "reconciled redirect must preserve identity"
    );
    let resolved: Vec<Uuid> = sqlx::query_scalar(
        "SELECT resolve_artist_redirect(id) FROM unnest($1::uuid[]) id ORDER BY id",
    )
    .bind(&artists[..3])
    .fetch_all(&db.pool)
    .await
    .expect("resolve multi-Artist closure");
    assert_eq!(resolved.len(), 3);
    assert!(
        resolved.iter().all(|id| *id == resolved[0]),
        "the full strong identity closure must converge through redirects"
    );
    let release_state: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT status,merged_into_release_id FROM releases WHERE id=ANY($1) ORDER BY status",
    )
    .bind(&releases[1..])
    .fetch_all(&db.pool)
    .await
    .expect("read strong Release outcome");
    assert_eq!(release_state.len(), 2);
    assert_eq!(release_state[0], ("active".to_owned(), None));
    assert_eq!(release_state[1].0, "merged");
    let target = release_state[1].1.expect("merged Release redirect");
    assert!(releases[1..].contains(&target));
    let target_active: bool =
        sqlx::query_scalar("SELECT status='active' FROM releases WHERE id=$1")
            .bind(target)
            .fetch_one(&db.pool)
            .await
            .expect("read redirected Release");
    assert!(target_active);
    let merge_events: i64 = sqlx::query_scalar("SELECT count(*) FROM release_merge_events WHERE source_release_id=ANY($1) AND target_release_id=ANY($1) AND actor='catalog-reconciler'")
        .bind(&releases[1..]).fetch_one(&db.pool).await.expect("read Release merge audit");
    assert_eq!(merge_events, 1);
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed")
            .fetch_one(&db.pool)
            .await
            .expect("count remaining closure seeds");
    assert_eq!(
        remaining, 0,
        "bounded turns must eventually consume closure and late seeds"
    );
    db.drop().await;
}

async fn multihop_fixture(label: &str) -> (TestDb, Uuid, Uuid, Uuid, i64) {
    let db = TestDb::create(label).await;
    let store = CatalogStore;
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        820,
        CatalogFixturePresentation {
            release_title: "Multi-hop Release",
            release_artist: "Hop Host",
            track_title: "Hop Track",
            track_artist: "Hop Host/Hop Guest",
            release_date: "2001-01-01",
        },
        &["Hop Host"],
    )
    .await;
    let worker = WorkerId::new();
    store
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim");
    store
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("identity-multihop", "test"),
            now,
        )
        .await
        .expect("import");
    let (middle, last, _) =
        seed_legacy_repeated_track_artist_identity(&db, fixture.release_id, "hop guest").await;
    // Strong reconciliation directs the lower UUID into the higher UUID.
    // Keep B's track-credit name as the survivor so the distinct B↔C edge
    // remains valid after the first merge.
    let first =
        Uuid::parse_str("00000000-0000-7000-8000-000000000001").expect("valid low UUIDv7 fixture");
    sqlx::query("INSERT INTO artists (id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at) VALUES ($1,$2,'Hop Provider','hop provider','Hop Provider','unknown','active',now(),now())")
        .bind(first).bind(Uuid::now_v7()).execute(&db.pool).await.expect("first artist");
    for artist in [first, middle] {
        sqlx::query("INSERT INTO artist_external_identities (id,artist_id,entity_type,provider,external_id,provider_unique_scope,confidence,evidence_source,is_locked,created_at) VALUES ($1,$2,'artist','musicbrainz','00000000-0000-7000-8000-000000000005',false,9500,'multihop',false,now())")
            .bind(Uuid::now_v7()).bind(artist).execute(&db.pool).await.expect("strong edge");
    }
    sqlx::query("DELETE FROM catalog_identity_reconciliation_seed")
        .execute(&db.pool)
        .await
        .expect("isolate starting seed");
    let seed: i64 = sqlx::query_scalar("INSERT INTO catalog_identity_reconciliation_seed (artist_id,created_at) VALUES ($1,now()) RETURNING id")
        .bind(first).fetch_one(&db.pool).await.expect("seed only first artist");
    (db, first, middle, last, seed)
}

/// Strong evidence joins A to B, then the historical Track credit joins B to C.
/// Only A is seeded: C is initially invisible to the scoped same-Release query.
#[tokio::test]
async fn one_seed_expands_across_distinct_identity_relationships() {
    let (db, first, middle, last, seed) = multihop_fixture("identity_scoped_multihop").await;
    let store = CatalogStore;
    let strong = store
        .seeded_strong_artist_merge_candidates(&db.pool, &[seed], 64)
        .await
        .expect("first edge");
    assert!(
        strong
            .iter()
            .any(|row| [row.source_artist_id, row.target_artist_id].contains(&middle))
    );
    assert!(
        store
            .seeded_same_release_track_artist_merge_candidates(&db.pool, &[seed], 64)
            .await
            .expect("initial same-release scope")
            .is_empty(),
        "third artist must start outside seed scope"
    );
    for _ in 0..10 {
        if !reconcile_affected_identities_once(&store, &db.pool).await {
            break;
        }
    }
    let redirects: Vec<Uuid> = sqlx::query_scalar(
        "SELECT resolve_artist_redirect(id) FROM unnest($1::uuid[]) id ORDER BY id",
    )
    .bind([first, middle, last])
    .fetch_all(&db.pool)
    .await
    .expect("resolve multihop closure");
    assert_eq!(redirects.len(), 3);
    assert!(
        redirects.iter().all(|id| *id == redirects[0]),
        "one seed must discover and converge the whole relationship chain"
    );
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM catalog_identity_reconciliation_seed")
            .fetch_one(&db.pool)
            .await
            .expect("remaining seeds");
    assert_eq!(remaining, 0, "closure must complete in bounded turns");
    // An independent, identically shaped database starts without seeds. Its
    // production startup backstop performs global discovery; newly generated
    // seeds may then be drained by that same scheduler. Compare outcomes, not
    // UUID winners, because each fixture has distinct v7 identifiers.
    let (global_db, global_first, global_middle, global_last, _) =
        multihop_fixture("identity_global_multihop").await;
    // Independent historical Releases have no seeds. Their only discovery
    // route is the separately scheduled global strong-Release backstop.
    let global_releases = [Uuid::now_v7(), Uuid::now_v7()];
    for id in global_releases {
        sqlx::query("INSERT INTO releases (id,public_api_id,title,normalized_title,sort_title,status,created_at,updated_at) VALUES ($1,$2,'Historical Oracle Release','historical oracle release','Historical Oracle Release','active',now(),now())")
            .bind(id).bind(Uuid::now_v7()).execute(&global_db.pool).await.expect("insert historical Release");
        sqlx::query("INSERT INTO release_external_identities (id,release_id,provider,external_id,trusted,confidence,evidence_source,is_locked,observed_at) VALUES ($1,$2,'musicbrainz','10000000-0000-7000-8000-000000000002',true,9500,'oracle-test',false,now())")
            .bind(Uuid::now_v7()).bind(id).execute(&global_db.pool).await.expect("insert historical Release evidence");
    }
    sqlx::query("DELETE FROM catalog_identity_reconciliation_seed")
        .execute(&global_db.pool)
        .await
        .expect("require global startup discovery");
    let handle = start_catalog_loop(
        CatalogConfig {
            concurrency: 1,
            poll_interval: time::Duration::hours(1),
            ..CatalogConfig::default()
        },
        global_db.pool.clone(),
        CatalogStore,
        WorkEngineConfig::default(),
    );
    let completed = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let ids = [global_first, global_middle, global_last];
            let resolved: Vec<Uuid> = sqlx::query_scalar(
                "SELECT resolve_artist_redirect(id) FROM unnest($1::uuid[]) id ORDER BY id",
            )
            .bind(ids)
            .fetch_all(&global_db.pool)
            .await
            .expect("global redirects");
            let release_merged: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM releases WHERE id=ANY($1) AND status='merged'",
            )
            .bind(global_releases)
            .fetch_one(&global_db.pool)
            .await
            .expect("global Release outcome");
            if resolved.len() == 3
                && resolved.iter().all(|id| *id == resolved[0])
                && release_merged == 1
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    handle.stop().await;
    completed.expect("global startup backstops converge Artist and Release evidence");
    let release_state: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT status,merged_into_release_id FROM releases WHERE id=ANY($1) ORDER BY status",
    )
    .bind(global_releases)
    .fetch_all(&global_db.pool)
    .await
    .expect("global Release states");
    assert_eq!(release_state.len(), 2);
    assert_eq!(release_state[0], ("active".to_owned(), None));
    assert_eq!(release_state[1].0, "merged");
    let release_target = release_state[1].1.expect("global Release redirect");
    assert!(global_releases.contains(&release_target));
    let event_count: i64 = sqlx::query_scalar("SELECT count(*) FROM release_merge_events WHERE source_release_id=ANY($1) AND target_release_id=ANY($1) AND actor='catalog-reconciler'")
        .bind(global_releases).fetch_one(&global_db.pool).await.expect("global Release audit");
    assert_eq!(event_count, 1);
    for (pool, ids) in [
        (&db.pool, [first, middle, last]),
        (&global_db.pool, [global_first, global_middle, global_last]),
    ] {
        let counts: (i64, i64) = sqlx::query_as("SELECT count(*) FILTER (WHERE status='active'), count(*) FILTER (WHERE status='merged') FROM artists WHERE id=ANY($1)")
            .bind(ids).fetch_one(pool).await.expect("closure outcome counts");
        assert_eq!(
            counts,
            (1, 2),
            "global and scoped paths must preserve the same identity partition"
        );
    }
    global_db.drop().await;
    db.drop().await;
}
