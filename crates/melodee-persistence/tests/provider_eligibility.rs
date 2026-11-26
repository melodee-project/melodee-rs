//! Provider eligibility, bounded scheduling, backfill and dormant-work
//! acceptance evidence (ADR-0212).
//!
//! The scenarios mirror the operator-visible contract: an install only pays
//! for enabled, credentialed providers; enabling one later covers the existing
//! catalog exactly once; and disablement suspends history without losing it.

#![allow(clippy::expect_used, clippy::panic)]

mod catalog_common;
mod common;

use catalog_common::{CatalogFixturePresentation, seed_published_release};
use common::{TestDb, test_config};
use melodee_persistence::{
    CatalogImportRequest, CatalogStore, EligibilityRefreshOutcome, EnrichmentEntityType,
    EnrichmentPurpose, ProviderEligibilityStore, ProviderEnrichmentStore,
    ResolvedProviderEligibilityOwned, WorkerId, resolve_provider_eligibility,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const ALL_PROVIDERS: [&str; 9] = [
    "brave",
    "deezer",
    "discogs",
    "itunes",
    "lastfm",
    "metalapi",
    "musicbrainz",
    "spotify",
    "wikidata",
];

fn snapshot(eligible: &[&str]) -> Vec<ResolvedProviderEligibilityOwned> {
    ALL_PROVIDERS
        .iter()
        .map(|provider| ResolvedProviderEligibilityOwned {
            provider: (*provider).to_owned(),
            metadata_eligible: eligible.contains(provider),
            artwork_eligible: eligible.contains(provider),
        })
        .collect()
}

async fn import_fixture(db: &TestDb, ordinal: u32, title: &str) -> (Uuid, Vec<Uuid>) {
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    let now = OffsetDateTime::now_utc();
    let fixture = seed_published_release(
        db,
        ordinal,
        CatalogFixturePresentation {
            release_title: title,
            ..CatalogFixturePresentation::default()
        },
    )
    .await;
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim cataloging");
    let imported = catalog
        .import_manifest_v3(
            &db.pool,
            fixture.work_item_id,
            &worker,
            &CatalogImportRequest::new("provider-eligibility-v1", "eligibility-test"),
            now,
        )
        .await
        .expect("import fixture");
    let artist_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT artist_id FROM release_artists WHERE release_id=$1
         UNION SELECT DISTINCT artist_id FROM track_artists
          WHERE track_id IN (SELECT id FROM tracks WHERE release_id=$1)",
    )
    .bind(imported.release_id.into_uuid())
    .fetch_all(&db.pool)
    .await
    .expect("read imported artists");
    (imported.release_id.into_uuid(), artist_ids)
}

async fn task_count(db: &TestDb, entity_ids: &[Uuid]) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM provider_enrichment_tasks WHERE entity_id=ANY($1)")
        .bind(entity_ids)
        .fetch_one(&db.pool)
        .await
        .expect("count tasks")
}

async fn status_of(db: &TestDb, task_id: Uuid) -> (String, i32) {
    sqlx::query_as("SELECT status, attempt_count FROM provider_enrichment_tasks WHERE id=$1")
        .bind(task_id)
        .fetch_one(&db.pool)
        .await
        .expect("read task")
}

async fn seed_artist(db: &TestDb, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO artists (id, public_api_id, canonical_name, normalized_name, sort_name,
                              artist_type, status, revision, created_at, updated_at)
         VALUES ($1,$1,$2,$2,$2,'group','active',1,now(),now())",
    )
    .bind(id)
    .bind(name)
    .execute(&db.pool)
    .await
    .expect("seed artist");
    id
}

