//! Cross-slice PostgreSQL acceptance tests for the v3 catalog handoff.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{
    CatalogFixturePresentation, seed_legacy_repeated_track_artist_identity, seed_published_release,
    seed_published_release_with_musicbrainz_ids, seed_published_release_with_release_artists,
};
use common::{TestDb, test_config};
use melodee_domain::{
    ArtistId, ArtistMatchDecision, ArtistMatchDecisionId, ArtistMatchOutcome, ArtistMerge,
    ArtistMergeId, ArtistNegativeAssertion, ArtistNegativeAssertionId, ArtistPublicId, Confidence,
    DecisionAuthority, FailureClass, IngestionState, NewArtistMatchDecision, ObservedArtistId,
    PUBLICATION_MANIFEST_VERSION, RetryPolicy, WorkErrorCode,
};
use melodee_persistence::{
    ArtistReconciliationEvidence, CatalogImportRequest, CatalogStore, WorkItemStore, WorkerId,
};
use time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn slash_billing_keeps_release_artist_primary_and_creates_featured_track_artists() {
    let db = TestDb::create("catalog_slash_track_billing").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        92,
        CatalogFixturePresentation {
            release_title: "Digital Dancer",
            release_artist: "6arelyhuman",
            track_title: "Abyss",
            track_artist: "6arelyhuman/Baby Jane",
            release_date: "2024-05-03",
        },
        &["6arelyhuman"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim slash billing import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import slash billing");
    assert_eq!(imported.artist_count, 2);

    let release_artists: Vec<String> = sqlx::query_scalar(
        "SELECT artist.canonical_name
           FROM release_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
          WHERE billing.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read Release Artists");
    assert_eq!(release_artists, ["6arelyhuman"]);

    let track_artists: Vec<(String, String, String, i32)> = sqlx::query_as(
        "SELECT artist.canonical_name, billing.role, billing.join_phrase,
                billing.presentation_order
           FROM track_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
           JOIN tracks track ON track.id=billing.track_id
          WHERE track.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read structured Track Artists");
    assert_eq!(
        track_artists,
        vec![
            (
                "6arelyhuman".to_owned(),
                "primary".to_owned(),
                "/".to_owned(),
                1
            ),
            (
                "Baby Jane".to_owned(),
                "featured".to_owned(),
                String::new(),
                2
            ),
        ]
    );
    let display_artist: String =
        sqlx::query_scalar("SELECT display_artist FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read exact Track DisplayArtist");
    assert_eq!(display_artist, "6arelyhuman/Baby Jane");
    let associations: Vec<(String, String)> = sqlx::query_as(
        "SELECT artist.canonical_name, association.association_type
           FROM artist_track_associations association
           JOIN artists artist ON artist.id=association.artist_id
           JOIN tracks track ON track.id=association.track_id
          WHERE track.release_id=$1
          ORDER BY artist.canonical_name, association.association_type",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read Artist participation");
    assert!(
        associations
            .iter()
            .any(|row| row == &("Baby Jane".to_owned(), "featured_artist".to_owned()))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists WHERE canonical_name='6arelyhuman/Baby Jane'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count compound identities"),
        0
    );
    db.drop().await;
}

#[tokio::test]
async fn slash_billing_supports_multiple_featured_track_artists() {
    let db = TestDb::create("catalog_multi_feature_slash_billing").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        93,
        CatalogFixturePresentation {
            release_title: "Woounded, Vol. 2",
            release_artist: "9000 Rondae",
            track_title: "Face Hot",
            track_artist: "9000 Rondae/Glockboyz Teejaee/OnFully/Roadrunner Glockboyz Tez",
            release_date: "2023-01-01",
        },
        &["9000 Rondae"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim multiple-feature import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import multiple-feature billing");
    assert_eq!(imported.artist_count, 4);

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT artist.canonical_name, billing.role, billing.join_phrase
           FROM track_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
           JOIN tracks track ON track.id=billing.track_id
          WHERE track.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read multiple structured Track Artists");
    assert_eq!(
        rows,
        vec![
            (
                "9000 Rondae".to_owned(),
                "primary".to_owned(),
                "/".to_owned()
            ),
            (
                "Glockboyz Teejaee".to_owned(),
                "featured".to_owned(),
                "/".to_owned()
            ),
            ("OnFully".to_owned(), "featured".to_owned(), "/".to_owned()),
            (
                "Roadrunner Glockboyz Tez".to_owned(),
                "featured".to_owned(),
                String::new()
            ),
        ]
    );
    db.drop().await;
}

#[tokio::test]
async fn explicit_track_billing_phrases_create_featured_associations() {
    let db = TestDb::create("catalog_explicit_track_billing").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        94,
        CatalogFixturePresentation {
            release_title: "Explicit Billing",
            release_artist: "Somebody",
            track_title: "Together",
            track_artist: "Somebody Featuring Colin Hay with Guest",
            release_date: "2026-01-01",
        },
        &["Somebody"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim explicit billing import");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import explicit billing");

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT artist.canonical_name, billing.role, billing.join_phrase
           FROM track_artists billing
           JOIN artists artist ON artist.id=billing.artist_id
           JOIN tracks track ON track.id=billing.track_id
          WHERE track.release_id=$1 ORDER BY billing.presentation_order",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read explicit structured Track Artists");
    assert_eq!(
        rows,
        vec![
            (
                "Somebody".to_owned(),
                "primary".to_owned(),
                " Featuring ".to_owned()
            ),
            (
                "Colin Hay".to_owned(),
                "featured".to_owned(),
                " with ".to_owned()
            ),
            ("Guest".to_owned(), "featured".to_owned(), String::new()),
        ]
    );
    db.drop().await;
}

#[tokio::test]
async fn exact_release_artist_with_a_slash_remains_a_literal_identity() {
    let db = TestDb::create("catalog_literal_slash_artist").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        95,
        CatalogFixturePresentation {
            release_title: "Literal Slash",
            release_artist: "$NOT/Jeht",
            track_title: "No Split",
            track_artist: "$NOT/Jeht",
            release_date: "2026-01-01",
        },
        &["$NOT/Jeht"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim literal slash import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("structured-track-billing-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import literal slash Artist");
    assert_eq!(imported.artist_count, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM track_artists billing
              JOIN tracks track ON track.id=billing.track_id
             WHERE track.release_id=$1",
        )
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("count literal Track Artist rows"),
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn multi_artist_release_reuses_the_one_exact_track_artist_match() {
    let db = TestDb::create("catalog_multi_artist_exact_match").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        90,
        CatalogFixturePresentation {
            release_title: "High Risk",
            release_artist: "$NOT",
            track_title: "Locked In",
            track_artist: "$NOT",
            release_date: "2026-01-02",
        },
        &["$NOT", "$NOT/Jeht"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim exact-match import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-unique-match-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import multi-Artist Release");
    assert_eq!(
        imported.artist_count, 2,
        "the matching Track must not allocate a third Artist identity"
    );

    let rows: Vec<(String, String, uuid::Uuid)> = sqlx::query_as(
        "SELECT observed.scope, observed.normalized_value, decision.artist_id
           FROM observed_artists observed
           JOIN artist_match_current current ON current.observed_artist_id=observed.id
           JOIN artist_match_decisions decision ON decision.id=current.decision_id
          WHERE observed.release_id=$1
          ORDER BY observed.scope, observed.normalized_value, observed.id",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read multi-Artist decisions");
    assert_eq!(rows.len(), 3, "all raw observations remain distinct");
    let release_not = rows
        .iter()
        .find(|(scope, normalized, _)| scope == "release" && normalized == "$not")
        .expect("release $NOT observation");
    let release_collaboration = rows
        .iter()
        .find(|(scope, normalized, _)| scope == "release" && normalized == "$not/jeht")
        .expect("collaboration Release observation");
    let track_not = rows
        .iter()
        .find(|(scope, normalized, _)| scope == "track" && normalized == "$not")
        .expect("track $NOT observation");
    assert_eq!(track_not.2, release_not.2);
    assert_ne!(track_not.2, release_collaboration.2);
    db.drop().await;
}

#[tokio::test]
async fn multi_artist_release_keeps_equal_normalized_release_matches_ambiguous() {
    let db = TestDb::create("catalog_multi_artist_ambiguous_match").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        89,
        CatalogFixturePresentation {
            release_title: "Ambiguous Billing",
            release_artist: "Same Name",
            track_title: "Do Not Conflate",
            track_artist: "Same Name",
            release_date: "2026-01-01",
        },
        &["Same Name", " SAME NAME "],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim ambiguous import");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-unique-match-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import ambiguous multi-Artist Release");
    assert_eq!(
        imported.artist_count, 3,
        "two equal Release observations are not a unique identity match"
    );
    assert!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan same-Release Track candidates")
            .is_empty(),
        "Release Artist ambiguity must not enter Track-only reconciliation"
    );
    assert!(
        !catalog
            .name_only_artist_match_proposal_candidates(&db.pool, 64)
            .await
            .expect("scan exact-name review candidates")
            .is_empty(),
        "same-Release ambiguity remains reviewable without automatic convergence"
    );
    db.drop().await;
}

#[tokio::test]
async fn repeated_track_artist_credit_reconciliation_is_release_scoped_audited_and_unlocked() {
    let db = TestDb::create("catalog_same_release_track_reconciliation").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        88,
        CatalogFixturePresentation {
            release_title: "Woounded, Vol. 2",
            release_artist: "9000 Rondae",
            track_title: "Face Hot",
            track_artist: "9000 Rondae/Glockboyz Teejaee",
            release_date: "2023-01-01",
        },
        &["9000 Rondae"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim repeated Track credit fixture");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-track-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import Track billing fixture");
    let (_, legacy_source_artist_id, legacy_observation_id) =
        seed_legacy_repeated_track_artist_identity(&db, fixture.release_id, "glockboyz teejaee")
            .await;

    let candidates = catalog
        .same_release_track_artist_merge_candidates(&db.pool, 64)
        .await
        .expect("scan repeated same-Release Track credits");
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.release_id, fixture.release_id.into_uuid());
    assert_eq!(candidate.normalized_name, "glockboyz teejaee");
    assert_eq!(candidate.source_artist_id, legacy_source_artist_id);
    let merge = ArtistMerge {
        id: ArtistMergeId::new(),
        source_artist_id: ArtistId::from_uuid(candidate.source_artist_id),
        source_public_api_id: ArtistPublicId::from_uuid(candidate.source_public_api_id),
        target_artist_id: ArtistId::from_uuid(candidate.target_artist_id),
        actor: "catalog-reconciler".to_owned(),
        reason: "ADR-0173 repeated same-Release Track credit".to_owned(),
        occurred_at: time::OffsetDateTime::now_utc(),
    };
    catalog
        .reconcile_artist(
            &db.pool,
            &merge,
            "same-release-track-v1",
            &ArtistReconciliationEvidence::SameReleaseTrackCredit {
                release_id: candidate.release_id,
                normalized_name: candidate.normalized_name.clone(),
            },
        )
        .await
        .expect("reconcile repeated Track credit");

    let statuses: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='provisional'),
                count(*) FILTER (WHERE status='merged')
           FROM artists WHERE normalized_name='glockboyz teejaee'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read repeated credit Artist statuses");
    assert_eq!(statuses, (1, 1));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM track_artists WHERE artist_id=$1",)
            .bind(candidate.target_artist_id)
            .fetch_one(&db.pool)
            .await
            .expect("count retargeted Track credits"),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM observed_artists WHERE id=$1")
            .bind(legacy_observation_id)
            .fetch_one(&db.pool)
            .await
            .expect("read retained raw observation"),
        1
    );
    let replacement: (String, bool, serde_json::Value) = sqlx::query_as(
        "SELECT decision.authority, decision.is_locked, decision.payload
           FROM artist_match_current current
           JOIN artist_match_decisions decision ON decision.id=current.decision_id
          WHERE current.observed_artist_id=$1",
    )
    .bind(legacy_observation_id)
    .fetch_one(&db.pool)
    .await
    .expect("read automated replacement decision");
    assert_eq!(replacement.0, "automated");
    assert!(!replacement.1);
    assert_eq!(
        replacement.2["reconciliation_predicate"],
        "same_release_track_credit"
    );
    let evidence: Vec<String> = sqlx::query_scalar(
        "SELECT evidence_kind FROM artist_match_evidence
          WHERE observed_artist_id=$1 AND candidate_artist_id=$2
          ORDER BY evidence_kind",
    )
    .bind(legacy_observation_id)
    .bind(candidate.target_artist_id)
    .fetch_all(&db.pool)
    .await
    .expect("read retained reconciliation evidence");
    assert_eq!(evidence, ["name_similarity", "release_context"]);
    let audit: serde_json::Value = sqlx::query_scalar(
        "SELECT moved_relationships FROM artist_merge_events
          WHERE source_artist_id=$1 AND target_artist_id=$2",
    )
    .bind(candidate.source_artist_id)
    .bind(candidate.target_artist_id)
    .fetch_one(&db.pool)
    .await
    .expect("read repeated credit merge audit");
    assert_eq!(
        audit["reconciliation"]["evidence"]["predicate"],
        "same_release_track_credit"
    );
    assert_eq!(
        audit["reconciliation"]["evidence"]["release_id"],
        candidate.release_id.to_string()
    );
    assert!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("repeat reconciliation scan")
            .is_empty(),
        "reconciliation is idempotent"
    );
    db.drop().await;
}

