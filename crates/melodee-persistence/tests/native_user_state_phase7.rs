//! user-state, playlist, and scrobble PostgreSQL acceptance.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "catalog_common/mod.rs"]
mod catalog_common;
#[path = "common/mod.rs"]
mod common;
#[path = "phase5_common/mod.rs"]
mod phase5_common;

use catalog_common::CatalogFixturePresentation;
use melodee_persistence::{
    NativeDiscoveryStore, NativePlaylistCreate, NativeScrobbleResult, NativeUserStateMutation,
    NativeUserStateStore, NativeUserStateStoreError,
};
use time::OffsetDateTime;
use uuid::Uuid;

async fn insert_user(db: &common::TestDb, username: &str) -> Uuid {
    let id = Uuid::now_v7();
    let public_id = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    sqlx::query(
        r#"
        INSERT INTO native_users (
            id, public_api_id, username, normalized_username, email, normalized_email,
            password_hash, password_hash_algorithm, stream_signing_key,
            created_at, updated_at
        ) VALUES ($1, $2, $3, lower($3), $4, lower($4), $5, 'bcrypt', $6, $7, $7)
        "#,
    )
    .bind(id)
    .bind(public_id)
    .bind(username)
    .bind(format!("{username}@example.test"))
    .bind("$2b$04$2b2T5QECQ1vk30e7lWDS2e2FmTHdq5nssZBrJaU0KksvJsCOQmCQS")
    .bind("phase7-test-stream-key")
    .bind(now)
    .execute(&db.pool)
    .await
    .expect("insert native user");
    id
}

async fn wait_for_lock_query(pool: &sqlx::PgPool, query_fragment: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE $1)")
                .bind(format!("%{query_fragment}%")).fetch_one(pool).await.expect("inspect lock wait");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("expected lock wait");
}

fn scrobble_probe_count() -> u64 {
    melodee_persistence::metrics::snapshot()
        .into_iter()
        .find(|metric| metric.name == "scrobble_wait_probe")
        .expect("probe metric")
        .count
}

