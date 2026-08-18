//! Phase 7 authentication/session/public-ID persistence acceptance tests.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::{T0, TestDb};
use melodee_auth::{PasswordHashAlgorithm, RefreshTokenDigest, SecretString, verify_password};
use melodee_persistence::{
    AuthLookup, AuthStore, AuthStoreError, MigrationIdentityBinding, NativeUserImport,
    PasswordResetIssue, PasswordResetOutcome, RateLimitOutcome, RateLimitPolicy,
    RefreshFamilyIssue, RefreshRotationOutcome, RefreshRotationRequest, SocialLoginLink,
};
use time::Duration;
use uuid::Uuid;

fn roles() -> Vec<String> {
    vec!["Administrator".to_string(), "HasStreamRole".to_string()]
}

fn imported_user<'a>(
    id: Uuid,
    public_api_id: Uuid,
    password_hash: &'a str,
    roles: &'a [String],
) -> NativeUserImport<'a> {
    NativeUserImport {
        id,
        public_api_id,
        username: "phase0-fixture",
        normalized_username: "PHASE0-FIXTURE",
        email: "phase0-fixture@example.test",
        normalized_email: "PHASE0-FIXTURE@EXAMPLE.TEST",
        password_hash,
        password_hash_algorithm: PasswordHashAlgorithm::Bcrypt,
        stream_signing_key: "legacy-public-key-secret",
        is_admin: true,
        is_editor: true,
        enabled: true,
        locked: false,
        blacklisted: false,
        preferred_language: Some("en-US"),
        preferred_theme: Some("dark"),
        auth_revision: 1,
        roles,
        created_at: T0,
        updated_at: T0,
    }
}

async fn seed_user(db: &TestDb) -> (AuthStore, Uuid, Uuid, String, Vec<String>) {
    let store = AuthStore::new();
    let id = Uuid::now_v7();
    let public_api_id =
        Uuid::parse_str("f78448cc-82ec-5864-877c-6c6163b00065").expect("captured public id");
    let password_hash = bcrypt::hash("Mel0deeR0cks!", 4).expect("bcrypt fixture");
    let roles = roles();
    store
        .import_user(
            &db.pool,
            &imported_user(id, public_api_id, &password_hash, &roles),
        )
        .await
        .expect("import user");
    (store, id, public_api_id, password_hash, roles)
}

#[tokio::test]
async fn legacy_user_import_preserves_public_id_hash_roles_and_stream_secret() {
    let db = TestDb::create("phase7_auth_import").await;
    let (store, id, public_api_id, password_hash, roles) = seed_user(&db).await;

    // Exact import replay is idempotent and preserves the captured UUID.
    let replayed = store
        .import_user(
            &db.pool,
            &imported_user(id, public_api_id, &password_hash, &roles),
        )
        .await
        .expect("exact replay");
    assert_eq!(replayed.public_api_id, public_api_id);
    assert_eq!(replayed.roles, vec!["HasStreamRole", "Administrator"]);

    let auth = store
        .user_for_auth(&db.pool, AuthLookup::Username("PHASE0-FIXTURE"))
        .await
        .expect("lookup")
        .expect("user");
    assert!(
        verify_password(
            b"Mel0deeR0cks!",
            auth.password_hash_algorithm,
            auth.password_hash.expose_secret(),
        )
        .expect("valid imported hash")
    );
    assert!(!format!("{auth:?}").contains(&password_hash));

    let stream = store
        .stream_principal_by_public_id(&db.pool, public_api_id)
        .await
        .expect("stream lookup")
        .expect("principal");
    assert!(stream.is_authorized());
    assert_eq!(
        stream.stream_signing_key,
        SecretString::new("legacy-public-key-secret".to_string())
    );
    assert!(!format!("{stream:?}").contains("legacy-public-key-secret"));

    // Public identity is immutable even to direct SQL.
    let rewrite = sqlx::query("UPDATE native_users SET public_api_id=$2 WHERE id=$1")
        .bind(id)
        .bind(Uuid::from_u128(0xaaaaaaaa_aaaa_4aaa_8aaa_aaaaaaaaaaaa))
        .execute(&db.pool)
        .await;
    assert!(rewrite.is_err());

    let changed_roles = vec!["Administrator".to_string()];
    let conflict = store
        .import_user(
            &db.pool,
            &imported_user(id, public_api_id, &password_hash, &changed_roles),
        )
        .await;
    assert!(matches!(conflict, Err(AuthStoreError::ImportConflict)));
    db.drop().await;
}

