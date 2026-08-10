//! PostgreSQL identity-integrity proofs for ADR-0117.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use std::collections::BTreeSet;

use catalog_common::{
    CatalogFixturePresentation, legacy::ARTIST_PUBLIC_IDS, seed_published_release,
};
use common::{T0, TestDb, test_config};
use melodee_domain::{
    ArtistAlias, ArtistAliasId, ArtistAliasType, ArtistId, ArtistMatchDecision,
    ArtistMatchDecisionId, ArtistMatchEvidence, ArtistMatchEvidenceId, ArtistMatchOutcome,
    ArtistNegativeAssertion, ArtistNegativeAssertionId, Confidence, DecisionAuthority,
    EvidenceKind, EvidencePolarity, ExternalIdentity, ExternalIdentityId, ExternalIdentityOwner,
    NewArtistMatchDecision, ObservedArtistId, TrackId,
};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, CatalogStoreError, ProviderNegativeAssertion, WorkerId,
};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

async fn insert_artist(
    pool: &PgPool,
    id: Uuid,
    public_api_id: Uuid,
    name: &str,
    normalized_name: &str,
    artist_type: &str,
    disambiguation: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO artists (
            id, public_api_id, canonical_name, normalized_name, sort_name,
            artist_type, disambiguation, status, created_at, updated_at
         ) VALUES ($1,$2,$3,$4,$3,$5,$6,'verified',$7,$7)",
    )
    .bind(id)
    .bind(public_api_id)
    .bind(name)
    .bind(normalized_name)
    .bind(artist_type)
    .bind(disambiguation)
    .bind(T0)
    .execute(pool)
    .await
    .map(|_| ())
}

fn frozen_artist_public_id(slug: &str) -> Uuid {
    let value = ARTIST_PUBLIC_IDS
        .iter()
        .find_map(|(candidate_slug, value)| (*candidate_slug == slug).then_some(*value))
        .expect("known frozen Artist slug");
    Uuid::parse_str(value).expect("valid frozen Artist public UUID")
}

#[tokio::test]
async fn exact_same_names_coexist_with_distinct_internal_and_legacy_public_ids() {
    let db = TestDb::create("catalog_exact_same_artist_names").await;
    let first_id = Uuid::now_v7();
    let second_id = Uuid::now_v7();
    let first_public_id = frozen_artist_public_id("dune-plain");
    let second_public_id = frozen_artist_public_id("dune-diacritic");

    insert_artist(
        &db.pool,
        first_id,
        first_public_id,
        "Dune",
        "DUNE",
        "group",
        Some("United States fixture"),
    )
    .await
    .expect("insert first same-name Artist");
    insert_artist(
        &db.pool,
        second_id,
        second_public_id,
        "Dune",
        "DUNE",
        "group",
        Some("Denmark fixture"),
    )
    .await
    .expect("insert second same-name Artist");

    let rows = sqlx::query_as::<_, (Uuid, Uuid, String, String, Option<Uuid>)>(
        "SELECT id, public_api_id, canonical_name, disambiguation, merged_into_artist_id
         FROM artists WHERE canonical_name = 'Dune' AND normalized_name = 'DUNE'
         ORDER BY public_api_id",
    )
    .fetch_all(&db.pool)
    .await
    .expect("query exact same-name Artists");
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].0, rows[1].0, "internal identities remain distinct");
    assert_ne!(rows[0].1, rows[1].1, "public identities remain distinct");
    assert!(rows.iter().all(|row| row.4.is_none()));
    assert_eq!(
        rows.iter()
            .map(|row| row.1)
            .collect::<std::collections::BTreeSet<_>>(),
        [first_public_id, second_public_id].into_iter().collect()
    );

    let duplicate_public_id = insert_artist(
        &db.pool,
        Uuid::now_v7(),
        first_public_id,
        "Not Dune",
        "NOTDUNE",
        "group",
        None,
    )
    .await;
    assert!(
        duplicate_public_id.is_err(),
        "legacy public identity cannot be reassigned"
    );

    db.drop().await;
}