async fn wait_for_scrobble_probe(after: u64) {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if scrobble_probe_count() > after {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("scrobble probes contended parent");
}

#[tokio::test]
async fn discovery_recount_observes_plays_and_relationship_changes_after_candidate_wait() {
    let db = common::TestDb::create("discovery_candidate_race").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7198,
        CatalogFixturePresentation {
            release_title: "Repair Race One",
            release_artist: "Repair Artist One",
            track_title: "Repair Track One",
            track_artist: "Repair Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7199,
        CatalogFixturePresentation {
            release_title: "Repair Race Two",
            release_artist: "Repair Artist Two",
            track_title: "Repair Track Two",
            track_artist: "Repair Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let (track,public,old_artist): (Uuid,Uuid,Uuid) = sqlx::query_as("SELECT t.id,t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(first.release_id().into_uuid()).fetch_one(&db.pool).await.expect("first identities");
    let new_artist: Uuid = sqlx::query_scalar("SELECT a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(second.release_id().into_uuid()).fetch_one(&db.pool).await.expect("second Artist");
    let user = insert_user(&db, "discovery-race-user").await;
    let now = OffsetDateTime::now_utc();
    NativeUserStateStore::record_scrobble(
        &db.pool,
        user,
        public,
        "repair-first",
        now,
        now,
        Some(30),
    )
    .await
    .expect("seed prior play");
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=0 WHERE entity_type='track' AND entity_id=$1")
        .bind(track).execute(&db.pool).await.expect("introduce repairable drift");

    // Pause the second play after it updated candidate rows but before it can
    // commit; a recount started now must wait before taking its snapshot.
    let mut blocker = db.pool.begin().await.expect("begin state blocker");
    sqlx::query(
        "SELECT user_id FROM native_track_user_states WHERE user_id=$1 AND track_id=$2 FOR UPDATE",
    )
    .bind(user)
    .bind(track)
    .execute(&mut *blocker)
    .await
    .expect("lock user state");
    let pool = db.pool.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "repair-second",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_lock_query(&db.pool, "INSERT INTO native_track_user_states").await;
    let pool = db.pool.clone();
    let repair = tokio::spawn(async move {
        NativeDiscoveryStore
            .refresh_discovery_aggregates(&pool, 500, 100)
            .await
    });
    wait_for_lock_query(
        &db.pool,
        "SELECT entity_id FROM native_recommendation_candidates",
    )
    .await;
    blocker.commit().await.expect("release state blocker");
    play.await
        .expect("join play")
        .expect("record concurrent play");
    repair
        .await
        .expect("join repair")
        .expect("repair popularity");
    let (actual, projected): (i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM native_track_plays WHERE track_id=$1), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='track' AND entity_id=$1)")
        .bind(track).fetch_one(&db.pool).await.expect("compare Track popularity");
    assert_eq!((actual, projected), (2, 2));

    // A committed Artist reassignment has the same ordering requirement.
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=9 WHERE entity_type='artist' AND entity_id=$1")
        .bind(new_artist).execute(&db.pool).await.expect("introduce Artist drift");
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (commit_tx, commit_rx) = tokio::sync::oneshot::channel();
    let pool = db.pool.clone();
    let change = tokio::spawn(async move {
        let mut tx = pool.begin().await.expect("begin Artist change");
        sqlx::query("UPDATE track_artists SET artist_id=$2 WHERE track_id=$1 AND role='primary'")
            .bind(track)
            .bind(new_artist)
            .execute(&mut *tx)
            .await
            .expect("change primary Artist");
        ready_tx.send(()).expect("signal pending commit");
        commit_rx.await.expect("await commit signal");
        tx.commit().await.expect("commit Artist change");
    });
    ready_rx.await.expect("Artist change holds candidates");
    let pool = db.pool.clone();
    let repair = tokio::spawn(async move {
        NativeDiscoveryStore
            .refresh_discovery_aggregates(&pool, 500, 100)
            .await
    });
    wait_for_lock_query(
        &db.pool,
        "SELECT entity_id FROM native_recommendation_candidates",
    )
    .await;
    commit_tx.send(()).expect("release Artist change");
    change.await.expect("join Artist change");
    repair
        .await
        .expect("join Artist repair")
        .expect("repair Artist popularity");
    let (old,new): (i64,i64) = sqlx::query_as("SELECT (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$1), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$2)")
        .bind(old_artist).bind(new_artist).fetch_one(&db.pool).await.expect("read Artist popularity");
    assert_eq!((old, new), (0, 2));
    db.drop().await;
}

#[tokio::test]
async fn concurrent_scrobbles_on_one_track_do_not_deadlock_or_lose_popularity() {
    use std::sync::Arc;
    use tokio::{sync::Barrier, task::JoinSet};

    const WORKERS: usize = 8;
    const PLAYS: usize = 4;
    let db = common::TestDb::create("concurrent_scrobbles").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7191,
        CatalogFixturePresentation {
            release_title: "Concurrent Plays",
            release_artist: "Concurrent Artist",
            track_title: "Concurrent Track",
            track_artist: "Concurrent Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let release_id = fixture.release_id().into_uuid();
    let (track_id, track_public, artist_id): (Uuid, Uuid, Uuid) = sqlx::query_as(
        "SELECT t.id,t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1",
    )
    .bind(release_id)
    .fetch_one(&db.pool)
    .await
    .expect("read Track and Artist");
    let mut users = Vec::new();
    for i in 0..WORKERS {
        users.push(insert_user(&db, &format!("concurrent-scrobble-{i}")).await);
    }
    let barrier = Arc::new(Barrier::new(WORKERS));
    let mut tasks = JoinSet::new();
    for (worker, user) in users.into_iter().enumerate() {
        let pool = db.pool.clone();
        let play_barrier = barrier.clone();
        tasks.spawn(async move {
            play_barrier.wait().await;
            for iteration in 0..PLAYS {
                let now = OffsetDateTime::now_utc();
                assert_eq!(
                    NativeUserStateStore::record_scrobble(
                        &pool,
                        user,
                        track_public,
                        &format!("concurrent-{worker}-{iteration}"),
                        now,
                        now,
                        Some(30),
                    )
                    .await
                    .expect("concurrent scrobble"),
                    NativeScrobbleResult::Applied
                );
            }
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(result) = tasks.join_next().await {
            result.expect("join scrobble worker");
        }
    })
    .await
    .expect("concurrent scrobbles complete");
    let counts: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM native_track_plays WHERE track_id=$1),
                (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='track' AND entity_id=$1),
                (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='release' AND entity_id=$2),
                (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$3)",
    )
    .bind(track_id).bind(release_id).bind(artist_id)
    .fetch_one(&db.pool).await.expect("read synchronous popularity");
    let expected = i64::try_from(WORKERS * PLAYS).unwrap();
    assert_eq!(counts, (expected, expected, expected, expected));
    db.drop().await;
}

#[tokio::test]
async fn bulk_plays_and_opposing_artist_changes_keep_recommendations_current() {
    use std::sync::Arc;
    use tokio::{sync::Barrier, task::JoinSet};

    let db = common::TestDb::create("recommendation_bulk").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7192,
        CatalogFixturePresentation {
            release_title: "Bulk One",
            release_artist: "Bulk Artist One",
            track_title: "Bulk Track One",
            track_artist: "Bulk Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7193,
        CatalogFixturePresentation {
            release_title: "Bulk Two",
            release_artist: "Bulk Artist Two",
            track_title: "Bulk Track Two",
            track_artist: "Bulk Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let releases = [
        first.release_id().into_uuid(),
        second.release_id().into_uuid(),
    ];
    let mut tracks = Vec::new();
    let mut artists = Vec::new();
    for release in releases {
        let (track, artist): (Uuid, Uuid) = sqlx::query_as("SELECT t.id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
            .bind(release).fetch_one(&db.pool).await.expect("read fixture identities");
        tracks.push(track);
        artists.push(artist);
    }
    let users = [
        insert_user(&db, "bulk-play-one").await,
        insert_user(&db, "bulk-play-two").await,
    ];
    let barrier = Arc::new(Barrier::new(4));
    let mut tasks = JoinSet::new();
    for worker in 0..2 {
        let pool = db.pool.clone();
        let play_barrier = barrier.clone();
        let ids = if worker == 0 {
            vec![tracks[0], tracks[1]]
        } else {
            vec![tracks[1], tracks[0]]
        };
        let user = users[worker];
        tasks.spawn(async move {
            play_barrier.wait().await;
            for iteration in 0..8 {
                sqlx::query("INSERT INTO native_track_plays(id,user_id,track_id,client_event_id,played_at,submitted_at,duration_seconds,completed) SELECT uuidv7(),$1,id,$3 || '-' || ordinal::text,now(),now(),30,true FROM unnest($2::uuid[]) WITH ORDINALITY rows(id,ordinal)")
                    .bind(user).bind(&ids).bind(format!("bulk-{worker}-{iteration}"))
                    .execute(&pool).await.expect("concurrent bulk play insert");
            }
        });
        let pool = db.pool.clone();
        let barrier = barrier.clone();
        let track = tracks[worker];
        let original = artists[worker];
        let other = artists[1 - worker];
        tasks.spawn(async move {
            barrier.wait().await;
            for iteration in 0..8 {
                let changed = sqlx::query(
                    "UPDATE track_artists SET artist_id=$2 WHERE track_id=$1 AND role='primary'",
                )
                .bind(track)
                .bind(if iteration % 2 == 0 { other } else { original })
                .execute(&pool)
                .await
                .expect("opposing primary Artist change");
                assert_eq!(changed.rows_affected(), 1);
            }
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(result) = tasks.join_next().await {
            result.expect("join bulk worker");
        }
    })
    .await
    .expect("bulk operations complete");

    // Give both Tracks both primary Artists, in opposite credit order, then
    // insert concurrent plays. Candidate locks must not follow credit order.
    for index in 0..2 {
        sqlx::query("INSERT INTO track_artists(id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) VALUES(uuidv7(),$1,$2,'primary',2,'Additional primary','',now())")
            .bind(tracks[index]).bind(artists[1-index]).execute(&db.pool).await.expect("add second primary Artist");
    }
    let barrier = Arc::new(Barrier::new(2));
    let mut tasks = JoinSet::new();
    for worker in 0..2 {
        let pool = db.pool.clone();
        let barrier = barrier.clone();
        let user = users[worker];
        let track = tracks[worker];
        tasks.spawn(async move {
            barrier.wait().await;
            for iteration in 0..8 {
                sqlx::query("INSERT INTO native_track_plays(id,user_id,track_id,client_event_id,played_at,submitted_at,duration_seconds,completed) VALUES(uuidv7(),$1,$2,$3,now(),now(),30,true)")
                    .bind(user).bind(track).bind(format!("multiple-primary-{worker}-{iteration}"))
                    .execute(&pool).await.expect("multiple primary Artist play");
            }
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(result) = tasks.join_next().await {
            result.expect("join primary worker");
        }
    })
    .await
    .expect("multiple-primary operations complete");

    let inconsistent: i64 = sqlx::query_scalar("SELECT count(*) FROM native_recommendation_candidates c WHERE (c.entity_type='track' AND c.entity_id=ANY($1) AND c.popularity<>(SELECT count(*) FROM native_track_plays p WHERE p.track_id=c.entity_id)) OR (c.entity_type='release' AND c.entity_id=ANY($2) AND c.popularity<>(SELECT count(*) FROM native_track_plays p JOIN tracks t ON t.id=p.track_id WHERE t.release_id=c.entity_id)) OR (c.entity_type='artist' AND c.entity_id=ANY($3) AND c.popularity<>(SELECT count(*) FROM native_track_plays p JOIN track_artists a ON a.track_id=p.track_id WHERE a.artist_id=c.entity_id AND a.role='primary'))")
        .bind(&tracks).bind(releases).bind(&artists).fetch_one(&db.pool).await.expect("compare recommendation popularity to plays");
    assert_eq!(inconsistent, 0);
    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE track_id=ANY($1)")
            .bind(&tracks)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(total, 48);
    db.drop().await;
}

#[tokio::test]
async fn opposing_release_moves_serialize_recommendation_candidates() {
    use std::sync::Arc;
    use tokio::{sync::Barrier, task::JoinSet};
    let db = common::TestDb::create("opposing_release_moves").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7194,
        CatalogFixturePresentation {
            release_title: "Move One",
            release_artist: "Move Artist One",
            track_title: "Move Track One",
            track_artist: "Move Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7195,
        CatalogFixturePresentation {
            release_title: "Move Two",
            release_artist: "Move Artist Two",
            track_title: "Move Track Two",
            track_artist: "Move Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let releases = [
        first.release_id().into_uuid(),
        second.release_id().into_uuid(),
    ];
    let mut tracks = Vec::new();
    for release in releases {
        let track: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE release_id=$1")
            .bind(release)
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
        tracks.push(track);
    }
    let user = insert_user(&db, "opposing-moves").await;
    for (index, track) in tracks.iter().enumerate() {
        sqlx::query("INSERT INTO native_track_plays(id,user_id,track_id,client_event_id,played_at,submitted_at,duration_seconds,completed) VALUES(uuidv7(),$1,$2,$3,now(),now(),30,true)")
            .bind(user).bind(track).bind(format!("move-play-{index}"))
            .execute(&db.pool).await.expect("seed play");
    }
    let barrier = Arc::new(Barrier::new(2));
    let mut tasks = JoinSet::new();
    for index in 0..2 {
        let pool = db.pool.clone();
        let barrier = barrier.clone();
        let track = tracks[index];
        let target = releases[1 - index];
        tasks.spawn(async move {
            let mut tx = pool.begin().await.expect("begin move");
            barrier.wait().await;
            sqlx::query("UPDATE tracks SET release_id=$2,ordinal=2 WHERE id=$1")
                .bind(track).bind(target).execute(&mut *tx).await.expect("move Track");
            sqlx::query("UPDATE observed_artists SET release_id=$2,source_ordinal=source_ordinal+100 WHERE track_id=$1")
                .bind(track).bind(target).execute(&mut *tx).await.expect("move observations");
            sqlx::query("UPDATE contributions SET release_id=$2 WHERE track_id=$1")
                .bind(track).bind(target).execute(&mut *tx).await.expect("move contributions");
            tx.commit().await.expect("commit move");
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(result) = tasks.join_next().await {
            result.expect("join move");
        }
    })
    .await
    .expect("opposing moves complete");
    for release in releases {
        let (actual, authoritative): (i64,i64) = sqlx::query_as("SELECT c.popularity,(SELECT count(*) FROM native_track_plays p JOIN tracks t ON t.id=p.track_id WHERE t.release_id=$1) FROM native_recommendation_candidates c WHERE c.entity_type='release' AND c.entity_id=$1")
            .bind(release).fetch_one(&db.pool).await.expect("compare release popularity");
        assert_eq!((actual, authoritative), (1, 1));
    }
    db.drop().await;
}

#[tokio::test]
async fn raw_play_waiting_for_artist_change_uses_committed_relationship() {
    let db = common::TestDb::create("raw_play_artist_race").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7196,
        CatalogFixturePresentation {
            release_title: "Race One",
            release_artist: "Race Artist One",
            track_title: "Race Track One",
            track_artist: "Race Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7197,
        CatalogFixturePresentation {
            release_title: "Race Two",
            release_artist: "Race Artist Two",
            track_title: "Race Track Two",
            track_artist: "Race Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let release = first.release_id().into_uuid();
    let (track, old_artist): (Uuid,Uuid) = sqlx::query_as("SELECT t.id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(release).fetch_one(&db.pool).await.expect("first identities");
    let new_artist: Uuid = sqlx::query_scalar("SELECT a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(second.release_id().into_uuid()).fetch_one(&db.pool).await.expect("second Artist");
    let user = insert_user(&db, "raw-race-user").await;
    let mut change = db.pool.begin().await.expect("begin relationship change");
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR NO KEY UPDATE")
        .bind(track)
        .execute(&mut *change)
        .await
        .expect("hold Track lock");
    let pool = db.pool.clone();
    let play = tokio::spawn(async move {
        sqlx::query("INSERT INTO native_track_plays(id,user_id,track_id,client_event_id,played_at,submitted_at,duration_seconds,completed) VALUES(uuidv7(),$1,$2,'raw-race',now(),now(),30,true)")
            .bind(user).bind(track).execute(&pool).await
    });
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'INSERT INTO native_track_plays%')")
                .fetch_one(&db.pool).await.expect("inspect blocked play");
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.expect("raw play waits on the relationship lock");
    sqlx::query("UPDATE track_artists SET artist_id=$2 WHERE track_id=$1 AND role='primary'")
        .bind(track)
        .bind(new_artist)
        .execute(&mut *change)
        .await
        .expect("change Artist while play waits");
    change.commit().await.expect("commit relationship change");
    tokio::time::timeout(std::time::Duration::from_secs(15), play)
        .await
        .expect("play resumes")
        .expect("join play")
        .expect("insert play");
    let result: (i64,i64,i64,i64) = sqlx::query_as("SELECT (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='track' AND entity_id=$1), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='release' AND entity_id=$2), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$3), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$4)")
        .bind(track).bind(release).bind(old_artist).bind(new_artist).fetch_one(&db.pool).await.expect("read projected popularity");
    assert_eq!(result, (1, 1, 0, 1));
    db.drop().await;
}

#[tokio::test]
async fn duplicate_primary_credits_match_recount_and_user_play_is_counted_once() {
    let db = common::TestDb::create("duplicate_primary_credits").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7200,
        CatalogFixturePresentation {
            release_title: "Repeated Credit",
            release_artist: "Repeated Artist",
            track_title: "Repeated Track",
            track_artist: "Repeated Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track,public,artist): (Uuid,Uuid,Uuid) = sqlx::query_as("SELECT t.id,t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(fixture.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read identities");
    let duplicate = Uuid::now_v7();
    sqlx::query("INSERT INTO track_artists(id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) VALUES($1,$2,$3,'primary',2,'Repeated Artist','',now())")
        .bind(duplicate).bind(track).bind(artist).execute(&db.pool).await.expect("add duplicate credit");
    let user = insert_user(&db, "repeated-primary-user").await;
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user,
            public,
            "duplicate-credit-play",
            now,
            now,
            Some(30)
        )
        .await
        .expect("record play with duplicate credit"),
        NativeScrobbleResult::Applied
    );
    let (artist_popularity,user_count): (i64,i64) = sqlx::query_as("SELECT (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$1), (SELECT play_count FROM native_artist_user_states WHERE artist_id=$1 AND user_id=$2)")
        .bind(artist).bind(user).fetch_one(&db.pool).await.expect("read counts");
    assert_eq!((artist_popularity, user_count), (2, 1));
    sqlx::query("UPDATE native_recommendation_candidates SET popularity=99 WHERE entity_type='artist' AND entity_id=$1")
        .bind(artist).execute(&db.pool).await.expect("introduce pre-existing drift");
    NativeDiscoveryStore
        .refresh_discovery_aggregates(&db.pool, 500, 100)
        .await
        .expect("repair drift from authoritative plays and credits");
    let repaired: i64 = sqlx::query_scalar("SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$1")
        .bind(artist).fetch_one(&db.pool).await.expect("read repaired popularity");
    assert_eq!(repaired, 2);
    sqlx::query("DELETE FROM track_artists WHERE id=$1")
        .bind(duplicate)
        .execute(&db.pool)
        .await
        .expect("remove duplicate credit");
    let after_delete: i64 = sqlx::query_scalar("SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$1")
        .bind(artist).fetch_one(&db.pool).await.expect("read popularity after deletion");
    assert_eq!(after_delete, 1);
    db.drop().await;
}

#[tokio::test]
async fn scrobble_retries_if_primary_artist_changes_during_parent_lock_acquisition() {
    let db = common::TestDb::create("scrobble_artist_lock_race").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7201,
        CatalogFixturePresentation {
            release_title: "Parent Race One",
            release_artist: "Parent Artist One",
            track_title: "Parent Track One",
            track_artist: "Parent Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7202,
        CatalogFixturePresentation {
            release_title: "Parent Race Two",
            release_artist: "Parent Artist Two",
            track_title: "Parent Track Two",
            track_artist: "Parent Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let (track, public, old_artist): (Uuid,Uuid,Uuid) = sqlx::query_as("SELECT t.id,t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(first.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read first identities");
    let new_artist: Uuid = sqlx::query_scalar("SELECT a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(second.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read second Artist");
    let user = insert_user(&db, "artist-parent-race").await;
    let mut change = db.pool.begin().await.expect("begin Artist change");
    sqlx::query("SELECT id FROM artists WHERE id=$1 FOR UPDATE")
        .bind(old_artist)
        .execute(&mut *change)
        .await
        .expect("lock old Artist before Track");
    let probes = scrobble_probe_count();
    let pool = db.pool.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "artist-parent-race",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_scrobble_probe(probes).await;
    sqlx::query("UPDATE track_artists SET artist_id=$2 WHERE track_id=$1 AND role='primary'")
        .bind(track)
        .bind(new_artist)
        .execute(&mut *change)
        .await
        .expect("change Artist before releasing parent");
    change.commit().await.expect("commit Artist change");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(15), play)
            .await
            .expect("play resumes")
            .expect("join play")
            .expect("record play"),
        NativeScrobbleResult::Applied
    );
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$1), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$2), (SELECT play_count FROM native_artist_user_states WHERE artist_id=$2 AND user_id=$3)")
        .bind(old_artist).bind(new_artist).bind(user).fetch_one(&db.pool).await.expect("read Artist counters");
    assert_eq!(counts, (0, 1, 1));
    db.drop().await;
}

#[tokio::test]
async fn scrobble_retries_if_release_changes_during_parent_lock_acquisition() {
    let db = common::TestDb::create("scrobble_release_lock_race").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7203,
        CatalogFixturePresentation {
            release_title: "Parent Release One",
            release_artist: "Parent Release Artist One",
            track_title: "Parent Release Track One",
            track_artist: "Parent Release Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7204,
        CatalogFixturePresentation {
            release_title: "Parent Release Two",
            release_artist: "Parent Release Artist Two",
            track_title: "Parent Release Track Two",
            track_artist: "Parent Release Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let original = first.release_id().into_uuid();
    let target = second.release_id().into_uuid();
    let (track, public): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(original)
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
    let user = insert_user(&db, "release-parent-race").await;
    let mut change = db.pool.begin().await.expect("begin Release change");
    sqlx::query("SELECT id FROM releases WHERE id=$1 FOR UPDATE")
        .bind(original)
        .execute(&mut *change)
        .await
        .expect("lock old Release before Track");
    let probes = scrobble_probe_count();
    let pool = db.pool.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "release-parent-race",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_scrobble_probe(probes).await;
    sqlx::query("UPDATE tracks SET release_id=$2,ordinal=2 WHERE id=$1")
        .bind(track)
        .bind(target)
        .execute(&mut *change)
        .await
        .expect("move Track");
    sqlx::query("UPDATE observed_artists SET release_id=$2,source_ordinal=source_ordinal+100 WHERE track_id=$1")
        .bind(track).bind(target).execute(&mut *change).await.expect("move observations");
    sqlx::query("UPDATE contributions SET release_id=$2 WHERE track_id=$1")
        .bind(track)
        .bind(target)
        .execute(&mut *change)
        .await
        .expect("move contributions");
    change.commit().await.expect("commit Release change");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(15), play)
            .await
            .expect("play resumes")
            .expect("join play")
            .expect("record play"),
        NativeScrobbleResult::Applied
    );
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='release' AND entity_id=$1), (SELECT popularity FROM native_recommendation_candidates WHERE entity_type='release' AND entity_id=$2), (SELECT play_count FROM native_release_user_states WHERE release_id=$2 AND user_id=$3)")
        .bind(original).bind(target).bind(user).fetch_one(&db.pool).await.expect("read Release counters");
    assert_eq!(counts, (0, 1, 1));
    db.drop().await;
}

#[tokio::test]
async fn scrobble_parent_credit_cursors_cover_more_than_one_page() {
    let db = common::TestDb::create("scrobble_credit_pages").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7205,
        CatalogFixturePresentation {
            release_title: "Many Credits",
            release_artist: "Existing Credit",
            track_title: "Many Credit Track",
            track_artist: "Existing Credit",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track, public): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
    sqlx::query("INSERT INTO artists(id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at) SELECT uuidv7(),uuidv7(),'Paged Artist ' || n,'paged artist ' || n,'Paged Artist ' || n,'group','verified',now(),now() FROM generate_series(1,300) n")
        .execute(&db.pool).await.expect("seed extra Artists");
    sqlx::query("INSERT INTO track_artists(id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) SELECT uuidv7(),$1,id,'primary',row_number() OVER (ORDER BY id)::int+1,canonical_name,'',now() FROM artists WHERE canonical_name LIKE 'Paged Artist %'")
        .bind(track).execute(&db.pool).await.expect("seed extra primary credits");
    let user = insert_user(&db, "paged-credit-user").await;
    let now = OffsetDateTime::now_utc();
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user,
            public,
            "paged-credits",
            now,
            now,
            Some(30)
        )
        .await
        .expect("record paged-credit play"),
        NativeScrobbleResult::Applied
    );
    let (count,plays): (i64,i64) = sqlx::query_as("SELECT count(*),coalesce(sum(play_count),0)::bigint FROM native_artist_user_states WHERE user_id=$1")
        .bind(user).fetch_one(&db.pool).await.expect("read paged user state");
    assert_eq!((count, plays), (301, 301));
    db.drop().await;
}

#[tokio::test]
async fn scrobble_succeeds_after_more_than_eight_parent_contentions() {
    let db = common::TestDb::create("scrobble_retry_exhaustion").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7210,
        CatalogFixturePresentation {
            release_title: "Retry Exhaustion",
            release_artist: "Existing Credit",
            track_title: "Retry Track",
            track_artist: "Existing Credit",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track, public): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
    sqlx::query("INSERT INTO artists(id,public_api_id,canonical_name,normalized_name,sort_name,artist_type,status,created_at,updated_at) SELECT uuidv7(),uuidv7(),'Retry Artist ' || n,'retry artist ' || n,'Retry Artist ' || n,'group','verified',now(),now() FROM generate_series(1,2048) n")
        .execute(&db.pool).await.expect("seed Artists");
    sqlx::query("INSERT INTO track_artists(id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at) SELECT uuidv7(),$1,id,'primary',row_number() OVER (ORDER BY id)::int+1,canonical_name,'',now() FROM artists WHERE canonical_name LIKE 'Retry Artist %'")
        .bind(track).execute(&db.pool).await.expect("seed credits");
    let contention_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(16)
        .connect(&common::database_url(&db.name))
        .await
        .expect("open contention pool");
    let mut reader = contention_pool.begin().await.expect("begin cursor reader");
    sqlx::query("DECLARE scrobble_artists NO SCROLL CURSOR FOR SELECT artist_id FROM track_artists WHERE track_id=$1 AND role='primary'")
        .bind(track).execute(&mut *reader).await.expect("declare cursor");
    let mut selected = Vec::new();
    for _ in 0..9 {
        let page: Vec<Uuid> = sqlx::query_scalar("FETCH FORWARD 256 FROM scrobble_artists")
            .fetch_all(&mut *reader)
            .await
            .expect("fetch page");
        selected.push(*page.first().expect("nine nonempty pages"));
    }
    reader.rollback().await.expect("close cursor");
    let mut blockers = Vec::new();
    for artist in selected {
        let mut blocker = contention_pool.begin().await.expect("begin parent blocker");
        sqlx::query("SELECT id FROM artists WHERE id=$1 FOR UPDATE")
            .bind(artist)
            .execute(&mut *blocker)
            .await
            .expect("lock parent");
        blockers.push(blocker);
    }
    let user = insert_user(&db, "retry-exhaustion").await;
    let mut probes = scrobble_probe_count();
    let pool = contention_pool.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "retry-exhaustion",
            now,
            now,
            Some(30),
        )
        .await
    });
    for blocker in blockers {
        wait_for_scrobble_probe(probes).await;
        assert!(!play.is_finished(), "locked parent must keep play pending");
        blocker.commit().await.expect("release parent");
        probes = scrobble_probe_count();
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(15), play)
        .await
        .expect("play exits")
        .expect("join play")
        .expect("play succeeds after contention");
    assert_eq!(result, NativeScrobbleResult::Applied);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id=$1")
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("count plays");
    assert_eq!(count, 1);
    contention_pool.close().await;
    db.drop().await;
}

#[tokio::test]
async fn cancelled_scrobble_releases_parent_wait_and_reuses_connection() {
    let db = common::TestDb::create("scrobble_cancel_wait").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7213,
        CatalogFixturePresentation {
            release_title: "Cancelled Play",
            release_artist: "Cancelled Artist",
            track_title: "Cancelled Track",
            track_artist: "Cancelled Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track,public,artist): (Uuid,Uuid,Uuid) = sqlx::query_as("SELECT t.id,t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(fixture.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read Track");
    let user = insert_user(&db, "cancelled-parent-wait").await;
    let mut blocker = db.pool.begin().await.expect("begin parent blocker");
    sqlx::query("SELECT id FROM artists WHERE id=$1 FOR UPDATE")
        .bind(artist)
        .execute(&mut *blocker)
        .await
        .expect("hold parent");
    let single_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&common::database_url(&db.name))
        .await
        .expect("open single-connection pool");
    let probes = scrobble_probe_count();
    let pool = single_pool.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "cancelled-parent-wait",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_scrobble_probe(probes).await;
    play.abort();
    assert!(play.await.expect_err("play cancelled").is_cancelled());
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(track)
        .execute(&mut *blocker)
        .await
        .expect("cancelled play does not retain Track");
    let value: i32 = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        sqlx::query_scalar("SELECT 1").fetch_one(&single_pool),
    )
    .await
    .expect("single connection returns to pool")
    .expect("reuse connection");
    assert_eq!(value, 1);
    blocker.rollback().await.expect("release parent");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id=$1")
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("count plays");
    assert_eq!(count, 0);
    single_pool.close().await;
    db.drop().await;
}

#[tokio::test]
async fn scrobble_succeeds_after_sustained_parent_contention() {
    let db = common::TestDb::create("scrobble_long_parent").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7214,
        CatalogFixturePresentation {
            release_title: "Long Wait Play",
            release_artist: "Long Wait Artist",
            track_title: "Long Wait Track",
            track_artist: "Long Wait Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (public,artist): (Uuid,Uuid) = sqlx::query_as("SELECT t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(fixture.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read Track");
    let user = insert_user(&db, "long-parent-wait").await;
    let mut blocker = db.pool.begin().await.expect("begin blocker");
    sqlx::query("SELECT id FROM artists WHERE id=$1 FOR UPDATE")
        .bind(artist)
        .execute(&mut *blocker)
        .await
        .expect("hold parent");
    let probes = scrobble_probe_count();
    let pool = db.pool.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "long-parent-wait",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_scrobble_probe(probes).await;
    tokio::time::sleep(std::time::Duration::from_millis(2300)).await;
    assert!(!play.is_finished(), "contention must not fail the play");
    blocker.rollback().await.expect("release parent");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(15), play)
            .await
            .expect("play finishes")
            .expect("join")
            .expect("record play"),
        NativeScrobbleResult::Applied
    );
    db.drop().await;
}

#[tokio::test]
async fn contended_scrobbles_leave_small_pools_available_for_heartbeat() {
    for max_connections in [1, 2, 3] {
        let db = common::TestDb::create("scrobble_probe_pool").await;
        let fixture = phase5_common::seed_cataloged_release(
            &db,
            7215,
            CatalogFixturePresentation {
                release_title: "Probe Pool",
                release_artist: "Probe Artist",
                track_title: "Probe Track",
                track_artist: "Probe Artist",
                release_date: "2026-07-01",
            },
        )
        .await;
        let (public,artist): (Uuid,Uuid) = sqlx::query_as("SELECT t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
            .bind(fixture.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read Track");
        let user = insert_user(&db, &format!("probe-pool-{max_connections}")).await;
        let worker = melodee_persistence::WorkerId::new();
        let config = common::test_config();
        let now = OffsetDateTime::now_utc();
        melodee_persistence::PolicyStore
            .claim_cataloged(&db.pool, &worker, &config, now)
            .await
            .expect("claim heartbeat fixture");
        let mut blocker = db.pool.begin().await.expect("begin parent blocker");
        sqlx::query("SELECT id FROM artists WHERE id=$1 FOR UPDATE")
            .bind(artist)
            .execute(&mut *blocker)
            .await
            .expect("hold parent");
        let limited = sqlx::postgres::PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(&common::database_url(&db.name))
            .await
            .expect("open small pool");
        let probes = scrobble_probe_count();
        let mut plays = Vec::new();
        for ordinal in 0..6 {
            let pool = limited.clone();
            plays.push(tokio::spawn(async move {
                let now = OffsetDateTime::now_utc();
                NativeUserStateStore::record_scrobble(
                    &pool,
                    user,
                    public,
                    &format!("probe-{ordinal}"),
                    now,
                    now,
                    Some(30),
                )
                .await
            }));
        }
        wait_for_scrobble_probe(probes).await;
        let value: i32 = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            sqlx::query_scalar("SELECT 1").fetch_one(&limited),
        )
        .await
        .expect("interactive query acquires pool")
        .expect("interactive query");
        assert_eq!(value, 1);
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            melodee_persistence::PolicyStore.renew_lease(
                &limited,
                fixture.work_item_id(),
                &worker,
                time::Duration::seconds(300),
                OffsetDateTime::now_utc(),
            ),
        )
        .await
        .expect("heartbeat acquires pool")
        .expect("renew heartbeat");
        for play in plays.iter().take(3) {
            play.abort();
        }
        blocker.rollback().await.expect("release parent");
        for (index, play) in plays.into_iter().enumerate() {
            if index < 3 {
                assert!(play.await.expect_err("cancelled").is_cancelled());
            } else {
                assert_eq!(
                    tokio::time::timeout(std::time::Duration::from_secs(15), play)
                        .await
                        .expect("play completes")
                        .expect("join")
                        .expect("record play"),
                    NativeScrobbleResult::Applied
                );
            }
        }
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id=$1")
                .bind(user)
                .fetch_one(&db.pool)
                .await
                .expect("count plays");
        assert_eq!(count, 3);
        limited.close().await;
        db.drop().await;
    }
}

#[tokio::test]
async fn stalled_track_probes_allow_cancellation_and_eventual_play() {
    let db = common::TestDb::create("scrobble_track_stall").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7217,
        CatalogFixturePresentation {
            release_title: "Stalled Track",
            release_artist: "Stall Artist",
            track_title: "Stall Track",
            track_artist: "Stall Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track, public): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
    let user = insert_user(&db, "stalled-track").await;
    let mut blocker = db.pool.begin().await.expect("begin blocker");
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR UPDATE")
        .bind(track)
        .execute(&mut *blocker)
        .await
        .expect("hold Track");
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&common::database_url(&db.name))
        .await
        .expect("open small pool");
    let probes = scrobble_probe_count();
    let cancelled_pool = limited.clone();
    let cancelled = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &cancelled_pool,
            user,
            public,
            "cancelled-track",
            now,
            now,
            Some(30),
        )
        .await
    });
    let eventual_pool = limited.clone();
    let eventual = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &eventual_pool,
            user,
            public,
            "eventual-track",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_scrobble_probe(probes).await;
    cancelled.abort();
    assert!(cancelled.await.expect_err("cancelled").is_cancelled());
    let value: i32 = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        sqlx::query_scalar("SELECT 1").fetch_one(&limited),
    )
    .await
    .expect("pool remains available")
    .expect("query");
    assert_eq!(value, 1);
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(
        !eventual.is_finished(),
        "held Track must leave play pending"
    );
    blocker.rollback().await.expect("release Track");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(15), eventual)
            .await
            .expect("play finishes")
            .expect("join")
            .expect("record play"),
        NativeScrobbleResult::Applied
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id=$1")
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("count plays");
    assert_eq!(count, 1);
    limited.close().await;
    db.drop().await;
}

