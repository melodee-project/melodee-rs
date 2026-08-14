//! Phase 9 WP-10 device profile acceptance (ADR-0134 §5,
//! PRD-DEVICE-001/002): migration shape, constraint integrity, bounded CRUD,
//! legacy-parity validation, and the atomic single-default invariant.

#![allow(clippy::expect_used)]

mod common;

use common::TestDb;
use melodee_persistence::{
    DeviceProfileStore, DeviceProfileStoreError, DeviceProfileUpdate, NewDeviceProfile,
};
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

fn new_profile(name: &str, is_default: bool) -> NewDeviceProfile {
    NewDeviceProfile {
        name: name.to_string(),
        client_match: json!({"clientName": "feishin", "userAgentContains": "Feishin/"}),
        profile: json!({"directPlay": false, "targetCodec": "opus", "maxBitrate": 192}),
        is_default,
    }
}

async fn default_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM device_profiles WHERE is_default")
        .fetch_one(pool)
        .await
        .expect("count defaults")
}

#[tokio::test]
async fn crud_round_trip_preserves_metadata_and_ordering() {
    let db = TestDb::create("device_profiles_crud").await;
    let store = DeviceProfileStore::new();
    let now = OffsetDateTime::now_utc();

    let first = store
        .create(&db.pool, &new_profile("b-mobile", false), now)
        .await
        .expect("create first");
    let second = store
        .create(&db.pool, &new_profile("a-desktop", true), now)
        .await
        .expect("create second");

    let fetched = store.get(&db.pool, first.id).await.expect("get");
    assert_eq!(fetched.name, "b-mobile");
    assert_eq!(fetched.profile["targetCodec"], Value::String("opus".into()));
    assert!(!fetched.is_default);

    // Listing is default-first, then name: a-desktop (default), b-mobile.
    let listed = store.list(&db.pool, 50, 0).await.expect("list");
    assert_eq!(
        listed.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![second.id, first.id]
    );

    let updated = store
        .update(
            &db.pool,
            first.id,
            &DeviceProfileUpdate {
                name: Some("b-mobile-renamed".to_string()),
                client_match: None,
                profile: Some(json!({"directPlay": true})),
            },
            now,
        )
        .await
        .expect("update");
    assert_eq!(updated.name, "b-mobile-renamed");
    assert_eq!(updated.profile, json!({"directPlay": true}));
    // Omitted fields keep their stored value.
    assert_eq!(updated.client_match, first.client_match);
    assert_eq!(updated.created_at, first.created_at);

    store.delete(&db.pool, second.id).await.expect("delete");
    assert!(matches!(
        store.get(&db.pool, second.id).await,
        Err(DeviceProfileStoreError::NotFound)
    ));
    db.drop().await;
}

#[tokio::test]
async fn single_default_invariant_holds_across_create_and_set_default() {
    let db = TestDb::create("device_profiles_default").await;
    let store = DeviceProfileStore::new();
    let now = OffsetDateTime::now_utc();

    let first = store
        .create(&db.pool, &new_profile("first", true), now)
        .await
        .expect("create first default");
    // Creating a second default atomically clears the first.
    let second = store
        .create(&db.pool, &new_profile("second", true), now)
        .await
        .expect("create second default");
    assert_eq!(default_count(&db.pool).await, 1);
    assert!(!store.get(&db.pool, first.id).await.expect("get").is_default);

    // set_default flips atomically back.
    let flipped = store
        .set_default(&db.pool, first.id, now)
        .await
        .expect("set_default");
    assert!(flipped.is_default);
    assert_eq!(default_count(&db.pool).await, 1);
    assert!(
        !store
            .get(&db.pool, second.id)
            .await
            .expect("get")
            .is_default
    );

    // Unknown id reports NotFound and leaves the invariant untouched.
    assert!(matches!(
        store.set_default(&db.pool, Uuid::now_v7(), now).await,
        Err(DeviceProfileStoreError::NotFound)
    ));
    assert_eq!(default_count(&db.pool).await, 1);

    // The partial unique index is fail-closed even against raw SQL.
    let raw =
        sqlx::query("INSERT INTO device_profiles (id, name, is_default) VALUES ($1, 'raw', true)")
            .bind(Uuid::now_v7())
            .execute(&db.pool)
            .await;
    assert!(raw.is_err(), "second default row must be rejected");
    db.drop().await;
}

#[tokio::test]
async fn legacy_parity_validation_rules_are_enforced() {
    let db = TestDb::create("device_profiles_validation").await;
    let store = DeviceProfileStore::new();
    let now = OffsetDateTime::now_utc();

    // Blank and over-long names are rejected before touching the database,
    // and the migration CHECK is the backstop.
    for bad in ["", "   ", &"x".repeat(256)] {
        assert!(matches!(
            store.create(&db.pool, &new_profile(bad, false), now).await,
            Err(DeviceProfileStoreError::InvalidProfile(_))
        ));
    }
    let raw = sqlx::query("INSERT INTO device_profiles (id, name) VALUES ($1, '   ')")
        .bind(Uuid::now_v7())
        .execute(&db.pool)
        .await;
    assert!(raw.is_err(), "blank name must violate the CHECK");

    // Legacy rule: direct play must not carry codec/bitrate.
    let mut invalid = new_profile("direct-play-with-codec", false);
    invalid.profile = json!({"directPlay": true, "targetCodec": "mp3"});
    assert!(matches!(
        store.create(&db.pool, &invalid, now).await,
        Err(DeviceProfileStoreError::InvalidProfile(_))
    ));

    // Legacy rule: transcoding profiles must name a codec.
    let mut invalid = new_profile("transcode-without-codec", false);
    invalid.profile = json!({"directPlay": false});
    assert!(matches!(
        store.create(&db.pool, &invalid, now).await,
        Err(DeviceProfileStoreError::InvalidProfile(_))
    ));

    // Bounded numeric fields (legacy dialog bounds).
    let mut invalid = new_profile("bitrate-out-of-range", false);
    invalid.profile = json!({"directPlay": false, "targetCodec": "mp3", "maxBitrate": 0});
    assert!(matches!(
        store.create(&db.pool, &invalid, now).await,
        Err(DeviceProfileStoreError::InvalidProfile(_))
    ));

    // Non-object jsonb documents are rejected by the CHECK backstop.
    let raw = sqlx::query(
        "INSERT INTO device_profiles (id, name, profile) VALUES ($1, 'scalar', '42'::jsonb)",
    )
    .bind(Uuid::now_v7())
    .execute(&db.pool)
    .await;
    assert!(raw.is_err(), "non-object profile must violate the CHECK");

    // client_match leaves must be bounded strings.
    let mut invalid = new_profile("bad-matcher", false);
    invalid.client_match = json!({"clientName": ["not", "a", "string"]});
    assert!(matches!(
        store.create(&db.pool, &invalid, now).await,
        Err(DeviceProfileStoreError::InvalidProfile(_))
    ));

    // Update path applies the same validation.
    let ok = store
        .create(&db.pool, &new_profile("valid", false), now)
        .await
        .expect("create valid");
    let invalid_update = DeviceProfileUpdate {
        name: None,
        client_match: None,
        profile: Some(json!({"directPlay": false})),
    };
    assert!(matches!(
        store.update(&db.pool, ok.id, &invalid_update, now).await,
        Err(DeviceProfileStoreError::InvalidProfile(_))
    ));
    db.drop().await;
}
