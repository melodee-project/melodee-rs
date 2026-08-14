//! Phase 9 WP-06 radio station store integrity tests (PRD-RADIO-001/002,
//! ADR-0134 §1): round-trip, field-optional update semantics, validation
//! bounds mirroring the migration 0020 CHECK constraints, deterministic
//! ordering, and immutable public identity.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use common::TestDb;
use melodee_persistence::{
    NewRadioStation, RadioStationStore, RadioStationUpdate, RadioStoreError,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const T0: OffsetDateTime = common::T0;

fn new_station<'a>(
    name: &'a str,
    stream_url: &'a str,
    home_page_url: Option<&'a str>,
) -> NewRadioStation<'a> {
    NewRadioStation {
        name,
        stream_url,
        home_page_url,
    }
}

#[tokio::test]
async fn create_get_list_round_trip() {
    let db = TestDb::create("wp06_radio_round_trip").await;
    let store = RadioStationStore::new();

    let first = store
        .create(
            &db.pool,
            &new_station(
                "Beta Radio",
                "http://beta.example.test/stream",
                Some("http://beta.example.test"),
            ),
            T0,
        )
        .await
        .expect("create beta");
    let second = store
        .create(
            &db.pool,
            &new_station("Alpha Radio", "http://alpha.example.test/stream", None),
            T0,
        )
        .await
        .expect("create alpha");

    // Ids are issued once as v7 UUIDs and differ per station.
    assert_ne!(first.id, second.id);
    assert_ne!(first.public_api_id, second.public_api_id);
    assert_eq!(first.created_at, T0);
    assert_eq!(first.updated_at, T0);

    let fetched = store
        .get_by_public_id(&db.pool, first.public_api_id)
        .await
        .expect("get")
        .expect("station");
    assert_eq!(fetched, first);

    // Listing is ordered by (name, public_api_id) and pages deterministically.
    let page = store.list(&db.pool, 50, 0).await.expect("list");
    assert_eq!(page, vec![second.clone(), first.clone()]);
    let page = store.list(&db.pool, 1, 1).await.expect("list page 2");
    assert_eq!(page, vec![first.clone()]);
    let page = store.list(&db.pool, 1, 2).await.expect("list past end");
    assert!(page.is_empty());

    // Unknown ids read as absent, never as an error.
    let missing = store
        .get_by_public_id(&db.pool, Uuid::now_v7())
        .await
        .expect("get missing");
    assert!(missing.is_none());
    db.drop().await;
}

#[tokio::test]
async fn update_is_field_optional_and_can_clear_homepage() {
    let db = TestDb::create("wp06_radio_update").await;
    let store = RadioStationStore::new();
    let station = store
        .create(
            &db.pool,
            &new_station(
                "Update Me",
                "http://update.example.test/stream",
                Some("http://update.example.test"),
            ),
            T0,
        )
        .await
        .expect("create");
    let later = T0 + Duration::seconds(60);

    // Partial update: only the name changes.
    let updated = store
        .update(
            &db.pool,
            station.public_api_id,
            &RadioStationUpdate {
                name: Some("Renamed".to_string()),
                ..RadioStationUpdate::default()
            },
            later,
        )
        .await
        .expect("rename");
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.stream_url, station.stream_url);
    assert_eq!(updated.home_page_url, station.home_page_url);
    assert_eq!(updated.created_at, station.created_at);
    assert_eq!(updated.updated_at, later);
    // Identity is untouched by updates.
    assert_eq!(updated.id, station.id);
    assert_eq!(updated.public_api_id, station.public_api_id);

    // Explicit null-equivalent clears the homepage link.
    let cleared = store
        .update(
            &db.pool,
            station.public_api_id,
            &RadioStationUpdate {
                home_page_url: Some(None),
                ..RadioStationUpdate::default()
            },
            later,
        )
        .await
        .expect("clear homepage");
    assert_eq!(cleared.home_page_url, None);
    assert_eq!(cleared.name, "Renamed");

    // Unknown id fails with NotFound.
    let result = store
        .update(
            &db.pool,
            Uuid::now_v7(),
            &RadioStationUpdate {
                name: Some("Nope".to_string()),
                ..RadioStationUpdate::default()
            },
            later,
        )
        .await;
    assert!(matches!(result, Err(RadioStoreError::NotFound)));
    db.drop().await;
}