#[tokio::test]
async fn provider_scope_and_membership_integrity_fail_closed() {
    let db = TestDb::create("catalog_identity_constraints").await;
    let first_id = Uuid::now_v7();
    let second_id = Uuid::now_v7();
    insert_artist(
        &db.pool,
        first_id,
        first_id,
        "Colin Hay",
        "COLINHAY",
        "person",
        None,
    )
    .await
    .expect("insert person");
    insert_artist(
        &db.pool,
        second_id,
        second_id,
        "Men at Work",
        "MENATWORK",
        "group",
        None,
    )
    .await
    .expect("insert group");

    for artist_id in [first_id, second_id] {
        sqlx::query(
            "INSERT INTO artist_external_identities (
                id, artist_id, provider, external_id, provider_unique_scope,
                confidence, evidence_source, created_at
             ) VALUES ($1,$2,'fixture-provider','shared-local-value',false,7000,'fixture',$3)",
        )
        .bind(Uuid::now_v7())
        .bind(artist_id)
        .bind(T0)
        .execute(&db.pool)
        .await
        .expect("non-unique provider evidence may name multiple candidates");
    }

    sqlx::query(
        "INSERT INTO artist_external_identities (
            id, artist_id, provider, external_id, provider_unique_scope,
            confidence, evidence_source, created_at
         ) VALUES ($1,$2,'musicbrainz','fixed-provider-id',true,10000,'fixture',$3)",
    )
    .bind(Uuid::now_v7())
    .bind(first_id)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("insert provider-unique evidence");
    let duplicate_provider_identity = sqlx::query(
        "INSERT INTO artist_external_identities (
            id, artist_id, provider, external_id, provider_unique_scope,
            confidence, evidence_source, created_at
         ) VALUES ($1,$2,'musicbrainz','fixed-provider-id',true,10000,'fixture',$3)",
    )
    .bind(Uuid::now_v7())
    .bind(second_id)
    .bind(T0)
    .execute(&db.pool)
    .await;
    assert!(duplicate_provider_identity.is_err());

    let inclusive_single_day = sqlx::query(
        "INSERT INTO group_memberships (
            id, member_artist_id, group_artist_id, role, start_date, end_date,
            confidence, evidence_source, created_at
         ) VALUES ($1,$2,$3,'member','1983-01-01','1983-01-01',10000,'fixture',$4)",
    )
    .bind(Uuid::now_v7())
    .bind(first_id)
    .bind(second_id)
    .bind(T0)
    .execute(&db.pool)
    .await;
    assert!(
        inclusive_single_day.is_ok(),
        "equal inclusive membership boundaries are valid"
    );

    let reversed_dates = sqlx::query(
        "INSERT INTO group_memberships (
            id, member_artist_id, group_artist_id, role, start_date, end_date,
            confidence, evidence_source, created_at
         ) VALUES ($1,$2,$3,'member','1984-01-01','1983-01-01',10000,'fixture',$4)",
    )
    .bind(Uuid::now_v7())
    .bind(first_id)
    .bind(second_id)
    .bind(T0)
    .execute(&db.pool)
    .await;
    assert!(reversed_dates.is_err());

    let self_membership = sqlx::query(
        "INSERT INTO group_memberships (
            id, member_artist_id, group_artist_id, role,
            confidence, evidence_source, created_at
         ) VALUES ($1,$2,$2,'member',10000,'fixture',$3)",
    )
    .bind(Uuid::now_v7())
    .bind(first_id)
    .bind(T0)
    .execute(&db.pool)
    .await;
    assert!(self_membership.is_err());

    db.drop().await;
}