#[tokio::test]
async fn refresh_rotation_is_atomic_and_replay_revokes_the_family() {
    let db = TestDb::create("phase7_auth_rotation").await;
    let (store, user_id, _, _, _) = seed_user(&db).await;
    let family_id = Uuid::now_v7();
    let token_id = Uuid::now_v7();
    let original = RefreshTokenDigest::from_token(b"first-secret-token");
    store
        .issue_refresh_family(
            &db.pool,
            &RefreshFamilyIssue {
                family_id,
                token_id,
                user_id,
                expected_auth_revision: 1,
                token_digest: &original,
                device_id: Some("android-fixture"),
                session_started_at: T0,
                family_expires_at: T0 + Duration::days(90),
                token_expires_at: T0 + Duration::days(30),
                issued_at: T0,
            },
        )
        .await
        .expect("issue family");

    let successor = RefreshTokenDigest::from_token(b"second-secret-token");
    let rotated = store
        .rotate_refresh_token(
            &db.pool,
            &RefreshRotationRequest {
                current_digest: &original,
                replacement_token_id: Uuid::now_v7(),
                replacement_digest: &successor,
                device_id: Some("android-fixture"),
                now: T0 + Duration::minutes(1),
                replacement_expires_at: T0 + Duration::days(30),
            },
        )
        .await
        .expect("rotate");
    assert_eq!(
        rotated,
        RefreshRotationOutcome::Rotated {
            user_id,
            family_id,
            refresh_expires_at: T0 + Duration::days(30),
        }
    );

    let replayed = store
        .rotate_refresh_token(
            &db.pool,
            &RefreshRotationRequest {
                current_digest: &original,
                replacement_token_id: Uuid::now_v7(),
                replacement_digest: &RefreshTokenDigest::from_token(b"attacker-token"),
                device_id: Some("android-fixture"),
                now: T0 + Duration::minutes(2),
                replacement_expires_at: T0 + Duration::days(30),
            },
        )
        .await
        .expect("replay result");
    assert_eq!(replayed, RefreshRotationOutcome::Replayed);

    let successor_after_cascade = store
        .rotate_refresh_token(
            &db.pool,
            &RefreshRotationRequest {
                current_digest: &successor,
                replacement_token_id: Uuid::now_v7(),
                replacement_digest: &RefreshTokenDigest::from_token(b"third-secret-token"),
                device_id: Some("android-fixture"),
                now: T0 + Duration::minutes(3),
                replacement_expires_at: T0 + Duration::days(30),
            },
        )
        .await
        .expect("family result");
    assert_eq!(successor_after_cascade, RefreshRotationOutcome::Replayed);
    db.drop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_refresh_consumers_have_one_winner() {
    let db = TestDb::create("phase7_auth_rotation_race").await;
    let (store, user_id, _, _, _) = seed_user(&db).await;
    let family_id = Uuid::now_v7();
    let original = RefreshTokenDigest::from_token(b"racing-token");
    store
        .issue_refresh_family(
            &db.pool,
            &RefreshFamilyIssue {
                family_id,
                token_id: Uuid::now_v7(),
                user_id,
                expected_auth_revision: 1,
                token_digest: &original,
                device_id: None,
                session_started_at: T0,
                family_expires_at: T0 + Duration::days(90),
                token_expires_at: T0 + Duration::days(30),
                issued_at: T0,
            },
        )
        .await
        .expect("issue");

    let mut tasks = Vec::new();
    for ordinal in 0..2_u8 {
        let pool = db.pool.clone();
        let current = original.clone();
        tasks.push(tokio::spawn(async move {
            let replacement = RefreshTokenDigest::from_token(&[ordinal; 32]);
            AuthStore::new()
                .rotate_refresh_token(
                    &pool,
                    &RefreshRotationRequest {
                        current_digest: &current,
                        replacement_token_id: Uuid::now_v7(),
                        replacement_digest: &replacement,
                        device_id: None,
                        now: T0 + Duration::minutes(1),
                        replacement_expires_at: T0 + Duration::days(30),
                    },
                )
                .await
                .expect("race rotation")
        }));
    }
    let first = tasks.remove(0).await.expect("first task");
    let second = tasks.remove(0).await.expect("second task");
    let outcomes = [first, second];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RefreshRotationOutcome::Rotated { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RefreshRotationOutcome::Replayed))
            .count(),
        1
    );
    db.drop().await;
}

