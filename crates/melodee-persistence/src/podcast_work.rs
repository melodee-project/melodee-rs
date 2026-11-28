//! SQL operations for podcast refresh, fenced publication, retention, and recovery.
//! Application services own transaction boundaries and filesystem/network work.

use sqlx::{FromRow, PgPool, Postgres, Transaction, postgres::PgQueryResult};
use time::OffsetDateTime;
use uuid::Uuid;

/// Public episode ID, stored path, lease owner, and whether its download lease is live.
pub type OwnedFileFact = (Uuid, Option<String>, Option<Uuid>, bool);

/// Parsed feed facts to persist in an application-owned transaction.
#[derive(Debug, Default)]
pub struct PodcastFeedUpdate {
    pub title: Option<String>,
    pub description: Option<String>,
    pub website: Option<String>,
    pub image_url: Option<String>,
    pub episodes: Vec<PodcastFeedEpisode>,
}

/// One parsed episode upsert, independent of the feed transport format.
#[derive(Debug, Default)]
pub struct PodcastFeedEpisode {
    pub feed_item_id: String,
    pub title: String,
    pub description: String,
    pub published_at: Option<OffsetDateTime>,
    pub duration_ms: Option<i64>,
    pub enclosure_url: Option<String>,
}

/// Claimed enclosure and retry facts under the worker lease.
#[derive(Debug, FromRow)]
pub struct DownloadClaim {
    pub id: i64,
    pub public_api_id: Uuid,
    pub enclosure_url: String,
    pub download_attempt_count: i32,
}

/// Failed or expired-lease download joined with its channel for recovery reporting.
#[derive(Debug, FromRow)]
pub struct StuckPodcastDownload {
    pub episode_id: i64,
    pub public_api_id: Uuid,
    pub channel_id: i64,
    pub channel_title: String,
    pub episode_title: String,
    pub download_status: String,
    pub download_attempt_count: i32,
    pub download_error: Option<String>,
    pub download_lease_owner: Option<Uuid>,
    pub download_lease_expires_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

/// Repository operation used by the bounded podcast application service.
pub async fn channel_feed_url(
    pool: &PgPool,
    channel_id: i64,
    owner: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar::<_, String>(
        "SELECT feed_url FROM native_podcast_channels WHERE id=$1 AND owner_user_id=$2 AND soft_deleted_at IS NULL",
    ).bind(channel_id).bind(owner).fetch_optional(pool).await
}

/// Repository operation used by the bounded podcast application service.
pub async fn update_channel(
    tx: &mut Transaction<'_, Postgres>,
    channel_id: i64,
    owner: Uuid,
    parsed: &PodcastFeedUpdate,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
        "UPDATE native_podcast_channels SET title=COALESCE($3,title),description=COALESCE($4,description),\
         website=COALESCE($5,website),image_url=COALESCE($6,image_url),last_sync_at=now(),updated_at=now() \
         WHERE id=$1 AND owner_user_id=$2 AND soft_deleted_at IS NULL",
    ).bind(channel_id).bind(owner).bind(&parsed.title).bind(&parsed.description)
        .bind(&parsed.website).bind(&parsed.image_url).execute(&mut **tx).await
}

/// Repository operation used by the bounded podcast application service.
pub async fn channel_auto_download(
    tx: &mut Transaction<'_, Postgres>,
    channel_id: i64,
    owner: Uuid,
) -> Result<Option<bool>, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT auto_download_enabled FROM native_podcast_channels
         WHERE id=$1 AND owner_user_id=$2 AND soft_deleted_at IS NULL",
    )
    .bind(channel_id)
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn upsert_feed_episode(
    tx: &mut Transaction<'_, Postgres>,
    channel_id: i64,
    item: &PodcastFeedEpisode,
    auto_download: bool,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
            "INSERT INTO native_podcast_episodes(public_api_id,channel_id,feed_item_id,title,description,\
             published_at,duration_ms,enclosure_url,download_status,created_at,updated_at) \
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,CASE WHEN $9 AND $8::text IS NOT NULL THEN 'Queued' ELSE 'NotDownloaded' END,now(),now()) \
             ON CONFLICT(channel_id,feed_item_id) DO UPDATE SET title=EXCLUDED.title,description=EXCLUDED.description,\
             published_at=EXCLUDED.published_at,duration_ms=EXCLUDED.duration_ms,enclosure_url=EXCLUDED.enclosure_url,updated_at=now()",
        ).bind(Uuid::now_v7()).bind(channel_id).bind(&item.feed_item_id).bind(&item.title)
            .bind(&item.description).bind(item.published_at).bind(item.duration_ms)
            .bind(&item.enclosure_url).bind(auto_download).execute(&mut **tx).await
}