#[tokio::test]
async fn cancelled_track_probe_never_publishes() {
    let db = common::TestDb::create("fifo_track_cancel").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7220,
        CatalogFixturePresentation {
            release_title: "FIFO Cancel",
            release_artist: "FIFO Artist",
            track_title: "FIFO Track",
            track_artist: "FIFO Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track, public): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
    let user = insert_user(&db, "fifo-cancel").await;
    let mut blocker = db.pool.begin().await.expect("begin blocker");
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR UPDATE")
        .bind(track)
        .execute(&mut *blocker)
        .await
        .expect("hold Track");
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&common::database_url(&db.name))
        .await
        .expect("open pool");
    let pool = limited.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "fifo-cancel",
            now,
            now,
            Some(30),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!play.is_finished(), "held Track must leave play pending");
    play.abort();
    assert!(play.await.expect_err("cancelled").is_cancelled());
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    blocker.rollback().await.expect("release Track");
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM native_track_plays WHERE user_id=$1")
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .expect("count plays");
    assert_eq!(count, 0, "detached read-only waiter cannot publish");
    limited.close().await;
    db.drop().await;
}

#[tokio::test]
async fn track_probe_preserves_session_timeout_for_counter_write() {
    let db = common::TestDb::create("fifo_track_timeout").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7221,
        CatalogFixturePresentation {
            release_title: "FIFO Timeout",
            release_artist: "FIFO Timeout Artist",
            track_title: "FIFO Timeout Track",
            track_artist: "FIFO Timeout Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track, public): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
    let user = insert_user(&db, "fifo-timeout").await;
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET lock_timeout='2s'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&common::database_url(&db.name))
        .await
        .expect("open pool");
    let now = OffsetDateTime::now_utc();
    NativeUserStateStore::record_scrobble(&limited, user, public, "fifo-first", now, now, Some(30))
        .await
        .expect("seed play");
    let mut track_blocker = db.pool.begin().await.expect("Track blocker");
    let mut state_blocker = db.pool.begin().await.expect("state blocker");
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR UPDATE")
        .bind(track)
        .execute(&mut *track_blocker)
        .await
        .expect("hold Track");
    sqlx::query(
        "SELECT user_id FROM native_track_user_states WHERE user_id=$1 AND track_id=$2 FOR UPDATE",
    )
    .bind(user)
    .bind(track)
    .execute(&mut *state_blocker)
    .await
    .expect("hold counter");
    let pool = limited.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "fifo-second",
            now,
            now,
            Some(30),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!play.is_finished(), "held Track must leave play pending");
    track_blocker.rollback().await.expect("release Track");
    wait_for_lock_query(&db.pool, "INSERT INTO native_track_user_states").await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !play.is_finished(),
        "probe timeout must not leak into counter writes"
    );
    state_blocker.rollback().await.expect("release counter");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), play)
            .await
            .expect("play finishes")
            .expect("join")
            .expect("record play"),
        NativeScrobbleResult::Applied
    );
    limited.close().await;
    db.drop().await;
}