#[tokio::test]
async fn logout_access_revision_and_migration_bindings_fail_closed() {
    let db = TestDb::create("phase7_auth_logout_binding").await;
    let (store, user_id, public_api_id, _, _) = seed_user(&db).await;
    let refresh = RefreshTokenDigest::from_token(b"logout-refresh-token");
    store
        .issue_refresh_family(
            &db.pool,
            &RefreshFamilyIssue {
                family_id: Uuid::now_v7(),
                token_id: Uuid::now_v7(),
                user_id,
                expected_auth_revision: 1,
                token_digest: &refresh,
                device_id: None,
                session_started_at: T0,
                family_expires_at: T0 + Duration::days(90),
                token_expires_at: T0 + Duration::days(30),
                issued_at: T0,
            },
        )
        .await
        .expect("issue");
    assert_eq!(
        store
            .revoke_all_refresh_tokens(&db.pool, user_id, T0 + Duration::minutes(1), "logout")
            .await
            .expect("logout"),
        1
    );
    let after_logout = store
        .rotate_refresh_token(
            &db.pool,
            &RefreshRotationRequest {
                current_digest: &refresh,
                replacement_token_id: Uuid::now_v7(),
                replacement_digest: &RefreshTokenDigest::from_token(b"post-logout"),
                device_id: None,
                now: T0 + Duration::minutes(2),
                replacement_expires_at: T0 + Duration::days(30),
            },
        )
        .await
        .expect("refresh after logout");
    assert_eq!(after_logout, RefreshRotationOutcome::Replayed);

    let binding = MigrationIdentityBinding {
        source_kind: "user".to_string(),
        source_key: "legacy-users:42".to_string(),
        internal_id: user_id,
        public_api_id,
        imported_at: T0,
    };
    store
        .bind_migration_identity(&db.pool, &binding)
        .await
        .expect("bind");
    store
        .bind_migration_identity(&db.pool, &binding)
        .await
        .expect("exact replay");
    let conflicting = MigrationIdentityBinding {
        public_api_id: Uuid::from_u128(0xbbbbbbbb_bbbb_4bbb_8bbb_bbbbbbbbbbbb),
        ..binding
    };
    assert!(matches!(
        store.bind_migration_identity(&db.pool, &conflicting).await,
        Err(AuthStoreError::IdentityBindingConflict)
    ));
    db.drop().await;
}

#[tokio::test]
async fn postgres_rate_limit_is_atomic_expiring_and_capacity_bounded() {
    let db = TestDb::create("phase7_auth_rate_limit").await;
    let policy = RateLimitPolicy {
        scope: "native-login",
        max_attempts: 2,
        window: Duration::minutes(1),
        max_retained_keys: 1,
    };
    let first_key = [1_u8; 32];
    let second_key = [2_u8; 32];
    let store = AuthStore::new();
    assert_eq!(
        store
            .record_rate_limit_attempt(&db.pool, &policy, &first_key, T0)
            .await
            .expect("first"),
        RateLimitOutcome::Allowed { remaining: 1 }
    );
    assert_eq!(
        store
            .record_rate_limit_attempt(&db.pool, &policy, &second_key, T0)
            .await
            .expect("capacity"),
        RateLimitOutcome::Limited {
            retry_after_seconds: 60
        }
    );

    let mut tasks = Vec::new();
    for _ in 0..2 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            AuthStore::new()
                .record_rate_limit_attempt(
                    &pool,
                    &RateLimitPolicy {
                        scope: "native-login",
                        max_attempts: 2,
                        window: Duration::minutes(1),
                        max_retained_keys: 1,
                    },
                    &first_key,
                    T0 + Duration::seconds(1),
                )
                .await
                .expect("atomic attempt")
        }));
    }
    let outcomes = [
        tasks.remove(0).await.expect("task"),
        tasks.remove(0).await.expect("task"),
    ];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RateLimitOutcome::Allowed { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, RateLimitOutcome::Limited { .. }))
            .count(),
        1
    );
    assert_eq!(
        store
            .record_rate_limit_attempt(&db.pool, &policy, &second_key, T0 + Duration::minutes(2),)
            .await
            .expect("expired key removed"),
        RateLimitOutcome::Allowed { remaining: 1 }
    );
    db.drop().await;
}

