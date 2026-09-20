//! Shared helpers for the legacy-migration integration suites
//! (`legacy.rs`, `legacy_faults.rs`). Source databases are seeded from the
//! frozen legacy DDL fixture; targets are fresh migrated databases; the CLI
//! under test is spawned as a real binary so coverage includes argument
//! parsing, fencing, and exit behavior end to end.

#![allow(dead_code, clippy::panic, clippy::expect_used, clippy::unwrap_used)]

use std::process::Command;

use sqlx::PgPool;
use uuid::Uuid;

pub const LEGACY_DDL: &str = include_str!("../fixtures/legacy_schema.sql");

pub fn admin_url() -> String {
    std::env::var("MELODEE_TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://melodee:melodee-dev@127.0.0.1:5433/postgres".to_string())
}

/// A disposable database seeded with the frozen legacy schema plus seed SQL.
pub struct LegacySource {
    pub name: String,
    pub url: String,
    pub pool: PgPool,
}

pub async fn create_source(test_name: &str, seed_sql: &str) -> LegacySource {
    create_source_with_ddl(test_name, LEGACY_DDL, seed_sql).await
}

pub async fn create_source_with_ddl(test_name: &str, ddl: &str, seed_sql: &str) -> LegacySource {
    let admin = PgPool::connect(&admin_url()).await.expect("admin connect");
    let name = format!("legacy_src_{}_{}", test_name, Uuid::now_v7().simple());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .expect("drop source");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
        .execute(&admin)
        .await
        .expect("create source");
    let url = admin_url().replace("/postgres", &format!("/{name}"));
    let pool = PgPool::connect(&url).await.expect("source connect");
    // DDL and seed are test-owned literals; AssertSqlSafe satisfies sqlx's
    // static-SQL bound for the owned copies.
    let ddl = ddl.to_string();
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(&pool)
        .await
        .expect("legacy ddl");
    if !seed_sql.trim().is_empty() {
        let seed = seed_sql.to_string();
        sqlx::raw_sql(sqlx::AssertSqlSafe(seed))
            .execute(&pool)
            .await
            .expect("legacy seed");
    }
    LegacySource { name, url, pool }
}

/// A disposable migrated target database.
pub struct Target {
    pub name: String,
    pub url: String,
    pub pool: PgPool,
}

pub async fn create_target(test_name: &str) -> Target {
    let admin = PgPool::connect(&admin_url()).await.expect("admin connect");
    let name = format!("legacy_tgt_{}_{}", test_name, Uuid::now_v7().simple());
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .expect("drop target");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE DATABASE \"{name}\"")))
        .execute(&admin)
        .await
        .expect("create target");
    let url = admin_url().replace("/postgres", &format!("/{name}"));
    let pool = PgPool::connect(&url).await.expect("target connect");
    melodee_persistence::migrate(&pool)
        .await
        .expect("target migrations");
    Target { name, url, pool }
}

pub async fn drop_db(name: &str) {
    let admin = PgPool::connect(&admin_url()).await.expect("admin connect");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"
    )))
    .execute(&admin)
    .await
    .expect("drop db");
}

/// Runs the CLI successfully; returns stdout.
pub fn run_legacy(target_url: &str, args: &[String]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .arg("legacy")
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, target_url)
        .output()
        .expect("spawn melodee legacy");
    assert!(
        output.status.success(),
        "args {args:?} failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is UTF-8")
}

/// Runs the CLI expecting failure; returns stderr.
pub fn run_legacy_failing(target_url: &str, args: &[String]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .arg("legacy")
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, target_url)
        .output()
        .expect("spawn melodee legacy");
    assert!(
        !output.status.success(),
        "args {args:?} unexpectedly succeeded"
    );
    String::from_utf8(output.stderr).expect("stderr is UTF-8")
}

/// Runs a top-level CLI command successfully; used for rollback commands
/// which are intentionally outside the `legacy` subcommand.
pub fn run_cli(database_url: &str, args: &[String]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_melodee-cli"))
        .args(args)
        .env(melodee_persistence::ENV_DATABASE_URL, database_url)
        .output()
        .expect("spawn melodee cli");
    assert!(
        output.status.success(),
        "args {args:?} failed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout is UTF-8")
}

pub fn source_args(source_url: &str) -> Vec<String> {
    vec!["--source-db".to_string(), source_url.to_string()]
}