/// A repeated import must not create additional jobs, and the long-term
/// invariant is one scheduling key per entity/provider/purpose.
#[tokio::test]
async fn repeated_import_schedules_each_purpose_once() {
    let db = TestDb::create("eligibility_repeat_import").await;
    let now = OffsetDateTime::now_utc();
    let outcome = ProviderEligibilityStore::refresh(
        &db.pool,
        &snapshot(&["musicbrainz", "itunes", "deezer"]),
        now,
    )
    .await
    .expect("resolve eligibility");
    assert_eq!(
        outcome.backfills_created, 6,
        "one backfill per eligible provider and entity type"
    );

    let (release_id, artist_ids) = import_fixture(&db, 301, "Eligible Fixture").await;
    let mut entities = artist_ids.clone();
    entities.push(release_id);
    // Two purposes per provider for the Release and for each Artist; three
    // eligible providers.
    let expected = 6 + 6 * i64::try_from(artist_ids.len()).expect("artist count");
    assert_eq!(task_count(&db, &entities).await, expected);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(DISTINCT provider) FROM provider_enrichment_tasks WHERE entity_id=ANY($1)",
        )
        .bind(&entities)
        .fetch_one(&db.pool)
        .await
        .expect("distinct providers"),
        3
    );

    // Importing the same publication again is idempotent.
    let catalog = CatalogStore;
    let worker = WorkerId::new();
    catalog
        .claim_cataloging(&db.pool, &worker, &test_config(), now)
        .await
        .expect("claim cataloging");
    let work_item_id: Uuid = sqlx::query_scalar(
        "SELECT work_item_id FROM catalog_imports WHERE release_id=$1 ORDER BY imported_at LIMIT 1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("read import");
    let _ = catalog
        .import_manifest_v3(
            &db.pool,
            work_item_id,
            &worker,
            &CatalogImportRequest::new("provider-eligibility-v1", "eligibility-test"),
            now,
        )
        .await;
    assert_eq!(task_count(&db, &entities).await, expected);
}

/// With zero eligible providers the same import creates no work at all and
/// still reaches a playable catalog state.
#[tokio::test]
async fn zero_eligible_providers_create_no_work() {
    let db = TestDb::create("eligibility_none").await;
    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&[]), OffsetDateTime::now_utc())
        .await
        .expect("resolve eligibility");

    let (release_id, artist_ids) = import_fixture(&db, 302, "Unenriched Fixture").await;
    let mut entities = artist_ids.clone();
    entities.push(release_id);
    assert_eq!(task_count(&db, &entities).await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_enrichment_attempts")
            .fetch_one(&db.pool)
            .await
            .expect("count attempts"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM release_work_items WHERE id=(
                SELECT work_item_id FROM catalog_imports WHERE release_id=$1)",
        )
        .bind(release_id)
        .fetch_one(&db.pool)
        .await
        .expect("read work item state"),
        "cataloged",
        "unreachable providers must not block catalogability"
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM tracks WHERE release_id=$1 AND playable_media_asset_id IS NOT NULL)",
        )
        .bind(release_id)
        .fetch_one(&db.pool)
        .await
        .expect("playable media")
    );

    // Enabling a fourth provider backfills exactly the missing purposes for
    // the entities the disabled period created, and only once.
    let outcome = ProviderEligibilityStore::refresh(
        &db.pool,
        &snapshot(&["musicbrainz"]),
        OffsetDateTime::now_utc(),
    )
    .await
    .expect("enable provider");
    assert_eq!(outcome.backfills_created, 2);
    for _ in 0..4 {
        if ProviderEligibilityStore::advance_backfill(
            &db.pool,
            OffsetDateTime::now_utc(),
            melodee_persistence::BACKFILL_PAGE_SIZE,
        )
        .await
        .expect("advance backfill")
        .is_none()
        {
            break;
        }
    }
    assert_eq!(
        task_count(&db, &entities).await,
        (2 + 2 * i64::try_from(artist_ids.len()).expect("artist count")),
        "the backfill covers exactly the missing metadata and artwork purposes"
    );
    assert_eq!(
        ProviderEligibilityStore::advance_backfill(
            &db.pool,
            OffsetDateTime::now_utc(),
            melodee_persistence::BACKFILL_PAGE_SIZE,
        )
        .await
        .expect("completed backfill"),
        None,
        "a completed scan is never restarted"
    );
}