#[tokio::test]
async fn reset_is_one_use_replaces_hash_and_revokes_refresh_atomically() {
    let db = TestDb::create("phase7_password_reset").await;
    let (store, user_id, _, _, _) = seed_user(&db).await;
    let refresh = RefreshTokenDigest::from_token(b"refresh-before-reset");
    store
        .issue_refresh_family(
            &db.pool,
            &RefreshFamilyIssue {
                family_id: Uuid::now_v7(),
                token_id: Uuid::now_v7(),
                user_id,
                expected_auth_revision: 1,
                token_digest: &refresh,
                device_id: None,
                session_started_at: T0,
                family_expires_at: T0 + Duration::days(90),
                token_expires_at: T0 + Duration::days(30),
                issued_at: T0,
            },
        )
        .await
        .expect("refresh family");
    let clear = b"one-time-reset-token";
    let digest = RefreshTokenDigest::from_token(clear);
    assert!(
        store
            .issue_password_reset(
                &db.pool,
                &PasswordResetIssue {
                    token_id: Uuid::now_v7(),
                    normalized_email: "PHASE0-FIXTURE@EXAMPLE.TEST",
                    token_digest: &digest,
                    issued_at: T0,
                    expires_at: T0 + Duration::hours(1),
                },
            )
            .await
            .expect("issue")
    );
    assert_eq!(
        store
            .validate_password_reset(&db.pool, &digest, T0 + Duration::minutes(1))
            .await
            .expect("validate"),
        PasswordResetOutcome::Valid
    );
    let new_hash = bcrypt::hash("new-safe-password", 4).expect("hash");
    assert!(matches!(
        store
            .consume_password_reset(
                &db.pool,
                &digest,
                &new_hash,
                T0 + Duration::minutes(2),
            )
            .await
            .expect("consume"),
        PasswordResetOutcome::Consumed { user_id: id } if id == user_id
    ));
    assert_eq!(
        store
            .consume_password_reset(&db.pool, &digest, &new_hash, T0 + Duration::minutes(3),)
            .await
            .expect("second consume"),
        PasswordResetOutcome::Invalid
    );
    let login = store
        .user_for_auth(&db.pool, AuthLookup::Username("PHASE0-FIXTURE"))
        .await
        .expect("lookup")
        .expect("user");
    assert!(
        verify_password(
            b"new-safe-password",
            login.password_hash_algorithm,
            login.password_hash.expose_secret(),
        )
        .expect("new hash")
    );
    assert_eq!(
        store
            .rotate_refresh_token(
                &db.pool,
                &RefreshRotationRequest {
                    current_digest: &refresh,
                    replacement_token_id: Uuid::now_v7(),
                    replacement_digest: &RefreshTokenDigest::from_token(b"after-reset"),
                    device_id: None,
                    now: T0 + Duration::minutes(4),
                    replacement_expires_at: T0 + Duration::days(30),
                },
            )
            .await
            .expect("refresh denied"),
        RefreshRotationOutcome::Replayed
    );

    let stale_login_issue = store
        .issue_refresh_family(
            &db.pool,
            &RefreshFamilyIssue {
                family_id: Uuid::now_v7(),
                token_id: Uuid::now_v7(),
                user_id,
                expected_auth_revision: 1,
                token_digest: &RefreshTokenDigest::from_token(b"stale-login-after-reset"),
                device_id: None,
                session_started_at: T0 + Duration::minutes(3),
                family_expires_at: T0 + Duration::days(90),
                token_expires_at: T0 + Duration::days(30),
                issued_at: T0 + Duration::minutes(3),
            },
        )
        .await;
    assert!(matches!(
        stale_login_issue,
        Err(AuthStoreError::InvalidRefreshRequest)
    ));
    let active_families: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM native_refresh_families
         WHERE user_id=$1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(&db.pool)
    .await
    .expect("active families after reset");
    assert_eq!(active_families, 0);

    sqlx::query("UPDATE native_users SET locked=true WHERE id=$1")
        .bind(user_id)
        .execute(&db.pool)
        .await
        .expect("lock account");
    assert!(
        !store
            .issue_password_reset(
                &db.pool,
                &PasswordResetIssue {
                    token_id: Uuid::now_v7(),
                    normalized_email: "PHASE0-FIXTURE@EXAMPLE.TEST",
                    token_digest: &RefreshTokenDigest::from_token(b"locked-account-reset"),
                    issued_at: T0 + Duration::minutes(5),
                    expires_at: T0 + Duration::hours(1),
                },
            )
            .await
            .expect("locked account remains non-enumerating")
    );
    db.drop().await;
}