/// Repository operation used by the bounded podcast application service.
pub async fn claim_download(
    pool: &PgPool,
    worker: Uuid,
    lease_seconds: i64,
    per_user_concurrency: i64,
    quota_bytes_per_user: i64,
) -> Result<Option<DownloadClaim>, sqlx::Error> {
    let (mut connection, _hold) = crate::metrics::acquire_for_claim(
        pool,
        crate::metrics::Metric::PodcastDownloadAcquire,
        crate::metrics::Metric::PodcastDownloadHold,
    )
    .await?;
    sqlx::query_as::<_, DownloadClaim>(
        "WITH claimed AS (SELECT episode.id FROM native_podcast_episodes episode
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id
         WHERE episode.enclosure_url IS NOT NULL AND episode.soft_deleted_at IS NULL
         AND channel.soft_deleted_at IS NULL AND episode.download_available_at<=now()
         AND (episode.download_status='Queued' OR
              (episode.download_status='Downloading' AND episode.download_lease_expires_at<=now())) \
         AND ($3::bigint <= 0 OR (SELECT count(*) FROM native_podcast_episodes active
              JOIN native_podcast_channels active_channel ON active_channel.id=active.channel_id
              WHERE active_channel.owner_user_id=channel.owner_user_id
                AND active.download_status='Downloading'
                AND active.download_lease_expires_at > now()) < $3::bigint) \
         AND ($4::bigint <= 0 OR (SELECT coalesce(sum(used.downloaded_bytes),0) \
              FROM native_podcast_episodes used \
              JOIN native_podcast_channels used_channel ON used_channel.id=used.channel_id \
              WHERE used_channel.owner_user_id=channel.owner_user_id \
                AND used.download_status='Downloaded') < $4::bigint) \
         ORDER BY episode.download_available_at,episode.id FOR UPDATE OF episode SKIP LOCKED LIMIT 1) \
         UPDATE native_podcast_episodes e SET download_status='Downloading',download_lease_owner=$1,\
         download_lease_expires_at=now()+make_interval(secs=>$2::double precision),updated_at=now() \
         FROM claimed WHERE e.id=claimed.id RETURNING e.id,e.public_api_id,e.enclosure_url,e.download_attempt_count",
    )
    .bind(worker)
    .bind(lease_seconds as f64)
    .bind(per_user_concurrency)
    .bind(quota_bytes_per_user)
    .fetch_optional(&mut *connection)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn lock_live_download(
    tx: &mut Transaction<'_, Postgres>,
    episode_id: i64,
    worker: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM native_podcast_episodes episode
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id
         WHERE episode.id=$1 AND episode.soft_deleted_at IS NULL
         AND channel.soft_deleted_at IS NULL AND episode.download_lease_owner=$2
         AND episode.download_status='Downloading'
         AND episode.download_lease_expires_at>now() FOR UPDATE OF episode)",
    )
    .bind(episode_id)
    .bind(worker)
    .fetch_one(&mut **tx)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn finish_download(
    tx: &mut Transaction<'_, Postgres>,
    episode_id: i64,
    worker: Uuid,
    attempt_count: i32,
    path: &str,
    total: i64,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status='Downloaded',local_path=$4,downloaded_bytes=$5,\
         downloaded_at=now(),download_error=NULL,download_lease_owner=NULL,download_lease_expires_at=NULL,updated_at=now() \
         WHERE id=$1 AND download_lease_owner=$2 AND download_status='Downloading'",
    ).bind(episode_id).bind(worker).bind(attempt_count)
        .bind(path).bind(total)
        .execute(&mut **tx).await
}

/// Repository operation used by the bounded podcast application service.
pub async fn record_download_failure(
    pool: &PgPool,
    episode_id: i64,
    worker: Uuid,
    next: i32,
    permanent: bool,
    error: &str,
    backoff: i64,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
        "UPDATE native_podcast_episodes SET download_status=CASE WHEN $4 THEN 'Failed' ELSE 'Queued' END,\
         download_attempt_count=$3,download_error=$5,download_available_at=now()+make_interval(secs=>$6::double precision),\
         download_lease_owner=NULL,download_lease_expires_at=NULL,updated_at=now() \
         WHERE id=$1 AND download_lease_owner=$2",
    ).bind(episode_id).bind(worker).bind(next).bind(permanent).bind(error)
        .bind(backoff as f64).execute(pool).await
}