/// Disablement suspends historically attempted work; enablement revives it
/// exactly once; dormant rows are never claimed.
#[tokio::test]
async fn disablement_suspends_and_enablement_revives_once() {
    let db = TestDb::create("eligibility_dormant").await;
    let store = ProviderEnrichmentStore;
    let now = OffsetDateTime::now_utc();
    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&["lastfm"]), now)
        .await
        .expect("resolve eligibility");
    let artist_id = seed_artist(&db, "Dormant Artist").await;
    store
        .schedule(
            &db.pool,
            EnrichmentEntityType::Artist,
            artist_id,
            "lastfm",
            EnrichmentPurpose::ArtistMetadata,
            &format!("artist:{artist_id}:lastfm:artist_metadata"),
            now,
        )
        .await
        .expect("schedule");

    let claimed = store
        .claim(&db.pool, "worker-a", 8, 30, now)
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1, "an eligible provider is claimable");
    store
        .fail_or_retry(
            &db.pool,
            &claimed[0],
            "worker-a",
            "provider_lookup",
            "upstream unavailable",
            Some(now + Duration::seconds(5)),
            None,
            now,
        )
        .await
        .expect("record genuine retry");
    let task_id = claimed[0].id;

    let outcome =
        ProviderEligibilityStore::refresh(&db.pool, &snapshot(&[]), now + Duration::seconds(1))
            .await
            .expect("disable provider");
    assert_eq!(outcome.dormant_suspended, 1);
    assert_eq!(status_of(&db, task_id).await, ("dormant".to_owned(), 1));
    assert_eq!(
        store
            .claim(&db.pool, "worker-a", 8, 30, now + Duration::seconds(2))
            .await
            .expect("claim while disabled")
            .len(),
        0,
        "dormant work is outside the runnable set"
    );
    let stats = ProviderEligibilityStore::queue_stats(&db.pool)
        .await
        .expect("queue stats");
    let lastfm = stats
        .iter()
        .find(|row| row.provider == "lastfm")
        .expect("lastfm stats");
    assert_eq!((lastfm.dormant, lastfm.runnable), (1, 0));

    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&["lastfm"]), now + Duration::seconds(3))
        .await
        .expect("re-enable provider");
    assert_eq!(status_of(&db, task_id).await, ("queued".to_owned(), 1));
    let claimed = store
        .claim(&db.pool, "worker-a", 8, 30, now + Duration::seconds(4))
        .await
        .expect("claim after enable");
    assert_eq!(claimed.len(), 1, "revived work runs exactly once");
    assert_eq!(claimed[0].attempt_count, 2, "the retry budget is preserved");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM provider_enrichment_tasks WHERE entity_id=$1",
        )
        .bind(artist_id)
        .fetch_one(&db.pool)
        .await
        .expect("task identity preserved"),
        1
    );

    // A disable observed before I/O releases the claim without an attempt row.
    let before = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_enrichment_attempts")
        .fetch_one(&db.pool)
        .await
        .expect("count attempts");
    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&[]), now + Duration::seconds(5))
        .await
        .expect("disable again");
    store
        .release_claim(
            &db.pool,
            &claimed[0],
            "worker-a",
            now + Duration::seconds(6),
        )
        .await
        .expect("release claim");
    assert_eq!(status_of(&db, task_id).await, ("retry".to_owned(), 1));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM provider_enrichment_attempts")
            .fetch_one(&db.pool)
            .await
            .expect("count attempts"),
        before,
        "releasing a claim fabricates no disabled attempt"
    );
}

/// A historical success whose only outcome was `disabled` is not satisfaction.
#[tokio::test]
async fn disabled_only_success_is_dormant_not_satisfied() {
    let db = TestDb::create("eligibility_disabled_only").await;
    let now = OffsetDateTime::now_utc();
    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&["spotify"]), now)
        .await
        .expect("resolve eligibility");
    let artist_id = seed_artist(&db, "Legacy Disabled Artist").await;
    let task_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO provider_enrichment_tasks (
            id, entity_type, entity_id, provider, purpose, request_key, status,
            attempt_count, next_attempt_at, created_at, updated_at, completed_at
         ) VALUES ($1,'artist',$2,'spotify','artist_metadata',
                   'artist:'||$2||':spotify:artist_metadata','succeeded',1,$3,$3,$3,$3)",
    )
    .bind(task_id)
    .bind(artist_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert legacy disabled-only success");
    sqlx::query(
        "INSERT INTO provider_enrichment_attempts (
            id, task_id, attempt_number, provider, purpose, outcome, started_at, completed_at
         ) VALUES ($1,$2,1,'spotify','artist_metadata','disabled',$3,$3)",
    )
    .bind(Uuid::now_v7())
    .bind(task_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert disabled attempt");

    let outcome =
        ProviderEligibilityStore::refresh(&db.pool, &snapshot(&[]), now + Duration::seconds(1))
            .await
            .expect("disable provider");
    assert_eq!(outcome.dormant_suspended, 1);
    assert_eq!(status_of(&db, task_id).await, ("dormant".to_owned(), 1));

    ProviderEligibilityStore::refresh(
        &db.pool,
        &snapshot(&["spotify"]),
        now + Duration::seconds(2),
    )
    .await
    .expect("re-enable provider");
    assert_eq!(
        status_of(&db, task_id).await,
        ("queued".to_owned(), 1),
        "disabled-only history is re-attempted"
    );
}