#[tokio::test]
async fn family_issuance_waits_for_and_loses_to_a_concurrent_credential_revision() {
    let db = TestDb::create("phase7_refresh_issue_reset_race").await;
    let (_, user_id, _, _, _) = seed_user(&db).await;
    let mut reset_transaction = db.pool.begin().await.expect("reset transaction");
    sqlx::query("SELECT id FROM native_users WHERE id=$1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *reset_transaction)
        .await
        .expect("hold credential row lock");

    let issue_pool = db.pool.clone();
    let issue = tokio::spawn(async move {
        AuthStore::new()
            .issue_refresh_family(
                &issue_pool,
                &RefreshFamilyIssue {
                    family_id: Uuid::now_v7(),
                    token_id: Uuid::now_v7(),
                    user_id,
                    expected_auth_revision: 1,
                    token_digest: &RefreshTokenDigest::from_token(b"concurrent-stale-login"),
                    device_id: None,
                    session_started_at: T0,
                    family_expires_at: T0 + Duration::days(90),
                    token_expires_at: T0 + Duration::days(30),
                    issued_at: T0,
                },
            )
            .await
    });

    // The family issuer has started but cannot inspect the account revision
    // until the credential mutation releases the same user-row lock.
    tokio::task::yield_now().await;
    sqlx::query("UPDATE native_users SET auth_revision=auth_revision+1 WHERE id=$1")
        .bind(user_id)
        .execute(&mut *reset_transaction)
        .await
        .expect("advance credential revision");
    reset_transaction.commit().await.expect("commit reset");

    assert!(matches!(
        issue.await.expect("issue task"),
        Err(AuthStoreError::InvalidRefreshRequest)
    ));
    let family_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM native_refresh_families WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&db.pool)
            .await
            .expect("family count");
    assert_eq!(family_count, 0);
    db.drop().await;
}

#[tokio::test]
async fn auth_schema_rejects_impossible_lifecycle_timestamps_and_family_shrink() {
    let db = TestDb::create("phase7_auth_schema_lifecycle").await;
    let (store, user_id, _, _, _) = seed_user(&db).await;
    let family_id = Uuid::now_v7();
    store
        .issue_refresh_family(
            &db.pool,
            &RefreshFamilyIssue {
                family_id,
                token_id: Uuid::now_v7(),
                user_id,
                expected_auth_revision: 1,
                token_digest: &RefreshTokenDigest::from_token(b"lifecycle-token"),
                device_id: None,
                session_started_at: T0,
                family_expires_at: T0 + Duration::days(90),
                token_expires_at: T0 + Duration::days(30),
                issued_at: T0,
            },
        )
        .await
        .expect("issue family");

    let shrink = sqlx::query("UPDATE native_refresh_families SET expires_at=$2 WHERE id=$1")
        .bind(family_id)
        .bind(T0 + Duration::days(1))
        .execute(&db.pool)
        .await;
    assert!(shrink.is_err(), "family cannot exclude an existing token");

    let bad_revocation = sqlx::query(
        "UPDATE native_refresh_tokens
         SET revoked_at=$2,revocation_reason='impossible' WHERE family_id=$1",
    )
    .bind(family_id)
    .bind(T0 - Duration::seconds(1))
    .execute(&db.pool)
    .await;
    assert!(
        bad_revocation.is_err(),
        "revocation cannot predate token issuance"
    );

    let reset_digest = RefreshTokenDigest::from_token(b"impossible-reset-time");
    let bad_reset = sqlx::query(
        "INSERT INTO native_password_reset_tokens(
             id,user_id,token_digest,issued_at,expires_at,consumed_at,created_at
         ) VALUES($1,$2,$3,$4,$5,$6,$4)",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(reset_digest.as_bytes().as_slice())
    .bind(T0)
    .bind(T0 + Duration::hours(1))
    .bind(T0 - Duration::seconds(1))
    .execute(&db.pool)
    .await;
    assert!(bad_reset.is_err(), "reset consumption cannot predate issue");
    db.drop().await;
}