pub fn map_arg() -> String {
    "/music=library".to_string()
}

/// Mixed seed: same-name artists, album with original release date, two songs
/// (one invalid hash), portable + unsupported credentials, playlist, share,
/// bookmark, states, play history, podcast, radio, settings.
///
/// Every NOT NULL column from the frozen legacy DDL is supplied explicitly;
/// the fixture schema has no database defaults (EF Core supplied values
/// client-side), so seeds must carry the full required column set.
pub const SEED_MIXED: &str = r#"
-- Artist 1 carries a Directory to prove the legacy path composition
-- (Artist.Directory / Album.Directory / Song.FileName) end to end.
INSERT INTO "Artists" ("Id","ApiKey","Name","NameNormalized","SortName","Directory","AlbumCount","CalculatedRating","IsLocked","LibraryId","MetaDataStatus","PlayedCount","SongCount","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'11111111-1111-1111-1111-111111111111','Miles Davis','miles davis','Davis, Miles','Miles Davis',1,0,false,1,0,0,2,0,now(),now()),
       (2,'22222222-2222-2222-2222-222222222222','Miles Davis','miles davis','Davis, Miles','Miles Davis (2)',0,0,false,1,0,0,0,0,now(),now());
INSERT INTO "Albums" ("Id","ApiKey","ArtistId","Name","NameNormalized","SortName","AlbumStatus","AlbumType","CalculatedRating","Duration","IsCompilation","IsLocked","MetaDataStatus","PlayedCount","SortOrder","ReleaseDate","OriginalReleaseDate","Directory","CreatedAt","LastUpdatedAt")
VALUES (1,'33333333-3333-3333-3333-333333333333',1,'Kind of Blue','kind of blue','Kind of Blue',1,1,0,3400,false,false,0,0,0,DATE '1990-01-01',DATE '1959-08-17','miles/kind of blue',now(),now());
INSERT INTO "Songs" ("Id","ApiKey","AlbumId","Title","TitleNormalized","TitleSort","SongNumber","Duration","FileName","FileSize","FileHash","ContentType","BitRate","SamplingRate","ChannelCount","BPM","BitDepth","CalculatedRating","IsLocked","IsVbr","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'44444444-4444-4444-4444-444444444444',1,'So What','so what','So What',1,545.0,'01 So What.flac',1234567,'not-a-valid-hex-hash','audio/flac',900000,44100,2,0,24,0,false,false,0,0,now(),now()),
       (2,'55555555-5555-5555-5555-555555555555',1,'Freddie Freeloader','freddie freeloader','Freddie Freeloader',2,589.0,'02 Freddie Freeloader.flac',2345678,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','audio/flac',910000,44100,2,0,24,0,false,false,0,0,now(),now());
INSERT INTO "Contributors" ("Id","ApiKey","AlbumId","SongId","ArtistId","ContributorName","ContributorType","Role","SubRole","MetaTagIdentifier","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'66666666-6666-6666-6666-666666666666',1,1,2,'Bill Evans',1,'piano',NULL,0,false,0,now(),now());
INSERT INTO "Users" ("Id","ApiKey","UserName","UserNameNormalized","Email","EmailNormalized","PasswordHash","PasswordHashAlgorithm","PublicKey","PasswordEncrypted","TimeZoneId","Description","PreferredLanguage","PreferredTheme","IsAdmin","IsEditor","IsLocked","IsScrobblingEnabled","HasStreamRole","HasDownloadRole","HasUploadRole","HasPlaylistRole","HasShareRole","HasCoverArtRole","HasCommentRole","HasPodcastRole","HasSettingsRole","HasJukeboxRole","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'77777777-7777-7777-7777-777777777777','mel','mel','mel@example.com','mel@example.com','$2a$10$abcdefghijklmnopqrstuuV5k7oW1O0Z1m8H0S1m8H0S1m8H0S1m8H0S','bcrypt','','','UTC','C# migration bio','en-US','dark',true,false,false,false,true,true,false,true,true,false,false,false,false,false,0,now(),now()),
       (2,'88888888-8888-8888-8888-888888888888','legacy','legacy','legacy@example.com','legacy@example.com','SHA256$deadbeef','sha256','','','UTC',NULL,NULL,NULL,false,false,false,false,true,false,false,false,false,false,false,false,false,false,0,now(),now());
INSERT INTO "Playlists" ("Id","ApiKey","Name","Description","IsPublic","UserId","Duration","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'99999999-9999-9999-9999-999999999999','Favorites','favs',true,1,1134.0,false,0,now(),now());
INSERT INTO "PlaylistSong" ("PlaylistId","SongId","PlaylistOrder","SongApiKey")
VALUES (1,1,1,'44444444-4444-4444-4444-444444444444'),(1,2,2,'55555555-5555-5555-5555-555555555555');
INSERT INTO "Shares" ("Id","ApiKey","ShareId","ShareUniqueId","ShareType","UserId","Description","IsDownloadable","IsLocked","SortOrder","VisitCount","CreatedAt","LastUpdatedAt")
VALUES (1,'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa',1,'0123456789abcdef0123456789abcdef',1,1,'share desc',false,false,0,3,now(),now());
INSERT INTO "Bookmarks" ("Id","ApiKey","UserId","SongId","Position","Comment","CalculatedRating","IsLocked","PlayedCount","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'dddddddd-0000-0000-0000-000000000001',1,1,12000,'resume here',0,false,0,0,now(),now());
INSERT INTO "UserSongs" ("Id","ApiKey","UserId","SongId","IsStarred","IsHated","Rating","PlayedCount","IsLocked","SortOrder","LastPlayedAt","CreatedAt","LastUpdatedAt")
VALUES (1,'dddddddd-0000-0000-0000-000000000002',1,1,true,false,5,12,false,0,now(),now(),now());
INSERT INTO "UserAlbums" ("Id","ApiKey","UserId","AlbumId","IsStarred","IsHated","Rating","PlayedCount","IsLocked","SortOrder","LastPlayedAt","CreatedAt","LastUpdatedAt")
VALUES (1,'dddddddd-0000-0000-0000-000000000003',1,1,false,false,4,3,false,0,now(),now(),now());
INSERT INTO "UserArtists" ("Id","ApiKey","UserId","ArtistId","IsStarred","IsHated","Rating","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'dddddddd-0000-0000-0000-000000000004',1,1,true,false,5,false,0,now(),now());
INSERT INTO "UserSongPlayHistories" ("Id","UserId","SongId","PlayedAt","SecondsPlayed","Client","IsNowPlaying","Source")
VALUES (1,1,1,now(),540,'meloamp',false,0);
INSERT INTO "PodcastChannels" ("Id","ApiKey","UserId","FeedUrl","Title","Description","ImageUrl","SiteUrl","AutoDownloadEnabled","ConsecutiveFailureCount","IsLocked","SortOrder","RefreshIntervalHours","MaxDownloadedEpisodes","MaxStorageBytes","IsDeleted","CreatedAt","LastUpdatedAt")
VALUES (1,'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb',1,'https://feeds.example.com/show','Show','desc','https://img','https://site',true,0,false,0,24,10,1073741824,false,now(),now());
INSERT INTO "RadioStations" ("Id","ApiKey","Name","StreamUrl","HomePageUrl","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'cccccccc-cccc-cccc-cccc-cccccccccccc','Jazz FM','https://stream.example.com/jazz','https://jazz.example.com',false,0,now(),now());
INSERT INTO "Settings" ("Id","ApiKey","Key","Value","Category","IsLocked","SortOrder","CreatedAt")
VALUES (1,'dddddddd-0000-0000-0000-000000000005','library.scan.enabled','true',1,false,0,now());
-- Extended user/operational domains.
INSERT INTO "UserGroups" ("Id","ApiKey","Name","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000001','Family',false,0,now(),now());
INSERT INTO "UserGroupMembers" ("Id","ApiKey","UserGroupId","UserId","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000002',1,1,false,0,now(),now());
INSERT INTO "SmartPlaylists" ("Id","ApiKey","Name","MqlQuery","NormalizedQuery","EntityType","IsPublic","LastResultCount","IsLocked","SortOrder","UserId","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000003','Top Rated','rating>=4','rating>=4','Song',false,2,false,0,1,now(),now());
INSERT INTO "PodcastEpisodes" ("Id","ApiKey","PodcastChannelId","Guid","EpisodeKey","Title","PublishDate","EnclosureUrl","DownloadStatus","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000004',1,'episode-guid-1','episode-guid-1','Episode 1',now(),'https://cdn.example.com/e1.mp3',0,false,0,now(),now());
INSERT INTO "PodcastEpisodeBookmarks" ("Id","PodcastEpisodeId","PositionSeconds","Comment","UserId","CreatedAt","UpdatedAt")
VALUES (1,1,42,'half way',1,now(),now());
INSERT INTO "UserPodcastEpisodePlayHistories" ("Id","UserId","PodcastEpisodeId","PlayedAt","SecondsPlayed","Client","IsNowPlaying","Source")
VALUES (1,1,1,now(),300,'meloamp',false,0);
INSERT INTO "UserPlaybackSettings" ("Id","ApiKey","UserId","AudioQuality","CrossfadeDuration","GaplessPlayback","IsLocked","SortOrder","VolumeNormalization","ReplayGain","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000005',1,'High',2.5,true,false,0,false,'none',now(),now());
INSERT INTO "UserEqualizerPresets" ("Id","ApiKey","UserId","Name","NameNormalized","BandsJson","IsDefault","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000006',1,'Jazz club','jazz club','[{"hz":60,"gain":3},{"hz":8000,"gain":-1}]',true,false,0,now(),now());
INSERT INTO "UserSocialLogins" ("Id","ApiKey","UserId","Provider","Subject","Email","DisplayName","IsLocked","SortOrder","LastLoginAt","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000007',1,'Google','google-sub-1','mel@example.com','Mel',false,0,now(),now(),now());
INSERT INTO "UserPins" ("Id","ApiKey","UserId","PinId","PinType","IsLocked","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000008',1,1,3,false,0,now(),now());
INSERT INTO "Requests" ("Id","ApiKey","Category","Status","Description","ArtistName","CreatedByUserId","UpdatedByUserId","LastActivityAt","LastActivityType","CreatedAt","UpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-000000000009',1,1,'Please add this record','Miles Davis',1,1,now(),1,now(),now());
INSERT INTO "RequestComments" ("Id","ApiKey","RequestId","Body","CreatedByUserId","IsSystem","CreatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-00000000000a',1,'Second this',2,false,now());
INSERT INTO "RequestUserStates" ("RequestId","UserId","LastSeenAt","CreatedAt","UpdatedAt")
VALUES (1,2,now(),now(),now());
INSERT INTO "UserDeviceProfiles" ("Id","ApiKey","UserId","Name","DirectPlay","IsDefaultProfile","IsLocked","Priority","SortOrder","CreatedAt","LastUpdatedAt")
VALUES (1,'eeeeeeee-0000-0000-0000-00000000000b',1,'Phone',true,true,false,1,0,now(),now());
UPDATE "Users" SET "LastFmSessionKey" = 'lastfm-session-abcdef' WHERE "Id" = 1;
"#;

/// Produces a legacy `v1:gcm:` protection blob (AES-256-GCM, key = SHA-256
/// of the legacy `security.secretKey`, wire order nonce‖tag‖ciphertext) for
/// the re-seal tests. Test-only encryption path mirroring the legacy C#
/// SecretProtector exactly.
pub fn legacy_protect(security_key: &str, plaintext: &str) -> String {
    use base64::Engine;
    use ring::aead::{AES_256_GCM, LessSafeKey, Nonce, UnboundKey};
    use sha2::Digest;
    let key = sha2::Sha256::digest(security_key.as_bytes());
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).expect("key"));
    let nonce_bytes: [u8; 12] = [7u8; 12];
    let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes).expect("nonce");
    let mut in_out = plaintext.as_bytes().to_vec();
    cipher
        .seal_in_place_append_tag(nonce, ring::aead::Aad::empty(), &mut in_out)
        .expect("seal");
    // ring produced ciphertext‖tag; legacy wire order is nonce‖tag‖ct.
    let split = in_out.len() - 16;
    let mut combined = Vec::with_capacity(12 + in_out.len());
    combined.extend_from_slice(&nonce_bytes);
    combined.extend_from_slice(&in_out[split..]);
    combined.extend_from_slice(&in_out[..split]);
    format!(
        "v1:gcm:{}",
        base64::engine::general_purpose::STANDARD.encode(combined)
    )
}

pub async fn count(pool: &PgPool, sql_table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM {sql_table}"
    )))
    .fetch_one(pool)
    .await
    .expect("count")
}