#[tokio::test]
async fn conflicting_provider_ids_block_same_release_track_artist_reconciliation() {
    let db = TestDb::create("catalog_same_release_track_provider_conflict").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        87,
        CatalogFixturePresentation {
            release_title: "Provider Conflict",
            release_artist: "Host Artist",
            track_title: "Shared Stage",
            track_artist: "Host Artist/Conflict Guest",
            release_date: "2026-01-01",
        },
        &["Host Artist"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim provider-conflict fixture");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-track-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import provider-conflict fixture");
    let (target_artist_id, source_artist_id, _) =
        seed_legacy_repeated_track_artist_identity(&db, fixture.release_id, "conflict guest").await;
    for (artist_id, external_id) in [
        (target_artist_id, "aa1b1c1d-0000-7000-8000-000000000051"),
        (source_artist_id, "aa1b1c1d-0000-7000-8000-000000000052"),
    ] {
        sqlx::query(
            "INSERT INTO artist_external_identities (
                id, artist_id, provider, external_id, confidence,
                evidence_source, created_at
             ) VALUES ($1,$2,'musicbrainz',$3,9000,'catalog-test',$4)",
        )
        .bind(uuid::Uuid::now_v7())
        .bind(artist_id)
        .bind(external_id)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("insert conflicting provider identity");
    }
    assert!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan provider-conflicted Track credits")
            .is_empty(),
        "different IDs from the same provider must block co-resolution"
    );
    db.drop().await;
}

#[tokio::test]
async fn compilation_evidence_blocks_same_release_track_artist_reconciliation() {
    let db = TestDb::create("catalog_same_release_track_compilation").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        85,
        CatalogFixturePresentation {
            release_title: "Compilation Context",
            release_artist: "Host Artist",
            track_title: "Shared Stage",
            track_artist: "Host Artist/Compilation Guest",
            release_date: "2026-01-01",
        },
        &["Host Artist"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim compilation fixture");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-track-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import compilation fixture");
    seed_legacy_repeated_track_artist_identity(&db, fixture.release_id, "compilation guest").await;
    assert_eq!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan pre-classification Track credits")
            .len(),
        1
    );

    sqlx::query(
        "INSERT INTO release_catalog_trait_assertions (
            id, release_id, trait, source_version, actor, reason, evidence, created_at
         ) VALUES ($1,$2,'compilation','catalog-test-v1','catalog-test',
                   'fixture compilation evidence',$3,$4)",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(fixture.release_id.into_uuid())
    .bind(serde_json::json!({"source": "test"}))
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert compilation evidence");

    assert!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan compilation Track credits")
            .is_empty(),
        "known compilations fail closed instead of co-resolving repeated names"
    );
    db.drop().await;
}