#[tokio::test]
async fn aliases_manual_decisions_and_negative_assertions_are_durable_and_scoped() {
    let db = TestDb::create("catalog_identity_evidence").await;
    let resolved_artist_id = Uuid::now_v7();
    let rejected_artist_id = Uuid::now_v7();
    insert_artist(
        &db.pool,
        resolved_artist_id,
        resolved_artist_id,
        "Richard Melville Hall",
        "RICHARDMELVILLEHALL",
        "person",
        None,
    )
    .await
    .expect("insert resolved Artist");
    insert_artist(
        &db.pool,
        rejected_artist_id,
        rejected_artist_id,
        "Moby Dick",
        "MOBYDICK",
        "other",
        Some("book title fixture, not the Artist"),
    )
    .await
    .expect("insert rejected candidate");
    let store = CatalogStore;
    store
        .add_artist_alias(
            &db.pool,
            &ArtistAlias {
                id: ArtistAliasId::new(),
                artist_id: ArtistId::from_uuid(resolved_artist_id),
                value: "Moby".to_owned(),
                normalized_value: "MOBY".to_owned(),
                alias_type: ArtistAliasType::Search,
                locale: None,
                evidence_source: "admin-fixture".to_owned(),
                confidence: Confidence::new(10_000).expect("valid confidence"),
                locked: true,
            },
            T0,
        )
        .await
        .expect("persist same-identity alias through CatalogStore");

    let fixture = seed_published_release(
        &db,
        51,
        CatalogFixturePresentation {
            release_title: "Alias Evidence",
            release_artist: "Moby",
            track_title: "Identity Evidence",
            track_artist: "Moby",
            release_date: "2005-01-01",
        },
    )
    .await;
    let worker = WorkerId::new();
    let catalog_now = OffsetDateTime::now_utc();
    let claimed = store
        .claim_cataloging(&db.pool, &worker, &test_config(), catalog_now)
        .await
        .expect("claim identity-evidence import");
    assert_eq!(claimed.len(), 1);
    store
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("identity-evidence-v1", "catalog-identity-test"),
            catalog_now,
        )
        .await
        .expect("import identity-evidence fixture");
    let observations: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM observed_artists WHERE release_id=$1 ORDER BY scope,source_ordinal,id",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("query imported Moby observations");
    assert_eq!(observations.len(), 2);
    let first_observation = ObservedArtistId::from_uuid(observations[0]);
    let second_observation = ObservedArtistId::from_uuid(observations[1]);

    let alias_candidates = store
        .find_artist_candidates_by_name(&db.pool, first_observation, "MOBY", 20)
        .await
        .expect("resolve alias candidate");
    assert!(alias_candidates.iter().any(|candidate| {
        candidate.artist_id.into_uuid() == resolved_artist_id && candidate.evidence_kind == "alias"
    }));

    let proposal_current_before: Uuid = sqlx::query_scalar(
        "SELECT decision_id FROM artist_match_current WHERE observed_artist_id=$1",
    )
    .bind(second_observation.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query current decision before proposal");
    let proposal_billing_before: Uuid =
        sqlx::query_scalar("SELECT artist_id FROM track_artists WHERE observed_artist_id=$1")
            .bind(second_observation.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query billing before proposal");
    let overlay_before_proposal: (Uuid, i64, i64) = sqlx::query_as(
        "SELECT current_overlay_id,revision,
                (SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=releases.id)
         FROM releases WHERE id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query overlay before proposal");
    let proposal_evidence = ArtistMatchEvidence {
        id: ArtistMatchEvidenceId::new(),
        observed_artist_id: second_observation,
        candidate_artist_id: ArtistId::from_uuid(rejected_artist_id),
        kind: EvidenceKind::Alias,
        polarity: EvidencePolarity::Supports,
        confidence: Confidence::new(7_000).expect("valid confidence"),
        source: "proposal-fixture".to_owned(),
    };
    let missing_evidence = ArtistMatchEvidence {
        id: ArtistMatchEvidenceId::new(),
        observed_artist_id: second_observation,
        candidate_artist_id: ArtistId::from_uuid(rejected_artist_id),
        kind: EvidenceKind::Alias,
        polarity: EvidencePolarity::Supports,
        confidence: Confidence::new(7_000).expect("valid confidence"),
        source: "unpersisted-proposal-fixture".to_owned(),
    };
    let proposal_with_missing_evidence = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: ArtistMatchDecisionId::new(),
            observed_artist_id: second_observation,
            artist_id: ArtistId::from_uuid(rejected_artist_id),
            outcome: ArtistMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "proposal-resolver-v1".to_owned(),
            },
            locked: false,
            reason: "must refer to durable exact evidence".to_owned(),
            decided_at: catalog_now,
        },
        std::slice::from_ref(&missing_evidence),
    )
    .expect("domain-valid proposal with unpersisted evidence");
    assert!(matches!(
        store
            .record_artist_match_decision(
                &db.pool,
                &proposal_with_missing_evidence,
                Confidence::new(7_000).expect("valid confidence"),
                serde_json::json!({"fixture": "missing-evidence"}),
            )
            .await,
        Err(CatalogStoreError::MutationConflict)
    ));
    let rejected_proposal_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM artist_match_decisions WHERE id=$1")
            .bind(proposal_with_missing_evidence.id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query rejected proposal");
    assert_eq!(rejected_proposal_count, 0);
    store
        .record_artist_match_evidence(
            &db.pool,
            &proposal_evidence,
            serde_json::json!({"candidate": "Moby Dick"}),
            catalog_now,
        )
        .await
        .expect("persist proposal evidence");
    let proposal = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: ArtistMatchDecisionId::new(),
            observed_artist_id: second_observation,
            artist_id: ArtistId::from_uuid(rejected_artist_id),
            outcome: ArtistMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "proposal-resolver-v1".to_owned(),
            },
            locked: false,
            reason: "durable candidate only".to_owned(),
            decided_at: catalog_now,
        },
        std::slice::from_ref(&proposal_evidence),
    )
    .expect("valid evidence-backed proposal");
    store
        .record_artist_match_decision(
            &db.pool,
            &proposal,
            Confidence::new(7_000).expect("valid confidence"),
            serde_json::json!({"fixture": "proposal-only"}),
        )
        .await
        .expect("persist proposal without applying it");
    let proposal_current_after: Uuid = sqlx::query_scalar(
        "SELECT decision_id FROM artist_match_current WHERE observed_artist_id=$1",
    )
    .bind(second_observation.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query current decision after proposal");
    let proposal_billing_after: Uuid =
        sqlx::query_scalar("SELECT artist_id FROM track_artists WHERE observed_artist_id=$1")
            .bind(second_observation.into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("query billing after proposal");
    let overlay_after_proposal: (Uuid, i64, i64) = sqlx::query_as(
        "SELECT current_overlay_id,revision,
                (SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=releases.id)
         FROM releases WHERE id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query overlay after proposal");
    assert_eq!(proposal_current_after, proposal_current_before);
    assert_eq!(proposal_billing_after, proposal_billing_before);
    assert_eq!(overlay_after_proposal, overlay_before_proposal);
    let persisted_proposal: (String, Vec<Uuid>) = sqlx::query_as(
        "SELECT outcome,
                ARRAY(SELECT jsonb_array_elements_text(payload->'evidence_ids')::uuid)
         FROM artist_match_decisions WHERE id=$1",
    )
    .bind(proposal.id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query durable proposal and evidence references");
    assert_eq!(persisted_proposal.0, "proposed");
    assert_eq!(persisted_proposal.1, vec![proposal_evidence.id.into_uuid()]);

    let original_track: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
        .bind(fixture.release_id.into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("query original Release Track");
    let second_track = Uuid::now_v7();
    let second_audio_identity = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO audio_identities (id,status,created_at,updated_at)
         VALUES ($1,'provisional',$2,$2)",
    )
    .bind(second_audio_identity)
    .bind(catalog_now)
    .execute(&db.pool)
    .await
    .expect("insert second AudioIdentity fixture");
    let mut second_track_tx = db
        .pool
        .begin()
        .await
        .expect("begin complete second Track fixture");
    let reused_media_asset: Uuid =
        sqlx::query_scalar("SELECT id FROM media_assets ORDER BY id LIMIT 1")
            .fetch_one(&mut *second_track_tx)
            .await
            .expect("query playable MediaAsset reused by second fixture Track");
    sqlx::query(
        "INSERT INTO tracks (
            id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,ordinal,title,
            normalized_title,sort_title,display_artist,duration_ms,source_path,
            created_at,updated_at
         ) VALUES ($1,$1,$2,$3,$4,2,'Second Release Track','second release track',
                   'Second Release Track','Moby',180000,'missing://second-track',$5,$5)",
    )
    .bind(second_track)
    .bind(fixture.release_id.into_uuid())
    .bind(second_audio_identity)
    .bind(reused_media_asset)
    .bind(catalog_now)
    .execute(&mut *second_track_tx)
    .await
    .expect("insert second Release Track fixture");
    sqlx::query(
        "INSERT INTO track_media_assets (track_id,media_asset_id,purpose,ordinal,created_at)
         VALUES ($1,$2,'playable',1,$3)",
    )
    .bind(second_track)
    .bind(reused_media_asset)
    .bind(catalog_now)
    .execute(&mut *second_track_tx)
    .await
    .expect("attach one playable asset to second Track fixture");
    second_track_tx
        .commit()
        .await
        .expect("commit complete second Track fixture");
    store
        .rebuild_artist_track_associations(
            &db.pool,
            &[TrackId::from_uuid(second_track)],
            catalog_now,
        )
        .await
        .expect("seed Release-level participation for second Track");

    let decision_id = ArtistMatchDecisionId::new();
    let manual = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: decision_id,
            observed_artist_id: first_observation,
            artist_id: ArtistId::from_uuid(resolved_artist_id),
            outcome: ArtistMatchOutcome::Confirmed,
            authority: DecisionAuthority::Administrator {
                actor: "catalog-admin".to_owned(),
            },
            locked: true,
            reason: "confirmed alias identity".to_owned(),
            decided_at: catalog_now + Duration::seconds(1),
        },
        &[],
    )
    .expect("valid locked manual decision");
    store
        .record_artist_match_decision(
            &db.pool,
            &manual,
            Confidence::new(10_000).expect("valid confidence"),
            serde_json::json!({"fixture": "manual-alias"}),
        )
        .await
        .expect("persist locked admin decision through CatalogStore");
    let revised_release: (i64, i64) = sqlx::query_as(
        "SELECT revision,
                (SELECT count(*) FROM catalog_semantic_overlays WHERE release_id=releases.id)
         FROM releases WHERE id=$1",
    )
    .bind(fixture.release_id.into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("query overlay correction revision");
    assert_eq!(revised_release, (2, 2));
    let resolved_release_tracks: BTreeSet<Uuid> = sqlx::query_scalar(
        "SELECT track_id FROM artist_track_associations
         WHERE artist_id=$1 AND association_type='primary_artist'",
    )
    .bind(resolved_artist_id)
    .fetch_all(&db.pool)
    .await
    .expect("query confirmed Release-level participation")
    .into_iter()
    .collect();
    assert_eq!(
        resolved_release_tracks,
        [original_track, second_track].into_iter().collect(),
        "confirmed Release-level decision refreshes every Track in the Release"
    );
    assert!(
        sqlx::query(
            "UPDATE artist_match_decisions SET reason = 'provider overwrite' WHERE id = $1"
        )
        .bind(decision_id.into_uuid())
        .execute(&db.pool)
        .await
        .is_err(),
        "decision evidence is append-only"
    );

    let provider_refresh = ArtistMatchDecision::new(
        NewArtistMatchDecision {
            id: ArtistMatchDecisionId::new(),
            observed_artist_id: first_observation,
            artist_id: ArtistId::from_uuid(rejected_artist_id),
            outcome: ArtistMatchOutcome::Proposed,
            authority: DecisionAuthority::Automated {
                resolver_version: "fake-authority-provider-v2".to_owned(),
            },
            locked: false,
            reason: "provider refresh must not override admin".to_owned(),
            decided_at: catalog_now + Duration::seconds(2),
        },
        &[],
    )
    .expect("valid automated proposal");
    assert!(matches!(
        store
            .record_artist_match_decision(
                &db.pool,
                &provider_refresh,
                Confidence::new(9_000).expect("valid confidence"),
                serde_json::json!({"provider": "fake-authority"}),
            )
            .await,
        Err(CatalogStoreError::LockedDecision)
    ));

    let assertion_id = ArtistNegativeAssertionId::new();
    store
        .record_artist_negative_assertion(
            &db.pool,
            &ArtistNegativeAssertion {
                id: assertion_id,
                observed_artist_id: first_observation,
                rejected_artist_id: ArtistId::from_uuid(rejected_artist_id),
                authority: DecisionAuthority::Administrator {
                    actor: "catalog-admin".to_owned(),
                },
                locked: true,
                reason: "explicitly rejected candidate".to_owned(),
                decided_at: catalog_now + Duration::seconds(3),
            },
        )
        .await
        .expect("persist scoped negative assertion through CatalogStore");
    assert!(
        store
            .artist_candidate_is_suppressed(
                &db.pool,
                first_observation,
                ArtistId::from_uuid(rejected_artist_id),
            )
            .await
            .expect("query exact local negative")
    );
    assert!(
        !store
            .artist_candidate_is_suppressed(
                &db.pool,
                second_observation,
                ArtistId::from_uuid(rejected_artist_id),
            )
            .await
            .expect("query other observation")
    );
    let assertion_counts = sqlx::query_as::<_, (i64, i64)>(
        "SELECT
            COUNT(*) FILTER (WHERE observed_artist_id = $1),
            COUNT(*) FILTER (WHERE observed_artist_id = $2)
         FROM artist_negative_assertions WHERE candidate_artist_id = $3",
    )
    .bind(first_observation.into_uuid())
    .bind(second_observation.into_uuid())
    .bind(rejected_artist_id)
    .fetch_one(&db.pool)
    .await
    .expect("query assertion scope");
    assert_eq!(assertion_counts, (1, 0), "rejection is not name-global");

    store
        .add_artist_external_identity(
            &db.pool,
            &ExternalIdentity {
                id: ExternalIdentityId::new(),
                owner: ExternalIdentityOwner::Artist(ArtistId::from_uuid(rejected_artist_id)),
                provider: "fake-authority".to_owned(),
                external_id: "candidate-404".to_owned(),
                confidence: Confidence::new(8_000).expect("valid confidence"),
                evidence_source: "provider-fixture".to_owned(),
                verified_at: Some(catalog_now),
                locked: false,
                provider_guarantees_uniqueness: true,
            },
            catalog_now,
        )
        .await
        .expect("persist external provider evidence");
    assert_eq!(
        store
            .find_artist_candidates_by_external_id(
                &db.pool,
                first_observation,
                "fake-authority",
                "candidate-404",
                20,
            )
            .await
            .expect("find provider candidate before negative")
            .len(),
        1
    );
    store
        .record_provider_negative_assertion(
            &db.pool,
            &ProviderNegativeAssertion {
                id: Uuid::now_v7(),
                observed_artist_id: first_observation,
                provider: "fake-authority".to_owned(),
                external_id: "candidate-404".to_owned(),
                authority: DecisionAuthority::Administrator {
                    actor: "catalog-admin".to_owned(),
                },
                locked: true,
                reason: "provider is evidence, not identity authority".to_owned(),
                decided_at: catalog_now + Duration::seconds(4),
            },
        )
        .await
        .expect("persist provider-keyed negative");
    assert!(
        store
            .find_artist_candidates_by_external_id(
                &db.pool,
                first_observation,
                "fake-authority",
                "candidate-404",
                20,
            )
            .await
            .expect("filter exact provider candidate")
            .is_empty()
    );
    assert_eq!(
        store
            .find_artist_candidates_by_external_id(
                &db.pool,
                second_observation,
                "fake-authority",
                "candidate-404",
                20,
            )
            .await
            .expect("provider negative is not name-global")
            .len(),
        1
    );

    store
        .revoke_artist_negative_assertion(
            &db.pool,
            assertion_id.into_uuid(),
            &DecisionAuthority::Administrator {
                actor: "catalog-admin".to_owned(),
            },
            "audited reversal",
            catalog_now + Duration::seconds(5),
        )
        .await
        .expect("append audited assertion reversal through CatalogStore");
    assert!(
        !store
            .artist_candidate_is_suppressed(
                &db.pool,
                first_observation,
                ArtistId::from_uuid(rejected_artist_id),
            )
            .await
            .expect("revocation supersedes exact local negative")
    );
    assert_eq!(
        store
            .find_artist_candidates_by_name(&db.pool, first_observation, "MOBYDICK", 20)
            .await
            .expect("revoked candidate can be proposed again")
            .len(),
        1
    );
    assert!(
        sqlx::query("DELETE FROM artist_negative_assertions WHERE id = $1")
            .bind(assertion_id.into_uuid())
            .execute(&db.pool)
            .await
            .is_err(),
        "negative evidence cannot be erased"
    );

    let alias_owner: (Uuid,) =
        sqlx::query_as("SELECT artist_id FROM artist_aliases WHERE normalized_alias = 'MOBY'")
            .fetch_one(&db.pool)
            .await
            .expect("query alias owner");
    assert_eq!(alias_owner.0, resolved_artist_id);

    db.drop().await;
}