#[tokio::test]
async fn reverse_submission_order_preserves_user_state_timestamps_and_replay() {
    let db = common::TestDb::create("scrobble_reverse_time").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7216,
        CatalogFixturePresentation {
            release_title: "Reverse Time",
            release_artist: "Reverse Artist",
            track_title: "Reverse Track",
            track_artist: "Reverse Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let public: Uuid = sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
        .bind(fixture.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read Track");
    let user = insert_user(&db, "reverse-time").await;
    let newer = OffsetDateTime::now_utc();
    let older = newer - time::Duration::seconds(10);
    for (event, when) in [("newer", newer), ("older", older)] {
        assert_eq!(
            NativeUserStateStore::record_scrobble(
                &db.pool,
                user,
                public,
                event,
                when,
                when,
                Some(30)
            )
            .await
            .expect("record play"),
            NativeScrobbleResult::Applied
        );
    }
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user,
            public,
            "newer",
            newer,
            newer,
            Some(30)
        )
        .await
        .expect("replay play"),
        NativeScrobbleResult::Adopted
    );
    for table in [
        "native_track_user_states",
        "native_release_user_states",
        "native_artist_user_states",
    ] {
        let query = format!(
            "SELECT play_count,last_played_at,created_at,updated_at FROM {table} WHERE user_id=$1"
        );
        let (count, last, created, updated): (i64, OffsetDateTime, OffsetDateTime, OffsetDateTime) =
            sqlx::query_as(sqlx::AssertSqlSafe(query))
                .bind(user)
                .fetch_one(&db.pool)
                .await
                .expect("read state");
        assert_eq!(count, 2);
        assert!((last - newer).abs() < time::Duration::microseconds(1));
        assert!(updated >= created);
    }
    db.drop().await;
}