#[tokio::test]
async fn validation_bounds_match_the_table_constraints() {
    let db = TestDb::create("wp06_radio_validation").await;
    let store = RadioStationStore::new();

    for (station, expected) in [
        (
            new_station("   ", "http://ok.example.test/s", None),
            RadioStoreError::InvalidName,
        ),
        (
            new_station(&"n".repeat(501), "http://ok.example.test/s", None),
            RadioStoreError::InvalidName,
        ),
        (
            new_station("Ok", "  ", None),
            RadioStoreError::InvalidStreamUrl,
        ),
        (
            new_station("Ok", &format!("http://{}", "u".repeat(2000)), None),
            RadioStoreError::InvalidStreamUrl,
        ),
        (
            new_station("Ok", "http://ok.example.test/s", Some(&"h".repeat(2001))),
            RadioStoreError::InvalidHomePageUrl,
        ),
    ] {
        let result = store.create(&db.pool, &station, T0).await;
        let error = result
            .err()
            .unwrap_or_else(|| panic!("expected {expected}"));
        assert_eq!(error.to_string(), expected.to_string());
    }
    assert_eq!(
        store.list(&db.pool, 50, 0).await.expect("list").len(),
        0,
        "rejected stations must not persist"
    );

    // Boundary values (exactly at the constraint limit) succeed.
    let boundary = store
        .create(
            &db.pool,
            &new_station(
                &"n".repeat(500),
                &format!("http://{}", "u".repeat(1993)),
                Some(&"h".repeat(2000)),
            ),
            T0,
        )
        .await
        .expect("boundary values are valid");
    assert_eq!(boundary.name.chars().count(), 500);

    // Invalid updates are rejected without touching the stored row.
    let result = store
        .update(
            &db.pool,
            boundary.public_api_id,
            &RadioStationUpdate {
                name: Some(" ".to_string()),
                ..RadioStationUpdate::default()
            },
            T0,
        )
        .await;
    assert!(matches!(result, Err(RadioStoreError::InvalidName)));
    let unchanged = store
        .get_by_public_id(&db.pool, boundary.public_api_id)
        .await
        .expect("get")
        .expect("station");
    assert_eq!(unchanged, boundary);
    db.drop().await;
}

#[tokio::test]
async fn public_identity_is_immutable_at_the_database() {
    let db = TestDb::create("wp06_radio_identity").await;
    let store = RadioStationStore::new();
    let station = store
        .create(
            &db.pool,
            &new_station("Stable", "http://stable.example.test/s", None),
            T0,
        )
        .await
        .expect("create");

    // The migration 0020 trigger rejects any id/public_api_id rewrite — the
    // fail-closed backstop for PRD-RADIO-002 id stability.
    let rewrite = sqlx::query(
        "UPDATE opensubsonic_radio_stations SET public_api_id = $2 WHERE public_api_id = $1",
    )
    .bind(station.public_api_id)
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await;
    assert!(rewrite.is_err(), "identity rewrite must be rejected");

    let unchanged = store
        .get_by_public_id(&db.pool, station.public_api_id)
        .await
        .expect("get")
        .expect("station");
    assert_eq!(unchanged, station);
    db.drop().await;
}

#[tokio::test]
async fn delete_removes_only_the_addressed_station() {
    let db = TestDb::create("wp06_radio_delete").await;
    let store = RadioStationStore::new();
    let kept = store
        .create(
            &db.pool,
            &new_station("Keep", "http://keep.example.test/s", None),
            T0,
        )
        .await
        .expect("create keep");
    let gone = store
        .create(
            &db.pool,
            &new_station("Drop", "http://drop.example.test/s", None),
            T0,
        )
        .await
        .expect("create drop");

    assert!(
        store
            .delete(&db.pool, gone.public_api_id)
            .await
            .expect("delete")
    );
    assert!(
        !store
            .delete(&db.pool, gone.public_api_id)
            .await
            .expect("second delete reports false")
    );
    let remaining = store.list(&db.pool, 50, 0).await.expect("list");
    assert_eq!(remaining, vec![kept]);
    db.drop().await;
}