#[tokio::test]
async fn negative_assertions_and_locked_decisions_block_same_release_track_reconciliation() {
    let db = TestDb::create("catalog_same_release_track_manual_conflict").await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let fixture = seed_published_release_with_release_artists(
        &db,
        86,
        CatalogFixturePresentation {
            release_title: "Manual Conflict",
            release_artist: "Host Artist",
            track_title: "Shared Stage",
            track_artist: "Host Artist/Manual Guest",
            release_date: "2026-01-01",
        },
        &["Host Artist"],
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim manual-conflict fixture");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("same-release-track-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import manual-conflict fixture");
    let (target_artist_id, source_artist_id, source_observation_id) =
        seed_legacy_repeated_track_artist_identity(&db, fixture.release_id, "manual guest").await;
    assert_eq!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan initial same-Release candidate")
            .len(),
        1
    );

    let assertion_id = ArtistNegativeAssertionId::new();
    catalog
        .record_artist_negative_assertion(
            &db.pool,
            &ArtistNegativeAssertion {
                id: assertion_id,
                observed_artist_id: ObservedArtistId::from_uuid(source_observation_id),
                rejected_artist_id: ArtistId::from_uuid(target_artist_id),
                authority: DecisionAuthority::Administrator {
                    actor: "catalog-test".to_owned(),
                },
                locked: true,
                reason: "same Track display name is not the same Artist".to_owned(),
                decided_at: now + Duration::seconds(1),
            },
        )
        .await
        .expect("record same-Release do-not-match assertion");
    assert!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan negative-assertion candidates")
            .is_empty()
    );

    catalog
        .revoke_artist_negative_assertion(
            &db.pool,
            assertion_id.into_uuid(),
            &DecisionAuthority::Administrator {
                actor: "catalog-test".to_owned(),
            },
            "resume same-Release evidence evaluation",
            now + Duration::seconds(2),
        )
        .await
        .expect("revoke same-Release do-not-match assertion");
    assert_eq!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan after same-Release assertion revocation")
            .len(),
        1
    );

    let locked = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: ArtistMatchDecisionId::new(),
            observed_artist_id: ObservedArtistId::from_uuid(source_observation_id),
            artist_id: ArtistId::from_uuid(source_artist_id),
            outcome: ArtistMatchOutcome::Confirmed,
            authority: DecisionAuthority::Administrator {
                actor: "catalog-test".to_owned(),
            },
            locked: true,
            reason: "administrator locked this same-Release identity".to_owned(),
            decided_at: now + Duration::seconds(3),
        },
        &[],
    )
    .expect("construct locked same-Release decision");
    catalog
        .record_artist_match_decision(
            &db.pool,
            &locked,
            Confidence::new(10_000).expect("valid confidence"),
            serde_json::json!({"fixture": "same-release-track-lock"}),
        )
        .await
        .expect("record locked same-Release decision");
    assert!(
        catalog
            .same_release_track_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan locked same-Release candidates")
            .is_empty()
    );
    db.drop().await;
}

#[tokio::test]
async fn same_name_release_credits_reuse_one_artist_across_releases() {
    // ADR-0189: C# resolves every credit against a normalized-name lookup at
    // import, so two Releases by the same artist name produce one Artist row and
    // there is nothing left for reconciliation to merge.
    let db = TestDb::create("catalog_repeated_primary_artist").await;
    let first = seed_published_release(
        &db,
        96,
        CatalogFixturePresentation {
            release_title: "Alice Cooper Goes to Hell",
            release_artist: "Alice Cooper",
            track_title: "Go to Hell",
            track_artist: "Alice Cooper",
            release_date: "1976-06-25",
        },
    )
    .await;
    let second = seed_published_release(
        &db,
        97,
        CatalogFixturePresentation {
            release_title: "Brutal Planet",
            release_artist: "Alice Cooper",
            track_title: "Brutal Planet",
            track_artist: "Alice Cooper",
            release_date: "2000-06-06",
        },
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim cross-Release fixtures")
            .len(),
        2
    );
    for fixture in [&first, &second] {
        catalog
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("cross-release-name-v1", "catalog-test"),
                now,
            )
            .await
            .expect("import cross-Release fixture");
    }

    let artists: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT artist.id,
                (SELECT count(*) FROM release_artists credit
                  WHERE credit.artist_id=artist.id AND credit.role='primary'),
                (SELECT count(*) FROM track_artists credit
                  WHERE credit.artist_id=artist.id)
           FROM artists artist
          WHERE artist.normalized_name='alice cooper'",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read same-name Artist reuse");
    assert_eq!(artists.len(), 1, "one name yields exactly one Artist row");
    assert_eq!(artists[0].1, 2, "both Releases bill the reused Artist");
    assert_eq!(artists[0].2, 2, "both Track credits use the reused Artist");

    let aggregate: (i64, i64) = sqlx::query_as(
        "SELECT release_count, billed_track_count
           FROM artist_catalog_aggregates WHERE artist_id=$1",
    )
    .bind(artists[0].0)
    .fetch_one(&db.pool)
    .await
    .expect("read reused Artist aggregate");
    assert_eq!(
        aggregate,
        (2, 2),
        "the reused Artist aggregate covers every Release"
    );

    assert!(
        catalog
            .repeated_artist_credit_merge_candidates(&db.pool, 64)
            .await
            .expect("scan for leftover same-name candidates")
            .is_empty(),
        "reuse leaves nothing to reconcile"
    );
    assert!(
        catalog
            .name_only_artist_match_proposal_candidates(&db.pool, 64)
            .await
            .expect("scan for leftover name-only proposals")
            .is_empty(),
        "reuse leaves no duplicate to propose against"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM artist_merge_events")
            .fetch_one(&db.pool)
            .await
            .expect("count merge events"),
        0,
        "reuse creates no merge history because nothing was merged"
    );
    db.drop().await;
}