#[tokio::test]
async fn endpoint_session_owner_membership_is_reciprocally_constrained() {
    let db = TestDb::create("phase7_endpoint_owner_constraint").await;
    let (_, owner_id, _, _, _) = seed_user(&db).await;
    let successor_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users(
             id,public_api_id,username,normalized_username,email,normalized_email,
             password_hash,password_hash_algorithm,stream_signing_key,created_at,updated_at
         ) VALUES($1,$1,'successor','SUCCESSOR','successor@example.test',
                  'SUCCESSOR@EXAMPLE.TEST',$2,'bcrypt','successor-key',$3,$3)",
    )
    .bind(successor_id)
    .bind(bcrypt::hash("successor-password", 4).expect("successor hash"))
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("successor user");
    let session_id = Uuid::now_v7();
    let mut setup = db.pool.begin().await.expect("session setup transaction");
    sqlx::query(
        "INSERT INTO native_endpoint_sessions(
             session_public_api_id,owner_user_id,created_at,updated_at
         ) VALUES($1,$2,$3,$3)",
    )
    .bind(session_id)
    .bind(owner_id)
    .bind(T0)
    .execute(&mut *setup)
    .await
    .expect("session");
    sqlx::query(
        "INSERT INTO native_endpoint_session_members(
             session_public_api_id,user_id,role,joined_at
         ) VALUES($1,$2,'owner',$3)",
    )
    .bind(session_id)
    .bind(owner_id)
    .bind(T0)
    .execute(&mut *setup)
    .await
    .expect("owner membership");
    setup.commit().await.expect("consistent session");

    let delete_owner = sqlx::query(
        "DELETE FROM native_endpoint_session_members
         WHERE session_public_api_id=$1 AND user_id=$2",
    )
    .bind(session_id)
    .bind(owner_id)
    .execute(&db.pool)
    .await;
    assert!(delete_owner.is_err(), "owner membership cannot be deleted");
    let demote_owner = sqlx::query(
        "UPDATE native_endpoint_session_members SET role='listener'
         WHERE session_public_api_id=$1 AND user_id=$2",
    )
    .bind(session_id)
    .bind(owner_id)
    .execute(&db.pool)
    .await;
    assert!(demote_owner.is_err(), "owner membership cannot be demoted");

    let mut transfer = db.pool.begin().await.expect("owner transfer transaction");
    sqlx::query(
        "INSERT INTO native_endpoint_session_members(
             session_public_api_id,user_id,role,joined_at
         ) VALUES($1,$2,'owner',$3)",
    )
    .bind(session_id)
    .bind(successor_id)
    .bind(T0 + Duration::seconds(1))
    .execute(&mut *transfer)
    .await
    .expect("successor membership");
    sqlx::query(
        "UPDATE native_endpoint_sessions SET owner_user_id=$2,updated_at=$3
         WHERE session_public_api_id=$1",
    )
    .bind(session_id)
    .bind(successor_id)
    .bind(T0 + Duration::seconds(1))
    .execute(&mut *transfer)
    .await
    .expect("transfer session owner");
    sqlx::query(
        "UPDATE native_endpoint_session_members SET role='listener'
         WHERE session_public_api_id=$1 AND user_id=$2",
    )
    .bind(session_id)
    .bind(owner_id)
    .execute(&mut *transfer)
    .await
    .expect("demote prior owner atomically");
    transfer.commit().await.expect("valid owner transfer");
    db.drop().await;
}