/// Repository operation used by the bounded podcast application service.
pub async fn list_stuck_downloads(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<StuckPodcastDownload>, sqlx::Error> {
    sqlx::query_as::<_, StuckPodcastDownload>(
        "SELECT episode.id AS episode_id,episode.public_api_id,episode.channel_id,\
         channel.title AS channel_title,episode.title AS episode_title,episode.download_status,\
         episode.download_attempt_count,episode.download_error,episode.download_lease_owner,\
         episode.download_lease_expires_at,episode.updated_at \
         FROM native_podcast_episodes episode \
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id \
         WHERE episode.soft_deleted_at IS NULL AND channel.soft_deleted_at IS NULL \
         AND ((episode.download_status='Downloading' AND episode.download_lease_expires_at<=now()) \
           OR episode.download_status='Failed') \
         ORDER BY episode.updated_at,episode.id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn recover_stuck_downloads(
    pool: &PgPool,
    limit: i64,
    retry_failed: bool,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
        "WITH stuck AS (SELECT episode.id,episode.download_status FROM native_podcast_episodes episode \
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id \
         WHERE episode.soft_deleted_at IS NULL AND channel.soft_deleted_at IS NULL \
         AND ((episode.download_status='Downloading' AND episode.download_lease_expires_at<=now()) \
           OR ($2 AND episode.download_status='Failed')) \
         ORDER BY episode.updated_at,episode.id FOR UPDATE OF episode SKIP LOCKED LIMIT $1) \
         UPDATE native_podcast_episodes e SET download_status='Queued',\
         download_attempt_count=CASE WHEN stuck.download_status='Failed' THEN 0 ELSE e.download_attempt_count END,\
         download_error=NULL,download_available_at=now(),\
         download_lease_owner=NULL,download_lease_expires_at=NULL,updated_at=now() \
         FROM stuck WHERE e.id=stuck.id",
    )
    .bind(limit)
    .bind(retry_failed)
    .execute(pool)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn due_channels(pool: &PgPool, limit: i64) -> Result<Vec<(i64, Uuid)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, Uuid)>(
        "SELECT id,owner_user_id FROM native_podcast_channels WHERE soft_deleted_at IS NULL \
         AND refresh_interval_hours IS NOT NULL \
         AND (last_sync_at IS NULL OR last_sync_at<=now()-make_interval(hours=>refresh_interval_hours)) \
         ORDER BY COALESCE(last_sync_at,'epoch'::timestamptz),id LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn owned_file_facts(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<Vec<OwnedFileFact>, sqlx::Error> {
    sqlx::query_as::<_, OwnedFileFact>(
        "SELECT episode.public_api_id,episode.local_path,episode.download_lease_owner,
                episode.download_status='Downloading' AND episode.download_lease_expires_at>now()
         FROM native_podcast_episodes episode
         JOIN native_podcast_channels channel ON channel.id=episode.channel_id
         WHERE episode.public_api_id=ANY($1) AND episode.soft_deleted_at IS NULL
           AND channel.soft_deleted_at IS NULL",
    )
    .bind(ids)
    .fetch_all(pool)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn retention_candidates(
    pool: &PgPool,
    keep_last_n: i64,
    days: i64,
    keep_unplayed_only: bool,
) -> Result<Vec<(i64, Uuid, String)>, sqlx::Error> {
    sqlx::query_as::<_, (i64, Uuid, String)>(
        "WITH ranked AS (SELECT e.id,e.public_api_id,e.local_path,e.downloaded_at,c.max_downloaded_episodes,\
         c.max_storage_bytes,row_number() OVER(PARTITION BY c.id ORDER BY e.published_at DESC NULLS LAST,e.id DESC) AS ordinal,\
         sum(COALESCE(e.downloaded_bytes,0)) OVER(PARTITION BY c.id ORDER BY e.published_at DESC NULLS LAST,e.id DESC) AS cumulative \
         FROM native_podcast_episodes e JOIN native_podcast_channels c ON c.id=e.channel_id \
         WHERE e.download_status='Downloaded' AND e.local_path IS NOT NULL
         AND e.soft_deleted_at IS NULL AND c.soft_deleted_at IS NULL \
         AND NOT EXISTS(SELECT 1 FROM native_podcast_bookmarks b WHERE b.episode_id=e.id)) \
         SELECT id,public_api_id,local_path FROM ranked WHERE \
         (max_downloaded_episodes IS NOT NULL AND ordinal>max_downloaded_episodes) OR \
         (max_storage_bytes IS NOT NULL AND cumulative>max_storage_bytes) OR \
         ($1::bigint > 0 AND ordinal > $1::bigint) OR \
         ($2::bigint > 0 AND downloaded_at IS NOT NULL
             AND downloaded_at < now() - make_interval(days => $2::integer)) OR \
         ($3::boolean AND EXISTS(SELECT 1 FROM native_podcast_play_history h
             WHERE h.episode_id=ranked.id AND NOT h.is_now_playing)) \
         ORDER BY id LIMIT 100",
    )
    .bind(keep_last_n)
    .bind(days)
    .bind(keep_unplayed_only)
    .fetch_all(pool)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn lock_retention_candidate(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    path: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, i64>(
        "SELECT episode.id FROM native_podcast_episodes episode
             WHERE episode.id=$1 AND episode.local_path=$2
               AND episode.download_status='Downloaded'
               AND NOT EXISTS(
                   SELECT 1 FROM native_podcast_bookmarks bookmark
                   WHERE bookmark.episode_id=episode.id
               )
             FOR UPDATE",
    )
    .bind(id)
    .bind(path)
    .fetch_optional(&mut **tx)
    .await
}

/// Repository operation used by the bounded podcast application service.
pub async fn clear_download(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    path: &str,
) -> Result<PgQueryResult, sqlx::Error> {
    sqlx::query(
            "UPDATE native_podcast_episodes SET download_status='NotDownloaded',local_path=NULL,\
             downloaded_bytes=NULL,downloaded_at=NULL,updated_at=now() WHERE id=$1 AND local_path=$2",
        )
        .bind(id)
        .bind(path)
        .execute(&mut **tx)
        .await
}