#[tokio::test]
async fn scrobble_preserves_session_lock_timeout_for_counter_writes() {
    let db = common::TestDb::create("scrobble_timeout_restore").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7218,
        CatalogFixturePresentation {
            release_title: "Timeout Restore",
            release_artist: "Timeout Artist",
            track_title: "Timeout Track",
            track_artist: "Timeout Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let (track, public): (Uuid, Uuid) =
        sqlx::query_as("SELECT id,public_api_id FROM tracks WHERE release_id=$1")
            .bind(fixture.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("read Track");
    let user = insert_user(&db, "timeout-restore").await;
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&common::database_url(&db.name))
        .await
        .expect("open single pool");
    sqlx::query("SET lock_timeout='2s'")
        .execute(&limited)
        .await
        .expect("set session timeout");
    let now = OffsetDateTime::now_utc();
    NativeUserStateStore::record_scrobble(
        &limited,
        user,
        public,
        "timeout-first",
        now,
        now,
        Some(30),
    )
    .await
    .expect("seed play");
    let mut blocker = db.pool.begin().await.expect("begin blocker");
    sqlx::query(
        "SELECT user_id FROM native_track_user_states WHERE user_id=$1 AND track_id=$2 FOR UPDATE",
    )
    .bind(user)
    .bind(track)
    .execute(&mut *blocker)
    .await
    .expect("lock counter");
    let pool = limited.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "timeout-second",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_lock_query(&db.pool, "INSERT INTO native_track_user_states").await;
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    assert!(
        !play.is_finished(),
        "binding must preserve the configured counter-write timeout"
    );
    blocker.commit().await.expect("release counter");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(10), play)
            .await
            .expect("play finishes")
            .expect("join")
            .expect("record play"),
        NativeScrobbleResult::Applied
    );
    let timeout: String = sqlx::query_scalar("SELECT current_setting('lock_timeout')")
        .fetch_one(&limited)
        .await
        .expect("read session timeout");
    assert_eq!(timeout, "2s");
    limited.close().await;
    db.drop().await;
}