#[tokio::test]
async fn legacy_same_name_artist_rows_are_reconciled_audited_and_reversible() {
    // Reconciliation still has to clean catalogues written before ADR-0189. A
    // temporary disambiguation hint suppresses reuse so the fixture reproduces
    // the legacy duplicate rows, then the hint is cleared to leave exactly the
    // identity facts the reconciler is allowed to converge.
    let db = TestDb::create("catalog_legacy_duplicate_artist_name").await;
    let first = seed_published_release(
        &db,
        96,
        CatalogFixturePresentation {
            release_title: "Alice Cooper Goes to Hell",
            release_artist: "Alice Cooper",
            track_title: "Go to Hell",
            track_artist: "Alice Cooper",
            release_date: "1976-06-25",
        },
    )
    .await;
    let second = seed_published_release(
        &db,
        97,
        CatalogFixturePresentation {
            release_title: "Brutal Planet",
            release_artist: "Alice Cooper",
            track_title: "Brutal Planet",
            track_artist: "Alice Cooper",
            release_date: "2000-06-06",
        },
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim legacy duplicate fixtures")
            .len(),
        2
    );
    catalog
        .import_manifest_v3(
            &db.pool,
            first.work_item_id,
            &worker,
            &CatalogImportRequest::new("cross-release-name-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import first legacy fixture");
    sqlx::query(
        "UPDATE artists SET disambiguation='legacy duplicate' WHERE normalized_name='alice cooper'",
    )
    .execute(&db.pool)
    .await
    .expect("force a legacy second row");
    catalog
        .import_manifest_v3(
            &db.pool,
            second.work_item_id,
            &worker,
            &CatalogImportRequest::new("cross-release-name-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import second legacy fixture");
    sqlx::query("UPDATE artists SET disambiguation=NULL WHERE normalized_name='alice cooper'")
        .execute(&db.pool)
        .await
        .expect("clear the temporary hint");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='alice cooper' AND status='provisional'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count legacy duplicates"),
        2
    );

    let candidates = catalog
        .repeated_artist_credit_merge_candidates(&db.pool, 64)
        .await
        .expect("scan repeated Artist credits");
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.normalized_name, "alice cooper");
    let target_release_credit: (Uuid, Uuid) = sqlx::query_as(
        "SELECT credit.id, credit.artist_id
           FROM release_artists credit
          WHERE credit.release_id=$1",
    )
    .bind(second.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read second Release credit");
    catalog
        .reconcile_artist(
            &db.pool,
            &ArtistMerge {
                id: ArtistMergeId::new(),
                source_artist_id: ArtistId::from_uuid(candidate.source_artist_id),
                source_public_api_id: ArtistPublicId::from_uuid(candidate.source_public_api_id),
                target_artist_id: ArtistId::from_uuid(candidate.target_artist_id),
                actor: "catalog-reconciler".to_owned(),
                reason: "repeated exact Artist credit".to_owned(),
                occurred_at: time::OffsetDateTime::now_utc(),
            },
            "repeated-credit-v1",
            &ArtistReconciliationEvidence::RepeatedCredit {
                normalized_name: candidate.normalized_name.clone(),
            },
        )
        .await
        .expect("reconcile repeated Artist credit");

    let statuses: (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status='provisional'),
                count(*) FILTER (WHERE status='merged')
           FROM artists WHERE normalized_name='alice cooper'",
    )
    .fetch_one(&db.pool)
    .await
    .expect("read same-name Artist statuses");
    assert_eq!(statuses, (1, 1), "the duplicate is tombstoned, not deleted");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artist_merge_events
              WHERE moved_relationships #>> '{reconciliation,evidence,predicate}'
                    = 'repeated_credit'
                AND moved_relationships #>> '{reconciliation,evidence,scope}'
                    = 'different_releases'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count merge events"),
        1,
        "the contextual merge remains audited"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artist_merge_events WHERE source_artist_id=$1",
        )
        .bind(candidate.source_artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("count merge events for the tombstoned Artist"),
        1,
        "the audit names the merged Artist"
    );

    // The merge is reversible through the existing split path.
    let restored_artist_id = ArtistId::new();
    let restored_public_id = ArtistPublicId::from_uuid(Uuid::now_v7());
    let restored = melodee_domain::Artist::new(melodee_domain::NewArtist {
        id: restored_artist_id,
        public_api_id: restored_public_id,
        canonical_name: "Alice Cooper".to_owned(),
        normalized_name: "alice cooper".to_owned(),
        sort_name: "Alice Cooper".to_owned(),
        artist_type: melodee_domain::ArtistType::Unknown,
        disambiguation: None,
        active_dates: None,
        status: melodee_domain::ArtistStatus::Provisional,
        created_at: time::OffsetDateTime::now_utc(),
    })
    .expect("valid restored Artist");
    let track_credit: Uuid = sqlx::query_scalar(
        "SELECT credit.id FROM track_artists credit
           JOIN tracks track ON track.id=credit.track_id
          WHERE track.release_id=$1",
    )
    .bind(second.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read second Release Track credit");
    catalog
        .split_artist(
            &db.pool,
            &melodee_domain::ArtistSplit {
                id: melodee_domain::ArtistSplitId::new(),
                source_artist_id: ArtistId::from_uuid(candidate.target_artist_id),
                new_artist_id: restored_artist_id,
                assignments: vec![
                    melodee_domain::SplitAssignment::ReleaseArtist(
                        melodee_domain::ReleaseArtistId::from_uuid(target_release_credit.0),
                    ),
                    melodee_domain::SplitAssignment::TrackArtist(
                        melodee_domain::TrackArtistId::from_uuid(track_credit),
                    ),
                ],
                actor: "catalog-admin".to_owned(),
                reason: "restore the legacy duplicate".to_owned(),
                occurred_at: time::OffsetDateTime::now_utc(),
            },
            &restored,
            "manual-identity-correction-v2",
        )
        .await
        .expect("split the reconciled credit back out");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='alice cooper' AND status<>'merged'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count active same-name Artists after the split"),
        2,
        "the split restores a second active Artist"
    );
    db.drop().await;
}

#[tokio::test]
async fn conflicting_provider_ids_preserve_same_name_identities_and_allow_review() {
    let db = TestDb::create("catalog_cross_release_name_conflict").await;
    let first_artist_id = "aa1b1c1d-0000-7000-8000-000000000041";
    let second_artist_id = "aa1b1c1d-0000-7000-8000-000000000042";
    let fixtures = [
        seed_published_release_with_musicbrainz_ids(
            &db,
            98,
            CatalogFixturePresentation {
                release_title: "First Same-Name Release",
                release_artist: "Same Name",
                track_title: "First Track",
                track_artist: "Same Name",
                release_date: "2025-01-01",
            },
            serde_json::json!({
                "artist": first_artist_id,
                "release_artist": first_artist_id
            }),
        )
        .await,
        seed_published_release_with_musicbrainz_ids(
            &db,
            99,
            CatalogFixturePresentation {
                release_title: "Second Same-Name Release",
                release_artist: "Same Name",
                track_title: "Second Track",
                track_artist: "Same Name",
                release_date: "2026-01-01",
            },
            serde_json::json!({
                "artist": second_artist_id,
                "release_artist": second_artist_id
            }),
        )
        .await,
    ];
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim provider-conflict fixtures")
            .len(),
        2
    );
    for fixture in &fixtures {
        catalog
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("cross-release-name-v1", "catalog-test"),
                now,
            )
            .await
            .expect("import provider-conflict fixture");
    }

    assert_eq!(
        catalog
            .strong_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan strong candidates")
            .len(),
        0,
        "different IDs from the same provider block automatic convergence"
    );
    assert!(
        catalog
            .repeated_primary_release_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan contextual candidates with provider conflict")
            .is_empty(),
        "provider evidence blocks weaker primary-credit convergence"
    );
    assert!(
        !catalog
            .name_only_artist_match_proposal_candidates(&db.pool, 64)
            .await
            .expect("scan conflicted names for review")
            .is_empty(),
        "conflicting same-name identities remain available for human review"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='same name' AND status='provisional'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count preserved same-name identities"),
        2
    );
    db.drop().await;
}

#[tokio::test]
async fn conflicting_active_dates_block_strong_provider_id_reconciliation() {
    let db = TestDb::create("catalog_strong_artist_date_conflict").await;
    let shared_artist_id = "aa1b1c1d-0000-7000-8000-000000000061";
    let fixtures = [
        seed_published_release_with_musicbrainz_ids(
            &db,
            100,
            CatalogFixturePresentation {
                release_title: "British Same-Name Release",
                release_artist: "Nirvana",
                track_title: "First Track",
                track_artist: "Nirvana",
                release_date: "1968-01-01",
            },
            serde_json::json!({
                "artist": shared_artist_id,
                "release_artist": shared_artist_id
            }),
        )
        .await,
        seed_published_release_with_musicbrainz_ids(
            &db,
            101,
            CatalogFixturePresentation {
                release_title: "American Same-Name Release",
                release_artist: "Nirvana",
                track_title: "Second Track",
                track_artist: "Nirvana",
                release_date: "1991-01-01",
            },
            serde_json::json!({
                "artist": shared_artist_id,
                "release_artist": shared_artist_id
            }),
        )
        .await,
    ];
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim active-date fixtures")
            .len(),
        2
    );
    // Both fixtures carry the same trusted provider id, so a pre-existing row is
    // reused by identity (ADR-0189) unless a disambiguation hint keeps the pair
    // apart; the fixture needs the two legacy rows to exercise date gating.
    catalog
        .import_manifest_v3(
            &db.pool,
            fixtures[0].work_item_id,
            &worker,
            &CatalogImportRequest::new("strong-id-conflict-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import first active-date fixture");
    sqlx::query(
        "UPDATE artists SET disambiguation='legacy duplicate'
          WHERE normalized_name='nirvana'",
    )
    .execute(&db.pool)
    .await
    .expect("force a legacy second row");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixtures[1].work_item_id,
            &worker,
            &CatalogImportRequest::new("strong-id-conflict-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import second active-date fixture");
    sqlx::query("UPDATE artists SET disambiguation=NULL WHERE normalized_name='nirvana'")
        .execute(&db.pool)
        .await
        .expect("clear the temporary hint");
    let artist_ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT artist_id FROM release_artists
          WHERE release_id=ANY($1) ORDER BY artist_id",
    )
    .bind(
        fixtures
            .iter()
            .map(|fixture| fixture.release_id.into_uuid())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&db.pool)
    .await
    .expect("read same-name Artist identities");
    assert_eq!(artist_ids.len(), 2);
    assert_eq!(
        catalog
            .strong_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan matching trusted IDs")
            .len(),
        1
    );

    sqlx::query("UPDATE artists SET begin_date='1966-01-01', end_date='1971-12-31' WHERE id=$1")
        .bind(artist_ids[0])
        .execute(&db.pool)
        .await
        .expect("set first Artist dates");
    sqlx::query("UPDATE artists SET begin_date='1987-01-01' WHERE id=$1")
        .bind(artist_ids[1])
        .execute(&db.pool)
        .await
        .expect("set second Artist dates");
    assert!(
        catalog
            .strong_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan conflicting active dates")
            .is_empty(),
        "disjoint active ranges block even an otherwise strong provider-ID merge"
    );
    assert!(
        catalog
            .repeated_primary_release_artist_merge_candidates(&db.pool, 64)
            .await
            .expect("scan contextual candidates with date conflict")
            .is_empty(),
        "active-date evidence also blocks primary-credit convergence"
    );
    db.drop().await;
}