/// Never-attempted queued rows are removed; genuine success and terminal
/// failure are preserved for diagnosis.
#[tokio::test]
async fn backlog_reconciliation_distinguishes_never_attempted_and_terminal_rows() {
    let db = TestDb::create("eligibility_backlog").await;
    let now = OffsetDateTime::now_utc();
    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&["wikidata"]), now)
        .await
        .expect("resolve eligibility");
    let artist_id = seed_artist(&db, "Backlog Artist").await;
    let mut ids = Vec::new();
    for (index, (status, attempts)) in [
        ("queued", 0_i32),
        ("succeeded", 1_i32),
        ("failed", 1_i32),
        ("retry", 2_i32),
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO provider_enrichment_tasks (
                id, entity_type, entity_id, provider, purpose, request_key, status,
                attempt_count, next_attempt_at, created_at, updated_at,
                completed_at, last_error_category, last_error_message
             ) VALUES ($1,'artist',$2,'wikidata','artist_metadata',$3,$4,$5,$6,$6,$6,
                       CASE WHEN $4 IN ('succeeded','failed') THEN $6 END,
                       CASE WHEN $4='failed' THEN 'provider_lookup' END,
                       CASE WHEN $4='failed' THEN 'exhausted' END)",
        )
        .bind(id)
        .bind(artist_id)
        .bind(format!("artist:{artist_id}:wikidata:{status}{index}"))
        .bind(status)
        .bind(attempts)
        .bind(now)
        .execute(&db.pool)
        .await
        .expect("insert backlog row");
        if attempts > 0 {
            sqlx::query(
                "INSERT INTO provider_enrichment_attempts (
                    id, task_id, attempt_number, provider, purpose, outcome,
                    error_category, error_message, started_at, completed_at
                 ) VALUES ($1,$2,1,'wikidata','artist_metadata',$3,$4,$5,$6,$6)",
            )
            .bind(Uuid::now_v7())
            .bind(id)
            .bind(if status == "succeeded" {
                "success"
            } else {
                "retry"
            })
            .bind(if status == "succeeded" {
                None
            } else {
                Some("provider_lookup")
            })
            .bind(if status == "succeeded" {
                None
            } else {
                Some("upstream unavailable")
            })
            .bind(now)
            .execute(&db.pool)
            .await
            .expect("insert attempt");
        }
        ids.push(id);
    }

    let outcome =
        ProviderEligibilityStore::refresh(&db.pool, &snapshot(&[]), now + Duration::seconds(1))
            .await
            .expect("disable provider");
    assert_eq!(outcome.never_attempted_removed, 1);
    assert_eq!(outcome.dormant_suspended, 1, "only the retry row suspends");
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM provider_enrichment_tasks WHERE id=$1)",
        )
        .bind(ids[0])
        .fetch_one(&db.pool)
        .await
        .expect("inspect removed row")
    );
    assert_eq!(status_of(&db, ids[1]).await, ("succeeded".to_owned(), 1));
    assert_eq!(status_of(&db, ids[2]).await, ("failed".to_owned(), 1));
    assert_eq!(status_of(&db, ids[3]).await, ("dormant".to_owned(), 2));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM provider_enrichment_attempts WHERE task_id=ANY($1)",
        )
        .bind(&ids[1..])
        .fetch_one(&db.pool)
        .await
        .expect("attempt history survives"),
        3
    );
}

/// Two reconcilers and repeated refreshes cannot duplicate revisions,
/// backfills or work.
#[tokio::test]
async fn concurrent_reconcilers_are_idempotent() {
    let db = TestDb::create("eligibility_concurrent").await;
    let now = OffsetDateTime::now_utc();
    let first = snapshot(&["musicbrainz", "itunes"]);
    let second = snapshot(&["musicbrainz", "itunes"]);
    let (left, right) = tokio::join!(
        ProviderEligibilityStore::refresh(&db.pool, &first, now),
        ProviderEligibilityStore::refresh(&db.pool, &second, now),
    );
    let left = left.expect("reconciler a");
    let right = right.expect("reconciler b");
    let total_changes = left.changed_providers.len() + right.changed_providers.len();
    assert_eq!(total_changes, 9, "each provider transitions exactly once");
    assert_eq!(left.backfills_created + right.backfills_created, 4);

    let state = ProviderEligibilityStore::state(&db.pool)
        .await
        .expect("state");
    assert_eq!(state.revision, 9);

    let again = ProviderEligibilityStore::refresh(
        &db.pool,
        &snapshot(&["musicbrainz", "itunes"]),
        now + Duration::seconds(1),
    )
    .await
    .expect("unchanged settings");
    assert_eq!(
        again,
        EligibilityRefreshOutcome {
            revision: 9,
            ..EligibilityRefreshOutcome::default()
        }
    );
    assert_eq!(
        ProviderEligibilityStore::list_backfills(&db.pool)
            .await
            .expect("backfills")
            .len(),
        4
    );
}