#[tokio::test]
async fn scrobble_table_lock_probes_preserve_small_pool_heartbeat_and_play() {
    let db = common::TestDb::create("scrobble_table_lock").await;
    let fixture = phase5_common::seed_cataloged_release(
        &db,
        7219,
        CatalogFixturePresentation {
            release_title: "Locked Credits",
            release_artist: "Locked Artist",
            track_title: "Locked Track",
            track_artist: "Locked Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let public: Uuid = sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id=$1")
        .bind(fixture.release_id().into_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("read Track");
    let user = insert_user(&db, "binding-table-lock").await;
    let worker = melodee_persistence::WorkerId::new();
    let config = common::test_config();
    melodee_persistence::PolicyStore
        .claim_cataloged(&db.pool, &worker, &config, OffsetDateTime::now_utc())
        .await
        .expect("claim heartbeat fixture");
    let mut blocker = db.pool.begin().await.expect("begin blocker");
    sqlx::query("LOCK TABLE track_artists IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .expect("hold credit table");
    let limited = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&common::database_url(&db.name))
        .await
        .expect("open small pool");
    let probes = scrobble_probe_count();
    let pool = limited.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &pool,
            user,
            public,
            "binding-table-lock",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_scrobble_probe(probes).await;
    let value: i32 = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        sqlx::query_scalar("SELECT 1").fetch_one(&limited),
    )
    .await
    .expect("interactive query acquires pool")
    .expect("query");
    assert_eq!(value, 1);
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        melodee_persistence::PolicyStore.renew_lease(
            &limited,
            fixture.work_item_id(),
            &worker,
            time::Duration::seconds(300),
            OffsetDateTime::now_utc(),
        ),
    )
    .await
    .expect("heartbeat acquires pool")
    .expect("renew heartbeat");
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    assert!(
        !play.is_finished(),
        "table lock contention must not drop the play"
    );
    blocker.rollback().await.expect("release table lock");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(15), play)
            .await
            .expect("play finishes")
            .expect("join")
            .expect("record play"),
        NativeScrobbleResult::Applied
    );
    limited.close().await;
    db.drop().await;
}