#[tokio::test]
async fn disambiguated_same_name_primary_artists_remain_distinct() {
    let db = TestDb::create("catalog_primary_artist_disambiguation").await;
    let fixtures = [
        seed_published_release(
            &db,
            110,
            CatalogFixturePresentation {
                release_title: "British Psychedelic Release",
                release_artist: "Nirvana",
                track_title: "First Track",
                track_artist: "Nirvana",
                release_date: "1968-01-01",
            },
        )
        .await,
        seed_published_release(
            &db,
            111,
            CatalogFixturePresentation {
                release_title: "American Grunge Release",
                release_artist: "Nirvana",
                track_title: "Second Track",
                track_artist: "Nirvana",
                release_date: "1991-01-01",
            },
        )
        .await,
    ];
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim homonym fixtures")
            .len(),
        2
    );
    // A disambiguation hint present before the second import suppresses name
    // reuse (ADR-0189), which is how two genuinely different same-name Artists
    // stay separate.
    catalog
        .import_manifest_v3(
            &db.pool,
            fixtures[0].work_item_id,
            &worker,
            &CatalogImportRequest::new("homonym-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import first homonym fixture");
    sqlx::query(
        "UPDATE artists SET disambiguation='UK psychedelic group'
          WHERE normalized_name='nirvana'",
    )
    .execute(&db.pool)
    .await
    .expect("disambiguate the first homonym");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixtures[1].work_item_id,
            &worker,
            &CatalogImportRequest::new("homonym-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import second homonym fixture");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='nirvana' AND status='provisional'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count disambiguated Artists after import"),
        2,
        "a disambiguation hint refuses name reuse at import"
    );
    let updated = sqlx::query(
        "UPDATE artists artist
            SET disambiguation=CASE release.title
                WHEN 'British Psychedelic Release' THEN 'UK psychedelic group'
                ELSE 'US grunge band'
            END
           FROM release_artists credit
           JOIN releases release ON release.id=credit.release_id
          WHERE credit.artist_id=artist.id
            AND credit.role='primary'
            AND artist.normalized_name='nirvana'",
    )
    .execute(&db.pool)
    .await
    .expect("add homonym disambiguation")
    .rows_affected();
    assert_eq!(updated, 2);

    assert!(
        catalog
            .repeated_artist_credit_merge_candidates(&db.pool, 64)
            .await
            .expect("scan disambiguated homonyms")
            .is_empty(),
        "explicit disambiguation blocks contextual convergence"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM artists
              WHERE normalized_name='nirvana' AND status='provisional'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("count distinct homonyms"),
        2
    );
    db.drop().await;
}

#[tokio::test]
async fn negative_assertions_and_locked_decisions_suppress_name_only_proposals() {
    let db = TestDb::create("catalog_cross_release_manual_conflict").await;
    let fixtures = [
        seed_published_release(
            &db,
            105,
            CatalogFixturePresentation {
                release_title: "First Manual Context",
                release_artist: "Shared Billing",
                track_title: "First Track",
                track_artist: "Shared Billing",
                release_date: "2025-01-01",
            },
        )
        .await,
        seed_published_release(
            &db,
            106,
            CatalogFixturePresentation {
                release_title: "Second Manual Context",
                release_artist: "Shared Billing",
                track_title: "Second Track",
                track_artist: "Shared Billing",
                release_date: "2026-01-01",
            },
        )
        .await,
    ];
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &worker, &test_config(), now)
            .await
            .expect("claim manual-conflict fixtures")
            .len(),
        2
    );
    // Two legacy same-name rows are the subject of this fixture, so a temporary
    // disambiguation hint suppresses import-time reuse (ADR-0189) and is then
    // cleared to leave the ambiguous provisional pair reconciliation sees in a
    // pre-existing catalogue.
    catalog
        .import_manifest_v3(
            &db.pool,
            fixtures[0].work_item_id,
            &worker,
            &CatalogImportRequest::new("cross-release-name-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import first manual-conflict fixture");
    sqlx::query(
        "UPDATE artists SET disambiguation='legacy duplicate'
          WHERE normalized_name='shared billing'",
    )
    .execute(&db.pool)
    .await
    .expect("force a legacy second row");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixtures[1].work_item_id,
            &worker,
            &CatalogImportRequest::new("cross-release-name-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import second manual-conflict fixture");
    sqlx::query("UPDATE artists SET disambiguation=NULL WHERE normalized_name='shared billing'")
        .execute(&db.pool)
        .await
        .expect("clear the temporary hint");
    let candidate = catalog
        .name_only_artist_match_proposal_candidates(&db.pool, 64)
        .await
        .expect("scan initial name-only candidate")
        .into_iter()
        .next()
        .expect("same provisional billing is initially eligible");
    assert_eq!(
        catalog
            .repeated_artist_credit_merge_candidates(&db.pool, 64)
            .await
            .expect("scan initial contextual candidate")
            .len(),
        1
    );
    let source_observation = candidate.observed_artist_id;
    let assertion_id = ArtistNegativeAssertionId::new();
    catalog
        .record_artist_negative_assertion(
            &db.pool,
            &ArtistNegativeAssertion {
                id: assertion_id,
                observed_artist_id: ObservedArtistId::from_uuid(source_observation),
                rejected_artist_id: ArtistId::from_uuid(candidate.candidate_artist_id),
                authority: DecisionAuthority::Administrator {
                    actor: "catalog-test".to_owned(),
                },
                locked: true,
                reason: "same display name is not the same Artist".to_owned(),
                decided_at: now + Duration::seconds(1),
            },
        )
        .await
        .expect("record local do-not-match assertion");
    assert!(
        catalog
            .name_only_artist_match_proposal_candidates(&db.pool, 64)
            .await
            .expect("scan negative-assertion candidates")
            .iter()
            .all(|other| other.observed_artist_id != source_observation
                || other.candidate_artist_id != candidate.candidate_artist_id)
    );
    assert!(
        catalog
            .repeated_artist_credit_merge_candidates(&db.pool, 64)
            .await
            .expect("scan contextual candidates after rejection")
            .is_empty()
    );

    catalog
        .revoke_artist_negative_assertion(
            &db.pool,
            assertion_id.into_uuid(),
            &DecisionAuthority::Administrator {
                actor: "catalog-test".to_owned(),
            },
            "resume evidence evaluation",
            now + Duration::seconds(2),
        )
        .await
        .expect("revoke local do-not-match assertion");
    assert!(
        catalog
            .name_only_artist_match_proposal_candidates(&db.pool, 64)
            .await
            .expect("scan after negative revocation")
            .iter()
            .any(|other| other.observed_artist_id == source_observation
                && other.candidate_artist_id == candidate.candidate_artist_id)
    );
    assert_eq!(
        catalog
            .repeated_artist_credit_merge_candidates(&db.pool, 64)
            .await
            .expect("scan contextual candidates after revocation")
            .len(),
        1
    );

    let locked = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: ArtistMatchDecisionId::new(),
            observed_artist_id: ObservedArtistId::from_uuid(source_observation),
            artist_id: ArtistId::from_uuid(candidate.current_artist_id),
            outcome: ArtistMatchOutcome::Confirmed,
            authority: DecisionAuthority::Administrator {
                actor: "catalog-test".to_owned(),
            },
            locked: true,
            reason: "administrator locked this provisional identity".to_owned(),
            decided_at: now + Duration::seconds(3),
        },
        &[],
    )
    .expect("construct locked administrator decision");
    catalog
        .record_artist_match_decision(
            &db.pool,
            &locked,
            Confidence::new(10_000).expect("valid confidence"),
            serde_json::json!({"fixture": "cross-release-lock"}),
        )
        .await
        .expect("record locked administrator decision");
    assert!(
        catalog
            .name_only_artist_match_proposal_candidates(&db.pool, 64)
            .await
            .expect("scan locked-decision candidates")
            .iter()
            .all(|other| other.observed_artist_id != source_observation)
    );
    assert!(
        catalog
            .repeated_artist_credit_merge_candidates(&db.pool, 64)
            .await
            .expect("scan contextual candidates after manual lock")
            .is_empty()
    );
    db.drop().await;
}