#[tokio::test]
async fn library_group_binding_is_open_by_default_and_restricted_when_bound() {
    let db = TestDb::create("phase7_library_access").await;
    let (store, member, _, _, _) = seed_user(&db).await;
    let other_user = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,
         normalized_email,password_hash,password_hash_algorithm,stream_signing_key,
         created_at,updated_at) VALUES($1,$2,'other','OTHER','other@example.test',
         'OTHER@EXAMPLE.TEST',$3,'bcrypt','key',$4,$4)",
    )
    .bind(other_user)
    .bind(Uuid::now_v7())
    .bind(bcrypt::hash("other-password", 4).expect("hash"))
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("other user");
    let open_library = Uuid::now_v7();
    let restricted_library = Uuid::now_v7();
    assert!(
        store
            .can_access_library(&db.pool, member, Some(open_library))
            .await
            .expect("open")
    );
    let group = Uuid::now_v7();
    sqlx::query("INSERT INTO native_library_access_groups(id,public_api_id,name,created_at,updated_at) VALUES($1,$2,'restricted',$3,$3)")
        .bind(group).bind(Uuid::now_v7()).bind(T0).execute(&db.pool).await.expect("group");
    sqlx::query("INSERT INTO native_library_group_libraries(group_id,library_id,imported_at) VALUES($1,$2,$3)")
        .bind(group).bind(restricted_library).bind(T0).execute(&db.pool).await.expect("binding");
    sqlx::query(
        "INSERT INTO native_user_library_groups(user_id,group_id,created_at) VALUES($1,$2,$3)",
    )
    .bind(member)
    .bind(group)
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("membership");
    assert!(
        store
            .can_access_library(&db.pool, member, Some(restricted_library))
            .await
            .expect("member")
    );
    assert!(
        !store
            .can_access_library(&db.pool, other_user, Some(restricted_library))
            .await
            .expect("non-member")
    );
    db.drop().await;
}

#[tokio::test]
async fn social_provider_subject_ownership_lists_and_unlinks_without_leaking_subjects() {
    let db = TestDb::create("phase7_social_login_ownership").await;
    let (store, user_id, _, _, _) = seed_user(&db).await;
    store
        .link_social_login(
            &db.pool,
            &SocialLoginLink {
                user_id,
                provider: "google",
                subject: "immutable-google-subject",
                verified_email: Some("phase0-fixture@example.test"),
                display_name: Some("Phase 0 Fixture"),
                hosted_domain: Some("example.test"),
                now: T0,
            },
        )
        .await
        .expect("link provider");

    let links = store
        .social_logins_for_user(&db.pool, user_id)
        .await
        .expect("list links");
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].provider, "google");
    assert_eq!(
        links[0].verified_email.as_deref(),
        Some("phase0-fixture@example.test")
    );
    assert!(!format!("{links:?}").contains("immutable-google-subject"));

    let other_user = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO native_users(id,public_api_id,username,normalized_username,email,
         normalized_email,password_hash,password_hash_algorithm,stream_signing_key,
         created_at,updated_at) VALUES($1,$2,'social-other','SOCIAL-OTHER',
         'social-other@example.test','SOCIAL-OTHER@EXAMPLE.TEST',$3,'bcrypt','key',$4,$4)",
    )
    .bind(other_user)
    .bind(Uuid::now_v7())
    .bind(bcrypt::hash("other-password", 4).expect("hash"))
    .bind(T0)
    .execute(&db.pool)
    .await
    .expect("other user");
    assert!(
        store
            .link_social_login(
                &db.pool,
                &SocialLoginLink {
                    user_id: other_user,
                    provider: "google",
                    subject: "immutable-google-subject",
                    verified_email: None,
                    display_name: None,
                    hosted_domain: None,
                    now: T0,
                },
            )
            .await
            .is_err()
    );
    assert!(
        !store
            .unlink_social_login(&db.pool, other_user, "google")
            .await
            .expect("wrong owner unlink")
    );
    assert!(
        store
            .unlink_social_login(&db.pool, user_id, "google")
            .await
            .expect("owner unlink")
    );
    assert!(
        store
            .social_logins_for_user(&db.pool, user_id)
            .await
            .expect("empty links")
            .is_empty()
    );
    db.drop().await;
}