#[tokio::test]
async fn forward_credit_cursor_preserves_snapshot_and_closes_on_rollback() {
    let db = common::TestDb::create("scroll_credit_snapshot").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7206,
        CatalogFixturePresentation {
            release_title: "Snapshot One",
            release_artist: "Snapshot Artist One",
            track_title: "Snapshot Track One",
            track_artist: "Snapshot Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7207,
        CatalogFixturePresentation {
            release_title: "Snapshot Two",
            release_artist: "Snapshot Artist Two",
            track_title: "Snapshot Track Two",
            track_artist: "Snapshot Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let (track,old_artist): (Uuid,Uuid) = sqlx::query_as("SELECT t.id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(first.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read original credit");
    let new_artist: Uuid = sqlx::query_scalar("SELECT a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(second.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read alternate Artist");
    let mut connection = db.pool.acquire().await.expect("acquire connection");
    let mut reader = sqlx::Acquire::begin(&mut *connection)
        .await
        .expect("begin cursor reader");
    sqlx::query("DECLARE scrobble_artists NO SCROLL CURSOR FOR SELECT artist_id FROM track_artists WHERE track_id=$1 AND role='primary'")
        .bind(track)
        .execute(&mut *reader)
        .await
        .expect("declare cursor");
    sqlx::query("UPDATE track_artists SET artist_id=$2 WHERE track_id=$1 AND role='primary'")
        .bind(track)
        .bind(new_artist)
        .execute(&db.pool)
        .await
        .expect("commit different primary Artist");
    let retained: Vec<Uuid> = sqlx::query_scalar("FETCH FORWARD 256 FROM scrobble_artists")
        .fetch_all(&mut *reader)
        .await
        .expect("fetch original cursor");
    assert_eq!(retained, vec![old_artist]);
    sqlx::query("DECLARE current_test NO SCROLL CURSOR FOR SELECT DISTINCT artist_id FROM track_artists WHERE track_id=$1 AND role='primary' ORDER BY artist_id")
        .bind(track).execute(&mut *reader).await.expect("declare fresh cursor");
    let current: Vec<Uuid> = sqlx::query_scalar("FETCH FORWARD 256 FROM current_test")
        .fetch_all(&mut *reader)
        .await
        .expect("fetch current cursor");
    assert_eq!(current, vec![new_artist]);
    reader.rollback().await.expect("close cursors");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_cursors WHERE name IN ('scrobble_artists','current_test')",
    )
    .fetch_one(&mut *connection)
    .await
    .expect("inspect cursor cleanup");
    assert_eq!(remaining, 0);
    drop(connection);
    db.drop().await;
}

#[tokio::test]
async fn scrobble_and_pending_credit_write_serialize_without_lost_popularity() {
    let db = common::TestDb::create("scrobble_pending_credit").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7211,
        CatalogFixturePresentation {
            release_title: "Pending Credit One",
            release_artist: "Pending Artist One",
            track_title: "Pending Track One",
            track_artist: "Pending Artist One",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7212,
        CatalogFixturePresentation {
            release_title: "Pending Credit Two",
            release_artist: "Pending Artist Two",
            track_title: "Pending Track Two",
            track_artist: "Pending Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let (track,public,old): (Uuid,Uuid,Uuid) = sqlx::query_as("SELECT t.id,t.public_api_id,a.artist_id FROM tracks t JOIN track_artists a ON a.track_id=t.id AND a.role='primary' WHERE t.release_id=$1")
        .bind(first.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read Track");
    let new: Uuid = sqlx::query_scalar("SELECT a.artist_id FROM track_artists a JOIN tracks t ON t.id=a.track_id WHERE t.release_id=$1 AND a.role='primary'")
        .bind(second.release_id().into_uuid()).fetch_one(&db.pool).await.expect("read new Artist");
    let user = insert_user(&db, "pending-credit").await;
    let mut holder = db.pool.begin().await.expect("begin Track blocker");
    sqlx::query("SELECT id FROM tracks WHERE id=$1 FOR UPDATE")
        .bind(track)
        .execute(&mut *holder)
        .await
        .expect("hold Track");
    let writer_pool = db.pool.clone();
    let writer = tokio::spawn(async move {
        let mut tx = writer_pool.begin().await?;
        sqlx::query("UPDATE track_artists SET artist_id=$2 WHERE track_id=$1 AND role='primary'")
            .bind(track)
            .bind(new)
            .execute(&mut *tx)
            .await?;
        tx.commit().await
    });
    wait_for_lock_query(&db.pool, "UPDATE track_artists SET artist_id").await;
    let probes = scrobble_probe_count();
    let play_pool = db.pool.clone();
    let play = tokio::spawn(async move {
        let now = OffsetDateTime::now_utc();
        NativeUserStateStore::record_scrobble(
            &play_pool,
            user,
            public,
            "pending-credit",
            now,
            now,
            Some(30),
        )
        .await
    });
    wait_for_scrobble_probe(probes).await;
    holder.commit().await.expect("release Track");
    let (written, played) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        (
            writer.await.expect("writer join"),
            play.await.expect("play join"),
        )
    })
    .await
    .expect("both operations finish");
    written.expect("commit credit change");
    assert_eq!(played.expect("record play"), NativeScrobbleResult::Applied);
    let counts: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM native_track_plays WHERE track_id=$1),(SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$2),(SELECT popularity FROM native_recommendation_candidates WHERE entity_type='artist' AND entity_id=$3)")
        .bind(track).bind(old).bind(new).fetch_one(&db.pool).await.expect("read play and popularity");
    assert_eq!(counts, (1, 0, 1));
    db.drop().await;
}

#[tokio::test]
async fn annotations_playlists_and_scrobbles_are_bounded_atomic_and_idempotent() {
    let db = common::TestDb::create("phase7_user_state").await;
    let first = phase5_common::seed_cataloged_release(
        &db,
        7101,
        CatalogFixturePresentation {
            release_title: "Phase 7 One",
            release_artist: "Phase 7 Artist",
            track_title: "Phase 7 Track One",
            track_artist: "Phase 7 Artist",
            release_date: "2026-07-01",
        },
    )
    .await;
    let second = phase5_common::seed_cataloged_release(
        &db,
        7102,
        CatalogFixturePresentation {
            release_title: "Phase 7 Two",
            release_artist: "Phase 7 Artist Two",
            track_title: "Phase 7 Track Two",
            track_artist: "Phase 7 Artist Two",
            release_date: "2026-07-02",
        },
    )
    .await;
    let user_id = insert_user(&db, "phase7-user").await;
    let now = OffsetDateTime::now_utc();

    let (artist_public_id, track_public_id): (Uuid, Uuid) = sqlx::query_as(
        r#"
        SELECT a.public_api_id, t.public_api_id
        FROM tracks t
        JOIN track_artists ta ON ta.track_id = t.id AND ta.role = 'primary'
        JOIN artists a ON a.id = ta.artist_id
        WHERE t.release_id = $1
        "#,
    )
    .bind(first.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("first public IDs");
    let second_track_public_id: Uuid =
        sqlx::query_scalar("SELECT public_api_id FROM tracks WHERE release_id = $1")
            .bind(second.release_id().into_uuid())
            .fetch_one(&db.pool)
            .await
            .expect("second Track public ID");

    let artist_state = NativeUserStateStore::mutate_artist(
        &db.pool,
        user_id,
        artist_public_id,
        NativeUserStateMutation::Starred(true),
        now,
    )
    .await
    .expect("star Artist");
    assert!(artist_state.starred);
    let artist_state = NativeUserStateStore::mutate_artist(
        &db.pool,
        user_id,
        artist_public_id,
        NativeUserStateMutation::Hated(true),
        now,
    )
    .await
    .expect("hate Artist");
    assert!(artist_state.hated);
    assert!(!artist_state.starred);
    let release_state = NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        first.release_id().into_uuid(),
        NativeUserStateMutation::Rating(5),
        now,
    )
    .await
    .expect("rate Release");
    assert_eq!(release_state.rating, 5);
    let release_state = NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        first.release_id().into_uuid(),
        NativeUserStateMutation::Starred(true),
        now,
    )
    .await
    .expect("star Release");
    assert!(release_state.starred);
    let release_state = NativeUserStateStore::mutate_release(
        &db.pool,
        user_id,
        first.release_id().into_uuid(),
        NativeUserStateMutation::Hated(true),
        now,
    )
    .await
    .expect("hate Release");
    assert!(release_state.hated);
    assert!(!release_state.starred);
    let track_state = NativeUserStateStore::mutate_track(
        &db.pool,
        user_id,
        track_public_id,
        NativeUserStateMutation::Hated(true),
        now,
    )
    .await
    .expect("hate Track");
    assert!(track_state.hated);
    let track_state = NativeUserStateStore::mutate_track(
        &db.pool,
        user_id,
        track_public_id,
        NativeUserStateMutation::Starred(true),
        now,
    )
    .await
    .expect("star Track");
    assert!(track_state.starred);
    assert!(!track_state.hated);
    assert!(matches!(
        NativeUserStateStore::mutate_track(
            &db.pool,
            user_id,
            track_public_id,
            NativeUserStateMutation::Rating(6),
            now,
        )
        .await,
        Err(NativeUserStateStoreError::InvalidRequest(_))
    ));

    let playlist = NativeUserStateStore::create_playlist(
        &db.pool,
        user_id,
        &NativePlaylistCreate {
            public_api_id: None,
            name: "Phase 7 Playlist".to_string(),
            description: Some("ordered acceptance".to_string()),
            is_public: false,
            track_public_ids: vec![track_public_id, second_track_public_id],
        },
        now,
    )
    .await
    .expect("create playlist");
    NativeUserStateStore::append_playlist_tracks(
        &db.pool,
        user_id,
        playlist.public_api_id,
        &[track_public_id],
        now,
    )
    .await
    .expect("append duplicate Track without legacy null failure");
    NativeUserStateStore::reorder_playlist_tracks(
        &db.pool,
        user_id,
        playlist.public_api_id,
        &[second_track_public_id, track_public_id],
        now,
    )
    .await
    .expect("set-based partial reorder");
    let items = NativeUserStateStore::playlist_items(&db.pool, user_id, playlist.public_api_id)
        .await
        .expect("list playlist items");
    assert_eq!(
        items
            .iter()
            .map(|item| item.track_public_id)
            .collect::<Vec<_>>(),
        vec![second_track_public_id, track_public_id]
    );

    let played_at = now - time::Duration::seconds(30);
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user_id,
            track_public_id,
            "phase7-event-1",
            played_at,
            now,
            Some(30),
        )
        .await
        .expect("record scrobble"),
        NativeScrobbleResult::Applied
    );
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user_id,
            track_public_id,
            "phase7-event-1",
            played_at,
            now,
            Some(30),
        )
        .await
        .expect("adopt exact replay"),
        NativeScrobbleResult::Adopted
    );
    let play_count: i64 = sqlx::query_scalar(
        "SELECT play_count FROM native_track_user_states s JOIN tracks t ON t.id = s.track_id WHERE s.user_id = $1 AND t.public_api_id = $2",
    )
    .bind(user_id)
    .bind(track_public_id)
    .fetch_one(&db.pool)
    .await
    .expect("read Track play count");
    assert_eq!(play_count, 1, "exact retry never double-counts");
    let popularity: i64 = sqlx::query_scalar(
        "SELECT popularity FROM native_recommendation_candidates candidate
         JOIN tracks track ON track.id=candidate.entity_id
         WHERE candidate.entity_type='track' AND track.public_api_id=$1",
    )
    .bind(track_public_id)
    .fetch_one(&db.pool)
    .await
    .expect("materialized recommendation popularity");
    assert_eq!(popularity, 1, "exact replay never double-counts popularity");

    let source_track_id: Uuid = sqlx::query_scalar("SELECT id FROM tracks WHERE public_api_id=$1")
        .bind(track_public_id)
        .fetch_one(&db.pool)
        .await
        .expect("Track internal ID");
    let first_artist_id: Uuid = sqlx::query_scalar(
        "SELECT artist_id FROM track_artists WHERE track_id=$1 AND role='primary'",
    )
    .bind(source_track_id)
    .fetch_one(&db.pool)
    .await
    .expect("first primary Artist");
    let second_artist_id: Uuid = sqlx::query_scalar(
        "SELECT billing.artist_id FROM track_artists billing
         JOIN tracks track ON track.id=billing.track_id
         WHERE track.release_id=$1 AND billing.role='primary'",
    )
    .bind(second.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("second primary Artist");
    let (audio_identity_id, media_asset_id): (Uuid, Uuid) =
        sqlx::query_as("SELECT audio_identity_id,playable_media_asset_id FROM tracks WHERE id=$1")
            .bind(source_track_id)
            .fetch_one(&db.pool)
            .await
            .expect("source Track media facts");
    let reassigned_track_id = Uuid::now_v7();
    let reassigned_track_public_id = Uuid::now_v7();
    let mut tx = db
        .pool
        .begin()
        .await
        .expect("reassignment fixture transaction");
    sqlx::query(
        "INSERT INTO tracks(
           id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,
           ordinal,title,normalized_title,sort_title,created_at,updated_at)
         VALUES($1,$2,$3,$4,$5,2,'Reassigned Track','REASSIGNED TRACK','Reassigned Track',now(),now())",
    )
    .bind(reassigned_track_id)
    .bind(reassigned_track_public_id)
    .bind(first.release_id().into_uuid())
    .bind(audio_identity_id)
    .bind(media_asset_id)
    .execute(&mut *tx)
    .await
    .expect("insert reassignment Track");
    sqlx::query(
        "INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at)
         VALUES($1,$2,'playable',now())",
    )
    .bind(reassigned_track_id)
    .bind(media_asset_id)
    .execute(&mut *tx)
    .await
    .expect("bind reassignment media");
    sqlx::query(
        "INSERT INTO track_artists(
           id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at)
         VALUES($1,$2,$3,'primary',1,'Phase 7 Artist','',now())",
    )
    .bind(Uuid::now_v7())
    .bind(reassigned_track_id)
    .bind(first_artist_id)
    .execute(&mut *tx)
    .await
    .expect("bind reassignment Artist");
    tx.commit().await.expect("commit reassignment Track");
    sqlx::query(
        "INSERT INTO native_track_plays(
           id,user_id,track_id,client_event_id,played_at,submitted_at,completed)
         VALUES($1,$2,$3,'relationship-reassignment',now(),now(),true)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(reassigned_track_id)
    .execute(&db.pool)
    .await
    .expect("record reassignment popularity");

    sqlx::query("UPDATE tracks SET release_id=$2 WHERE id=$1")
        .bind(reassigned_track_id)
        .bind(second.release_id().into_uuid())
        .execute(&db.pool)
        .await
        .expect("reassign Track Release");
    let release_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$2)",
    )
    .bind(first.release_id().into_uuid())
    .bind(second.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("reassigned Release popularity");
    assert_eq!(release_popularity, (1, 1));

    sqlx::query(
        "UPDATE track_artists SET artist_id=$2,display_value='Phase 7 Artist Two'
         WHERE track_id=$1 AND role='primary'",
    )
    .bind(reassigned_track_id)
    .bind(second_artist_id)
    .execute(&db.pool)
    .await
    .expect("reassign primary Artist");
    let artist_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$2)",
    )
    .bind(first_artist_id)
    .bind(second_artist_id)
    .fetch_one(&db.pool)
    .await
    .expect("reassigned Artist popularity");
    assert_eq!(artist_popularity, (1, 1));

    let concurrent_track_id = Uuid::now_v7();
    let concurrent_track_public_id = Uuid::now_v7();
    let mut fixture_tx = db.pool.begin().await.expect("concurrent Track fixture");
    sqlx::query(
        "INSERT INTO tracks(
           id,public_api_id,release_id,audio_identity_id,playable_media_asset_id,
           ordinal,title,normalized_title,sort_title,created_at,updated_at)
         VALUES($1,$2,$3,$4,$5,3,'Concurrent Reassignment Track',
           'CONCURRENT REASSIGNMENT TRACK','Concurrent Reassignment Track',now(),now())",
    )
    .bind(concurrent_track_id)
    .bind(concurrent_track_public_id)
    .bind(first.release_id().into_uuid())
    .bind(audio_identity_id)
    .bind(media_asset_id)
    .execute(&mut *fixture_tx)
    .await
    .expect("insert concurrent Track");
    sqlx::query(
        "INSERT INTO track_media_assets(track_id,media_asset_id,purpose,created_at)
         VALUES($1,$2,'playable',now())",
    )
    .bind(concurrent_track_id)
    .bind(media_asset_id)
    .execute(&mut *fixture_tx)
    .await
    .expect("bind concurrent Track media");
    sqlx::query(
        "INSERT INTO track_artists(
           id,track_id,artist_id,role,presentation_order,display_value,join_phrase,created_at)
         VALUES($1,$2,$3,'primary',1,'Phase 7 Artist','',now())",
    )
    .bind(Uuid::now_v7())
    .bind(concurrent_track_id)
    .bind(first_artist_id)
    .execute(&mut *fixture_tx)
    .await
    .expect("bind concurrent Track Artist");
    fixture_tx.commit().await.expect("commit concurrent Track");

    let mut relationship_tx = db.pool.begin().await.expect("relationship transaction");
    sqlx::query("UPDATE tracks SET release_id=$2 WHERE id=$1")
        .bind(concurrent_track_id)
        .bind(second.release_id().into_uuid())
        .execute(&mut *relationship_tx)
        .await
        .expect("move concurrent Track Release");
    sqlx::query(
        "UPDATE track_artists SET artist_id=$2,display_value='Phase 7 Artist Two'
         WHERE track_id=$1 AND role='primary'",
    )
    .bind(concurrent_track_id)
    .bind(second_artist_id)
    .execute(&mut *relationship_tx)
    .await
    .expect("move concurrent primary Artist");

    let play_pool = db.pool.clone();
    let mut play_task = tokio::spawn(async move {
        sqlx::query(
            "INSERT INTO native_track_plays(
               id,user_id,track_id,client_event_id,played_at,submitted_at,completed)
             VALUES($1,$2,$3,'concurrent-relationship-reassignment',now(),now(),true)",
        )
        .bind(Uuid::now_v7())
        .bind(user_id)
        .bind(concurrent_track_id)
        .execute(&play_pool)
        .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut play_task)
            .await
            .is_err(),
        "play projection waits for the in-flight Track relationship transaction"
    );
    relationship_tx
        .commit()
        .await
        .expect("commit concurrent relationships");
    play_task
        .await
        .expect("join concurrent play")
        .expect("record concurrent play");

    let concurrent_release_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='release' AND entity_id=$2)",
    )
    .bind(first.release_id().into_uuid())
    .bind(second.release_id().into_uuid())
    .fetch_one(&db.pool)
    .await
    .expect("concurrent Release popularity");
    assert_eq!(concurrent_release_popularity, (1, 2));
    let concurrent_artist_popularity: (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$1),
           (SELECT popularity FROM native_recommendation_candidates
            WHERE entity_type='artist' AND entity_id=$2)",
    )
    .bind(first_artist_id)
    .bind(second_artist_id)
    .fetch_one(&db.pool)
    .await
    .expect("concurrent Artist popularity");
    assert_eq!(concurrent_artist_popularity, (1, 2));
    assert!(matches!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user_id,
            second_track_public_id,
            "phase7-event-1",
            played_at,
            now,
            Some(30),
        )
        .await,
        Err(NativeUserStateStoreError::Conflict)
    ));

    NativeUserStateStore::delete_playlist(&db.pool, user_id, playlist.public_api_id)
        .await
        .expect("delete owned playlist");
    assert!(matches!(
        NativeUserStateStore::playlist(&db.pool, user_id, playlist.public_api_id).await,
        Err(NativeUserStateStoreError::NotFound)
    ));
    // A client clock ahead of the server must not fail the write. Previously the
    // future timestamp violated `native_now_playing_time_check` and surfaced to
    // the client as a `500`; the frozen server ignores client time entirely, so a
    // skewed timestamp is clamped to server-observed time.
    let future = now + time::Duration::seconds(600);
    NativeUserStateStore::record_now_playing(
        &db.pool,
        user_id,
        track_public_id,
        "phase7-future-now-playing",
        future,
        now,
    )
    .await
    .expect("a future now-playing timestamp is clamped, not rejected");
    let started_at: time::OffsetDateTime =
        sqlx::query_scalar("SELECT started_at FROM native_now_playing WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("read clamped now-playing row");
    assert!(
        started_at <= now,
        "clamped started_at must not be in the future"
    );
    assert_eq!(
        NativeUserStateStore::record_scrobble(
            &db.pool,
            user_id,
            track_public_id,
            "phase7-future-played",
            future,
            now,
            Some(30),
        )
        .await
        .expect("a future played timestamp is clamped, not rejected"),
        NativeScrobbleResult::Applied
    );

    db.drop().await;
}