#[tokio::test]
async fn catalog_import_persists_all_embedded_musicbrainz_families_and_enrichment_work() {
    let db = TestDb::create("catalog_embedded_evidence").await;
    let fixture = seed_published_release_with_musicbrainz_ids(
        &db,
        91,
        CatalogFixturePresentation {
            release_title: "Embedded Evidence",
            release_artist: "Evidence Artist",
            track_title: "Evidence Track",
            track_artist: "Evidence Artist",
            release_date: "2009-08-07",
        },
        serde_json::json!({
            "recording": "aa1b1c1d-0000-7000-8000-000000000001",
            "release": "aa1b1c1d-0000-7000-8000-000000000002",
            "release_group": "aa1b1c1d-0000-7000-8000-000000000003",
            "artist": "aa1b1c1d-0000-7000-8000-000000000004",
            "release_artist": "aa1b1c1d-0000-7000-8000-000000000005",
            "work": "not-a-uuid-but-retained"
        }),
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim catalog import");
    catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("embedded-evidence-v1", "catalog-test"),
            now,
        )
        .await
        .expect("import embedded evidence");

    let external_ids: Vec<(String, String, i32)> = sqlx::query_as(
        "SELECT provider, external_id, confidence
         FROM artist_external_identities ORDER BY external_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read Artist external evidence");
    assert_eq!(external_ids.len(), 2);
    assert!(
        external_ids
            .iter()
            .all(|(_, id, confidence)| { id.starts_with("aa1b1c1d-") && *confidence == 9500 })
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM release_external_identities
             WHERE release_id=$1 AND provider='musicbrainz'",
        )
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read Release external evidence"),
        2
    );
    // Embedded-identifier evidence only: the import also records durable
    // acoustic-fingerprint evidence (PRD-FP-001), which carries no `family`.
    let audio_evidence: Vec<(String, i32)> = sqlx::query_as(
        "SELECT payload->>'family', confidence FROM audio_identity_evidence
         WHERE track_id=(SELECT id FROM tracks WHERE release_id=$1)
           AND evidence_kind='external_identifier'
         ORDER BY payload->>'family'",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read AudioIdentity evidence");
    assert_eq!(audio_evidence.len(), 6);

    // PRD-FP-001: the Chromaprint observation normalization already computed is
    // promoted to durable identity evidence (digest only; the raw fingerprint
    // stays in the sealed manifest).
    let acoustic: (i32, Option<String>) = sqlx::query_as(
        "SELECT confidence, payload->>'fingerprint_digest' FROM audio_identity_evidence
         WHERE track_id=(SELECT id FROM tracks WHERE release_id=$1)
           AND evidence_kind='acoustic_fingerprint'",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("read acoustic identity evidence");
    assert_eq!(acoustic.0, 9000);
    assert_eq!(
        acoustic.1.as_deref().map(str::len),
        Some(64),
        "the durable comparison key is a BLAKE3 digest"
    );
    assert!(
        audio_evidence
            .iter()
            .any(|(family, confidence)| family == "work" && *confidence == 1000)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM audio_identity_match_decisions
             WHERE outcome='proposed'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("read AudioIdentity proposals"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_enrichment_tasks")
            .fetch_one(&db.pool)
            .await
            .expect("read scheduled enrichment"),
        36
    );

    let genre_terms: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT surface, display_value, match_value, source_order
           FROM catalog_genre_terms ORDER BY surface",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read projected genre terms");
    assert_eq!(
        genre_terms,
        vec![
            (
                "native".to_string(),
                "Fixture".to_string(),
                "Fixture".to_string(),
                1,
            ),
            (
                "opensubsonic".to_string(),
                "Fixture".to_string(),
                "Fixture".to_string(),
                100_001,
            ),
        ]
    );
    let genre_aggregates: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT surface, song_count, release_count
           FROM catalog_genre_aggregates ORDER BY surface",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read projected genre aggregates");
    assert_eq!(
        genre_aggregates,
        vec![
            ("native".to_string(), 1, 1),
            ("opensubsonic".to_string(), 1, 1),
        ]
    );

    sqlx::query("DELETE FROM catalog_genre_terms WHERE surface='native'")
        .execute(&db.pool)
        .await
        .expect("delete projected native term");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_genre_aggregates WHERE surface='native'",
        )
        .fetch_one(&db.pool)
        .await
        .expect("read deleted aggregate"),
        0,
        "statement-level delete maintenance removes empty aggregate keys"
    );
    db.drop().await;
}

#[tokio::test]
async fn published_v3_fixture_exposes_complete_catalog_handoff_without_media_files() {
    let db = TestDb::create("catalog_v3_handoff").await;
    let fixture = seed_published_release(
        &db,
        1,
        CatalogFixturePresentation {
            release_title: "Catalog Handoff",
            release_artist: "Fixture Artist",
            track_title: "No Media Reread",
            track_artist: "Fixture Artist",
            release_date: "2001-02-03",
        },
    )
    .await;

    let store = WorkItemStore;
    let item = store
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read published item");
    let stored_manifest = store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read durable v3 manifest")
        .expect("manifest exists");
    let publication = store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read durable publication")
        .expect("publication exists");

    assert_eq!(item.state, IngestionState::Published);
    assert_eq!(
        stored_manifest.schema_version,
        i32::from(PUBLICATION_MANIFEST_VERSION)
    );
    let decoded_manifest: melodee_domain::ReleaseManifest =
        serde_json::from_value(stored_manifest.payload).expect("decode durable v3 manifest");
    assert_eq!(decoded_manifest, fixture.manifest);
    assert_eq!(publication.release_id, fixture.release_id);
    assert_eq!(publication.plan_hash, fixture.plan_hash);
    assert_eq!(
        publication.sealed_manifest_hash.as_deref(),
        Some(fixture.sealed_manifest_hash.as_str())
    );
    assert!(
        !std::path::Path::new(&fixture.source_path).exists(),
        "fixture deliberately provides no readable inbound media"
    );
    assert!(
        !std::path::Path::new(&fixture.canonical_asset_path).exists(),
        "fixture deliberately provides no readable canonical media"
    );

    db.drop().await;
}