/// A resolver error retains the last known eligibility and records a
/// diagnostic instead of disabling enrichment.
#[tokio::test]
async fn resolver_failure_retains_last_known_state() {
    let db = TestDb::create("eligibility_resolver_failure").await;
    let now = OffsetDateTime::now_utc();
    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&["deezer"]), now)
        .await
        .expect("resolve eligibility");
    ProviderEligibilityStore::record_resolver_failure(&db.pool, "settings unreadable", now)
        .await
        .expect("record failure");
    let state = ProviderEligibilityStore::state(&db.pool)
        .await
        .expect("state");
    assert_eq!(state.last_error.as_deref(), Some("settings unreadable"));
    let eligibility = ProviderEligibilityStore::list(&db.pool)
        .await
        .expect("eligibility");
    assert!(
        eligibility
            .iter()
            .any(|row| row.provider == "deezer" && row.metadata_eligible)
    );
}

/// Backfill pages advance by stable keyset, resume after interruption, and
/// keep the scheduling key unique.
#[tokio::test]
async fn backfill_pages_are_bounded_and_resumable() {
    let db = TestDb::create("eligibility_backfill_pages").await;
    let now = OffsetDateTime::now_utc();
    let mut ids = Vec::new();
    for index in 0..12 {
        ids.push(seed_artist(&db, &format!("Paged Artist {index}")).await);
    }
    ProviderEligibilityStore::refresh(&db.pool, &snapshot(&["metalapi"]), now)
        .await
        .expect("resolve eligibility");

    let mut pages = 0;
    let mut seen = 0_u32;
    while let Some(page) = ProviderEligibilityStore::advance_backfill(&db.pool, now, 5)
        .await
        .expect("advance page")
    {
        pages += 1;
        seen += page.entities;
    }
    // The 12 Artists need three pages of 5 and the empty Release scan
    // completes in one further page.
    assert_eq!(pages, 4);
    assert_eq!(seen, 12);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM provider_enrichment_tasks WHERE entity_id=ANY($1)",
        )
        .bind(&ids)
        .fetch_one(&db.pool)
        .await
        .expect("count tasks"),
        24,
        "one metadata and one artwork task per eligible Artist"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM (SELECT entity_id, provider, purpose, request_key,
                                    count(*) FROM provider_enrichment_tasks
                                    GROUP BY 1,2,3,4 HAVING count(*) > 1) duplicate",
        )
        .fetch_one(&db.pool)
        .await
        .expect("duplicate scan"),
        0
    );
}

/// The shared resolver derives eligibility from settings precedence: a blank
/// credential is not configured, and the local MusicBrainz option needs none.
#[tokio::test]
async fn settings_resolution_matches_capability_rules() {
    let db = TestDb::create("eligibility_settings").await;
    let env: std::collections::BTreeMap<String, String> = [
        ("MELODEE__SEARCHENGINE__MUSICBRAINZ__ENABLED", "true"),
        ("MELODEE__SEARCHENGINE__SPOTIFY__ENABLED", "true"),
        ("MELODEE__SEARCHENGINE__SPOTIFY__ACCESSTOKEN", "  "),
        ("MELODEE__SEARCHENGINE__DISCOGS__ENABLED", "true"),
        ("MELODEE__SEARCHENGINE__DISCOGS__USERTOKEN", "token-value"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value.to_owned()))
    .collect();
    let reader = move |name: &str| env.get(name).cloned();
    let resolver = melodee_persistence::SettingsResolver::with_env_reader(&db.pool, &reader);
    let resolved = resolve_provider_eligibility(&resolver)
        .await
        .expect("resolve eligibility");
    let eligible: Vec<&str> = resolved
        .iter()
        .filter(|entry| entry.metadata_eligible)
        .map(|entry| entry.provider)
        .collect();
    assert!(eligible.contains(&"musicbrainz"));
    assert!(eligible.contains(&"discogs"));
    assert!(
        !eligible.contains(&"spotify"),
        "a whitespace-only secret is not a configured credential"
    );
}