#[tokio::test]
async fn import_adopts_reserved_release_id_is_idempotent_and_never_rewrites_v3() {
    let db = TestDb::create("catalog_import_exact_once").await;
    let fixture = seed_published_release(
        &db,
        2,
        CatalogFixturePresentation {
            release_title: "Reserved Identity",
            release_artist: "Same Raw Name",
            track_title: "Manifest Only",
            track_artist: "Same Raw Name",
            release_date: "2002-03-04",
        },
    )
    .await;
    let work_store = WorkItemStore;
    let before_manifest = work_store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-import manifest")
        .expect("pre-import manifest exists");
    let before_publication = work_store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read pre-import publication")
        .expect("pre-import publication exists");

    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let catalog_now = time::OffsetDateTime::now_utc();
    let claimed = catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), catalog_now)
        .await
        .expect("claim catalog import");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, fixture.work_item_id);
    assert_eq!(claimed[0].state, IngestionState::Cataloging);

    let request = CatalogImportRequest::new("phase4.fixture-resolver.v1", "catalog-test");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &request,
            catalog_now,
        )
        .await
        .expect("import manifest v3");
    assert!(imported.inserted);
    assert_eq!(imported.release_id, fixture.release_id);
    assert_eq!(imported.track_count, 1);
    assert_eq!(imported.media_asset_count, 1);
    assert_eq!(
        imported.artist_count, 1,
        "the sole matching Track observation reuses the Release Artist identity"
    );
    assert!(imported.set_based_mutation_statement_count > 0);

    let cataloged = work_store
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read cataloged item");
    assert_eq!(cataloged.state, IngestionState::Cataloged);
    assert!(cataloged.lease_owner.is_none());
    let counts_after_first = catalog.counts(&db.pool).await.expect("catalog counts");
    assert_eq!(counts_after_first.releases, 1);
    assert_eq!(counts_after_first.tracks, 1);
    assert_eq!(counts_after_first.artists, 1);
    let identity_rows: Vec<(uuid::Uuid, uuid::Uuid, String)> = sqlx::query_as(
        "SELECT decision.artist_id, observed.id, observed.scope
           FROM observed_artists observed
           JOIN artist_match_current current ON current.observed_artist_id = observed.id
           JOIN artist_match_decisions decision ON decision.id = current.decision_id
          ORDER BY observed.scope",
    )
    .fetch_all(&db.pool)
    .await
    .expect("read resolved Artist observations");
    assert_eq!(identity_rows.len(), 2, "raw observations remain distinct");
    assert_eq!(identity_rows[0].0, identity_rows[1].0);
    assert_ne!(identity_rows[0].1, identity_rows[1].1);
    assert_eq!(counts_after_first.media_assets, 1);
    assert_eq!(counts_after_first.imports, 1);

    let stored_release: (uuid::Uuid, uuid::Uuid) =
        sqlx::query_as("SELECT id, public_api_id FROM releases WHERE id = $1")
            .bind(fixture.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read imported Release");
    assert_eq!(stored_release.0, fixture.release_id.into_uuid());
    assert_eq!(
        stored_release.1,
        fixture.release_id.into_uuid(),
        "new public ID starts equal but remains a separate namespace"
    );
    let stored_asset: (String, String, i64) =
        sqlx::query_as("SELECT canonical_relative_path, byte_hash, byte_size FROM media_assets")
            .fetch_one(&db.pool)
            .await
            .expect("read imported canonical asset");
    assert_eq!(
        stored_asset.0,
        format!(
            "{}/{}",
            fixture.plan.target_relative_path, fixture.canonical_asset_path
        )
    );
    assert_eq!(stored_asset.1, fixture.normalized_byte_hash);
    assert_eq!(stored_asset.2, 42);

    let retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &request,
            catalog_now,
        )
        .await
        .expect("identical uncertain-commit retry");
    assert!(!retry.inserted);
    assert_eq!(retry.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("retry counts"),
        counts_after_first
    );

    let conflicting_retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("phase4.different-resolver.v2", "catalog-test"),
            catalog_now,
        )
        .await;
    assert!(conflicting_retry.is_err(), "different binding fails closed");
    assert_eq!(
        catalog.counts(&db.pool).await.expect("conflict counts"),
        counts_after_first
    );

    let after_manifest = work_store
        .get_manifest(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-import manifest")
        .expect("post-import manifest exists");
    let after_publication = work_store
        .get_publication(&db.pool, fixture.work_item_id)
        .await
        .expect("read post-import publication")
        .expect("post-import publication exists");
    assert_eq!(after_manifest.manifest_hash, before_manifest.manifest_hash);
    assert_eq!(after_manifest.payload, before_manifest.payload);
    assert_eq!(after_publication, before_publication);
    assert!(
        sqlx::query(
            "UPDATE release_work_item_manifests
             SET manifest_hash = $2 WHERE work_item_id = $1",
        )
        .bind(fixture.work_item_id)
        .bind("f".repeat(64))
        .execute(&db.pool)
        .await
        .is_err(),
        "sealed v3 is immutable at the database boundary"
    );

    db.drop().await;
}

#[tokio::test]
async fn catalog_lease_expiry_fences_stale_worker_and_direct_reclaim_converges() {
    let db = TestDb::create("catalog_import_lease_fencing").await;
    let fixture = seed_published_release(&db, 3, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let config = test_config();
    let stale_worker = WorkerId::new();
    let winning_worker = WorkerId::new();
    let catalog_now = time::OffsetDateTime::now_utc();

    let first_claim = catalog
        .claim_cataloging(&db.pool, &stale_worker, &config, catalog_now)
        .await
        .expect("initial catalog claim");
    assert_eq!(first_claim.len(), 1);
    assert!(
        catalog
            .claim_cataloging(&db.pool, &winning_worker, &config, catalog_now)
            .await
            .expect("blocked concurrent claim")
            .is_empty(),
        "active lease admits one authoritative worker"
    );

    let after_expiry = catalog_now + config.lease_duration + Duration::seconds(1);
    let reclaimed = catalog
        .claim_cataloging(&db.pool, &winning_worker, &config, after_expiry)
        .await
        .expect("directly reclaim abandoned Cataloging");
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].id, fixture.work_item_id);
    let request = CatalogImportRequest::new("phase4.fixture-resolver.v1", "catalog-test");

    let stale_result = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &stale_worker,
            &request,
            after_expiry,
        )
        .await;
    assert!(
        stale_result.is_err(),
        "stale lease is fenced before catalog writes"
    );
    let empty_counts = catalog.counts(&db.pool).await.expect("post-fence counts");
    assert_eq!(empty_counts.releases, 0);
    assert_eq!(empty_counts.tracks, 0);
    assert_eq!(empty_counts.imports, 0);

    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &winning_worker,
            &request,
            after_expiry,
        )
        .await
        .expect("winning retry catalogs atomically");
    assert!(imported.inserted);
    assert_eq!(imported.release_id, fixture.release_id);
    let final_counts = catalog.counts(&db.pool).await.expect("final counts");
    assert_eq!(final_counts.releases, 1);
    assert_eq!(final_counts.tracks, 1);
    assert_eq!(final_counts.imports, 1);
    let identical_restart_retry = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &winning_worker,
            &request,
            after_expiry,
        )
        .await
        .expect("completed restart retry finds authoritative result");
    assert!(!identical_restart_retry.inserted);
    assert_eq!(identical_restart_retry.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("retry counts"),
        final_counts
    );

    db.drop().await;
}

#[tokio::test]
async fn catalog_renewal_cancellation_release_and_retry_are_fenced_and_reclaimable() {
    let db = TestDb::create("catalog_lease_renew_cancel").await;
    let fixture = seed_published_release(&db, 4, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let owner = WorkerId::new();
    let retry_worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let mut config = test_config();
    config.lease_duration = Duration::seconds(2);
    let claimed = catalog
        .claim_cataloging(&db.pool, &owner, &config, now)
        .await
        .expect("claim short catalog lease");
    assert_eq!(claimed.len(), 1);
    let original_expiry = claimed[0].lease_expires_at.expect("initial lease expiry");

    let renewed = catalog
        .renew_lease(
            &db.pool,
            fixture.work_item_id,
            &owner,
            Duration::seconds(10),
            now + Duration::seconds(1),
        )
        .await
        .expect("renew active catalog lease");
    assert_eq!(renewed.state, IngestionState::Cataloging);
    assert_eq!(renewed.lease_owner.as_deref(), Some(owner.as_str()));
    assert!(renewed.lease_expires_at.expect("renewed expiry") > original_expiry);
    assert!(
        catalog
            .claim_cataloging(&db.pool, &retry_worker, &config, now + Duration::seconds(3))
            .await
            .expect("renewal prevents theft")
            .is_empty()
    );

    catalog
        .release_lease(
            &db.pool,
            fixture.work_item_id,
            &owner,
            now + Duration::seconds(3),
        )
        .await
        .expect("cancellation releases catalog lease");
    let released = WorkItemStore
        .get(&db.pool, fixture.work_item_id)
        .await
        .expect("read released Cataloging work");
    assert_eq!(released.state, IngestionState::Cataloging);
    assert!(released.lease_owner.is_none());
    assert!(released.lease_expires_at.is_none());
    assert_eq!(
        catalog
            .counts(&db.pool)
            .await
            .expect("no partial rows")
            .imports,
        0
    );

    let reclaimed = catalog
        .claim_cataloging(&db.pool, &retry_worker, &config, now + Duration::seconds(3))
        .await
        .expect("reclaim cancellation-released work");
    assert_eq!(reclaimed.len(), 1);
    let request = CatalogImportRequest::new("phase4.cancel-retry.v1", "catalog-test");
    let result = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            now + Duration::seconds(3),
        )
        .await
        .expect("retry after cancellation catalogs atomically");
    assert!(result.inserted);
    assert_eq!(result.release_id, fixture.release_id);
    assert_eq!(
        catalog.counts(&db.pool).await.expect("one import").imports,
        1
    );

    db.drop().await;
}

#[tokio::test]
async fn retryable_catalog_failure_records_backoff_then_direct_reclaim_converges() {
    let db = TestDb::create("catalog_failure_backoff").await;
    let fixture = seed_published_release(&db, 5, CatalogFixturePresentation::default()).await;
    let catalog = CatalogStore;
    let first_worker = WorkerId::new();
    let retry_worker = WorkerId::new();
    let now = time::OffsetDateTime::now_utc();
    let config = test_config();
    assert_eq!(
        catalog
            .claim_cataloging(&db.pool, &first_worker, &config, now)
            .await
            .expect("claim catalog work")
            .len(),
        1
    );
    let policy = RetryPolicy {
        base_delay: Duration::seconds(5),
        max_delay: Duration::seconds(5),
        max_attempts: 3,
    };
    let failed = catalog
        .record_failure(
            &db.pool,
            fixture.work_item_id,
            &first_worker,
            WorkErrorCode::new("catalog.synthetic_restart"),
            "simulated process termination before import",
            FailureClass::Retryable,
            &policy,
            now,
        )
        .await
        .expect("record retryable catalog failure");
    assert_eq!(failed.state, IngestionState::Cataloging);
    assert_eq!(failed.attempt_count, 1);
    assert!(failed.lease_owner.is_none());
    let retry_at = failed.next_attempt_at.expect("durable retry deadline");
    assert!(
        catalog
            .claim_cataloging(
                &db.pool,
                &retry_worker,
                &config,
                retry_at - Duration::milliseconds(1),
            )
            .await
            .expect("backoff blocks early reclaim")
            .is_empty()
    );
    assert_eq!(
        catalog
            .counts(&db.pool)
            .await
            .expect("no partial import")
            .imports,
        0
    );

    let reclaimed = catalog
        .claim_cataloging(&db.pool, &retry_worker, &config, retry_at)
        .await
        .expect("direct reclaim at retry deadline");
    assert_eq!(reclaimed.len(), 1);
    let request = CatalogImportRequest::new("phase4.failure-retry.v1", "catalog-test");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            retry_at,
        )
        .await
        .expect("retry catalogs atomically");
    assert!(imported.inserted);
    let identical = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &retry_worker,
            &request,
            retry_at,
        )
        .await
        .expect("identical completed retry");
    assert!(!identical.inserted);
    assert_eq!(identical.catalog_import_id, imported.catalog_import_id);
    let event_counts: (i64, i64) = sqlx::query_as(
        "SELECT
            count(*) FILTER (WHERE event_type='failed'),
            count(*) FILTER (WHERE event_type='retry_scheduled')
         FROM release_work_item_events WHERE work_item_id=$1",
    )
    .bind(fixture.work_item_id)
    .fetch_one(&db.pool)
    .await
    .expect("query durable retry evidence");
    assert_eq!(event_counts, (1, 1));
    assert_eq!(
        catalog.counts(&db.pool).await.expect("one import").imports,
        1
    );

    db.drop().await;
}

#[tokio::test]
async fn trusted_release_evidence_merge_and_split_preserve_tracks_and_audit() {
    let db = TestDb::create("catalog_release_merge_split").await;
    let evidence = serde_json::json!({
        "recording": "bb1b1c1d-0000-7000-8000-000000000001",
        "release": "bb1b1c1d-0000-7000-8000-000000000002",
        "release_group": "bb1b1c1d-0000-7000-8000-000000000003",
        "artist": "bb1b1c1d-0000-7000-8000-000000000004",
        "release_artist": "bb1b1c1d-0000-7000-8000-000000000005"
    });
    let first = seed_published_release_with_musicbrainz_ids(
        &db,
        20,
        CatalogFixturePresentation {
            release_title: "Merge Source",
            ..CatalogFixturePresentation::default()
        },
        evidence.clone(),
    )
    .await;
    let second = seed_published_release_with_musicbrainz_ids(
        &db,
        21,
        CatalogFixturePresentation {
            release_title: "Merge Target",
            ..CatalogFixturePresentation::default()
        },
        evidence,
    )
    .await;
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let claimed = catalog
        .claim_cataloging(
            &db.pool,
            &worker,
            &test_config(),
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("claim Release fixtures");
    assert_eq!(claimed.len(), 2);
    for fixture in [&first, &second] {
        let now = time::OffsetDateTime::now_utc();
        catalog
            .import_manifest_v3(
                &db.pool,
                fixture.work_item_id,
                &worker,
                &CatalogImportRequest::new("release-merge-test-v1", "catalog-test"),
                now,
            )
            .await
            .expect("import Release fixture");
    }

    let candidates = catalog
        .strong_release_merge_candidates(&db.pool, 64)
        .await
        .expect("scan trusted Release evidence");
    let candidate = candidates
        .into_iter()
        .find(|candidate| {
            candidate.source_release_id == first.release_id.into_uuid()
                && candidate.target_release_id == second.release_id.into_uuid()
        })
        .expect("duplicate Releases form a strong evidence candidate");
    let source_track: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1 ORDER BY ordinal LIMIT 1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read source Track before merge");
    let target_track_count_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(second.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count target Tracks before merge");

    let merge_event = catalog
        .merge_release(
            &db.pool,
            candidate.source_release_id,
            candidate.target_release_id,
            "catalog-test",
            "strong provider evidence test merge",
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("merge Release with durable event");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM releases WHERE id=$1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read merged source status"),
        "merged"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tracks WHERE release_id=$1")
            .bind(second.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("count target Tracks after merge"),
        target_track_count_before + 1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM release_merge_events WHERE id=$1")
            .bind(merge_event)
            .fetch_one(&db.pool)
            .await
            .expect("read merge event"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_projection_outbox
             WHERE entity_type='track' AND entity_id=$1 AND entity_revision > 1
               AND processed_at IS NULL",
        )
        .bind(source_track)
        .fetch_one(&db.pool)
        .await
        .expect("read Track projection event after merge"),
        1
    );

    catalog
        .split_release_merge(
            &db.pool,
            merge_event,
            "catalog-test",
            "reversible merge test split",
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("split Release merge");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM releases WHERE id=$1")
            .bind(first.release_id.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read restored source status"),
        "active"
    );
    assert_eq!(
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT release_id FROM tracks WHERE id=$1")
            .bind(source_track)
            .fetch_one(&db.pool)
            .await
            .expect("read restored Track owner"),
        first.release_id.into_uuid()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM release_split_events WHERE merge_event_id=$1",
        )
        .bind(merge_event)
        .fetch_one(&db.pool)
        .await
        .expect("read split event"),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM catalog_audit_events
             WHERE entity_id = ANY($1) AND event_type IN ('release_merged','release_merged_into','release_split')",
        )
        .bind(vec![first.release_id.into_uuid(), second.release_id.into_uuid()])
        .fetch_one(&db.pool)
        .await
        .expect("read merge and split audit events"),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT track_count FROM release_catalog_aggregates WHERE release_id=$1",
        )
        .bind(first.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read restored source aggregate"),
        1
    );
    db.drop().await;
}
