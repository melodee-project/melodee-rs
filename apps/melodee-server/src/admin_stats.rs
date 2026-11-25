//! Library, listening, and database statistics for the admin Stats page
//! (PRD-UI-001, ADR-0184).
//!
//! Every number is a bounded, set-based aggregate over durable state: fixed
//! `GROUP BY` buckets with `LIMIT`s, a fixed table list for relation sizes, and
//! zero-filled day/hour spines for the time series so the charts plot a real axis
//! instead of isolated samples. Nothing here walks the catalog row-by-row, reads
//! media files, or probes the search index.

use axum::{
    Json, Router,
    extract::State,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::admin_access::AdminPrincipal;
use crate::admin_system::AdminSystemState;

/// Trailing window for the listening time series.
const LISTENING_WINDOW_DAYS: i64 = 14;
/// Maximum rows in any breakdown list.
const MAX_BREAKDOWN_ROWS: i64 = 12;
/// Maximum rows in a "top N" list.
const MAX_TOP_ROWS: i64 = 5;
pub fn router() -> Router<AdminSystemState> {
    Router::new().route("/api/admin/v1/stats", axum::routing::get(stats))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub generated_at: String,
    pub library: LibraryStatsDto,
    pub processing: ProcessingStatsDto,
    pub listening: ListeningStatsDto,
    pub database: DatabaseStatsDto,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingStatsDto {
    pub waiting_for_publication: i64,
    pub awaiting_review: i64,
    pub published_last_hour: i64,
    pub published_last_day: i64,
    pub imported_last_hour: i64,
    pub imported_last_day: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatsDto {
    pub artists: i64,
    pub releases: i64,
    pub tracks: i64,
    pub media_assets: i64,
    pub total_bytes: i64,
    pub total_duration_ms: i64,
    pub average_track_duration_ms: i64,
    pub lossless_tracks: i64,
    pub formats: Vec<FormatStatsDto>,
    pub decades: Vec<DecadeStatsDto>,
    pub largest_releases: Vec<ReleaseSizeDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatStatsDto {
    pub container: String,
    pub codec: String,
    pub tracks: i64,
    pub bytes: i64,
    pub lossless: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecadeStatsDto {
    pub decade: i32,
    pub releases: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseSizeDto {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub tracks: i64,
    pub bytes: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningStatsDto {
    pub total_plays: i64,
    pub total_playtime_seconds: i64,
    pub unique_tracks: i64,
    pub unique_listeners: i64,
    pub plays_per_day: Vec<DayActivityDto>,
    pub top_artists: Vec<TopArtistDto>,
    pub top_tracks: Vec<TopTrackDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayActivityDto {
    pub day: String,
    pub plays: i64,
    pub seconds: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopArtistDto {
    pub name: String,
    pub plays: i64,
    pub seconds: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopTrackDto {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub plays: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseStatsDto {
    pub total_bytes: i64,
    pub asset_file_bytes: i64,
    pub asset_file_count: i64,
    pub tables: Vec<TableSizeDto>,
    pub work_items: i64,
    pub play_events: i64,
    pub pending_projection: i64,
    /// PostgreSQL pool connections: total capacity, and how many are in use.
    pub pool_capacity: i64,
    pub pool_in_use: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableSizeDto {
    pub name: String,
    pub bytes: i64,
}

async fn stats(State(state): State<AdminSystemState>, _admin: AdminPrincipal) -> Response {
    match build_stats(&state.admin.pool).await {
        Ok(dto) => Json(dto).into_response(),
        Err(error) => {
            tracing::error!(%error, "admin stats aggregation failed");
            crate::admin_access::admin_service_error(
                "stats_unavailable",
                "statistics are temporarily unavailable",
            )
        }
    }
}

async fn build_stats(pool: &sqlx::PgPool) -> Result<StatsDto, sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    let library = library_stats(pool).await?;
    let listening = listening_stats(pool, now).await?;
    let database = database_stats(pool).await?;
    Ok(StatsDto {
        generated_at: now
            .format(&Rfc3339)
            .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z")),
        library,
        processing: processing_stats(pool, now).await?,
        listening,
        database,
    })
}

async fn processing_stats(
    pool: &sqlx::PgPool,
    as_of: OffsetDateTime,
) -> Result<ProcessingStatsDto, sqlx::Error> {
    // Receipts and imports are unique per work item. Retries and later state changes
    // must not inflate completions or remove a Release from its original time window.
    sqlx::query_as(
        "SELECT queue.*, publication.*, catalog.* FROM
          (SELECT count(*) FILTER (WHERE state = 'staged') AS waiting_for_publication,
                  count(*) FILTER (WHERE state = 'needs_review') AS awaiting_review
             FROM release_work_items WHERE state IN ('staged', 'needs_review')) AS queue
          CROSS JOIN (SELECT count(*) FILTER (WHERE published_at > $2) AS published_last_hour,
                             count(*) AS published_last_day
                        FROM release_publication_receipts WHERE published_at > $3 AND published_at <= $1) AS publication
          CROSS JOIN (SELECT count(*) FILTER (WHERE imported_at > $2) AS imported_last_hour,
                             count(*) AS imported_last_day
                        FROM catalog_imports WHERE imported_at > $3 AND imported_at <= $1) AS catalog",
    )
    .bind(as_of)
    .bind(as_of - time::Duration::hours(1))
    .bind(as_of - time::Duration::days(1))
    .fetch_one(pool)
    .await
}

async fn library_stats(pool: &sqlx::PgPool) -> Result<LibraryStatsDto, sqlx::Error> {
    // One bounded pass for the headline counts and totals.
    let (
        artists,
        releases,
        tracks,
        media_assets,
        total_bytes,
        total_duration_ms,
        lossless_tracks,
    ): (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM artists WHERE status <> 'merged'),
           (SELECT count(*) FROM releases),
           (SELECT count(*) FROM tracks),
           (SELECT count(*) FROM media_assets),
           (SELECT COALESCE(sum(byte_size), 0)::bigint FROM media_assets),
           (SELECT COALESCE(sum(duration_ms), 0)::bigint FROM tracks),
           (SELECT count(*) FROM media_assets
             WHERE codec IS NOT NULL
               AND lower(codec) IN ('flac','alac','wav','pcm_s16le','pcm_s24le','ape','wv'))",
    )
    .fetch_one(pool)
    .await?;

    let formats = sqlx::query_as::<_, (Option<String>, Option<String>, i64, i64)>(
        "SELECT container, codec, count(*), COALESCE(sum(byte_size), 0)::bigint
           FROM media_assets
          GROUP BY container, codec
          ORDER BY count(*) DESC, container, codec
          LIMIT $1",
    )
    .bind(MAX_BREAKDOWN_ROWS)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(container, codec, count, bytes)| {
        let codec = codec.unwrap_or_else(|| "unknown".to_owned());
        let lossless = matches!(
            codec.to_ascii_lowercase().as_str(),
            "flac" | "alac" | "wav" | "pcm_s16le" | "pcm_s24le" | "ape" | "wv"
        );
        FormatStatsDto {
            container: container.unwrap_or_else(|| "unknown".to_owned()),
            codec,
            tracks: count,
            bytes,
            lossless,
        }
    })
    .collect();

    // Release years come from the typed date when present, else from the observed
    // date text (`2019`, `2019-04-12`), which is what most tagged releases carry.
    let decades = sqlx::query_as::<_, (i32, i64)>(
        "WITH observed AS (
           SELECT COALESCE(
                    extract(year FROM release_date)::int,
                    CASE WHEN release_date_text ~ '^[0-9]{4}'
                         THEN left(release_date_text, 4)::int END
                  ) AS year
             FROM releases
         )
         SELECT (year / 10) * 10 AS decade, count(*)
           FROM observed
          WHERE year IS NOT NULL AND year BETWEEN 1900 AND 2200
          GROUP BY 1
          ORDER BY 1 DESC
          LIMIT $1",
    )
    .bind(MAX_BREAKDOWN_ROWS)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(decade, releases)| DecadeStatsDto { decade, releases })
    .collect();

    let largest_releases = sqlx::query_as::<_, (uuid::Uuid, String, Option<String>, i64, i64)>(
        "SELECT release.public_api_id,
                release.title,
                release.display_artist,
                count(track.id)::bigint AS tracks,
                COALESCE(sum(asset.byte_size), 0)::bigint AS bytes
           FROM releases release
           JOIN tracks track ON track.release_id = release.id
           LEFT JOIN media_assets asset ON asset.id = track.playable_media_asset_id
          GROUP BY release.id, release.public_api_id, release.title, release.display_artist
          ORDER BY bytes DESC, release.title
          LIMIT $1",
    )
    .bind(MAX_TOP_ROWS)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, title, artist, tracks, bytes)| ReleaseSizeDto {
        id: id.to_string(),
        title,
        artist,
        tracks,
        bytes,
    })
    .collect();

    Ok(LibraryStatsDto {
        artists,
        releases,
        tracks,
        media_assets,
        total_bytes,
        total_duration_ms,
        average_track_duration_ms: if tracks > 0 {
            total_duration_ms / tracks
        } else {
            0
        },
        lossless_tracks,
        formats,
        decades,
        largest_releases,
    })
}

async fn listening_stats(
    pool: &sqlx::PgPool,
    now: OffsetDateTime,
) -> Result<ListeningStatsDto, sqlx::Error> {
    let since = now - time::Duration::days(LISTENING_WINDOW_DAYS);

    let (total_plays, total_playtime_seconds, unique_tracks, unique_listeners): (
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT count(*),
                COALESCE(sum(COALESCE(duration_seconds, 0)), 0)::bigint,
                count(DISTINCT track_id),
                count(DISTINCT user_id)
           FROM native_track_plays",
    )
    .fetch_one(pool)
    .await?;

    // Zero-filled day spine: a day with no plays is a data point, not a gap.
    let plays_per_day = sqlx::query_as::<_, (time::Date, i64, i64)>(
        "WITH spine AS (
           SELECT generate_series(
                    (date_trunc('day', $1::timestamptz))::date,
                    (date_trunc('day', now()))::date,
                    interval '1 day'
                  )::date AS day
         ),
         played AS (
           SELECT played_at::date AS day,
                  count(*)::bigint AS plays,
                  COALESCE(sum(COALESCE(duration_seconds, 0)), 0)::bigint AS seconds
             FROM native_track_plays
            WHERE played_at >= date_trunc('day', $1::timestamptz)
            GROUP BY 1
         )
         SELECT spine.day, COALESCE(played.plays, 0), COALESCE(played.seconds, 0)
           FROM spine LEFT JOIN played USING (day)
          ORDER BY 1",
    )
    .bind(since)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(day, plays, seconds)| DayActivityDto {
        day: day.to_string(),
        plays,
        seconds,
    })
    .collect();

    let top_artists = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT artist.canonical_name,
                count(*)::bigint AS plays,
                COALESCE(sum(COALESCE(play.duration_seconds, 0)), 0)::bigint AS seconds
           FROM native_track_plays play
           JOIN tracks track ON track.id = play.track_id
           JOIN track_artists billing ON billing.track_id = track.id AND billing.role = 'primary'
           JOIN artists artist ON artist.id = billing.artist_id
          GROUP BY artist.id, artist.canonical_name
          ORDER BY plays DESC, artist.canonical_name
          LIMIT $1",
    )
    .bind(MAX_TOP_ROWS)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(name, plays, seconds)| TopArtistDto {
        name,
        plays,
        seconds,
    })
    .collect();

    let top_tracks = sqlx::query_as::<_, (uuid::Uuid, String, Option<String>, i64)>(
        "SELECT track.public_api_id, track.title, track.display_artist, count(*)::bigint AS plays
           FROM native_track_plays play
           JOIN tracks track ON track.id = play.track_id
          GROUP BY track.id, track.public_api_id, track.title, track.display_artist
          ORDER BY plays DESC, track.title
          LIMIT $1",
    )
    .bind(MAX_TOP_ROWS)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, title, artist, plays)| TopTrackDto {
        id: id.to_string(),
        title,
        artist,
        plays,
    })
    .collect();

    Ok(ListeningStatsDto {
        total_plays,
        total_playtime_seconds,
        unique_tracks,
        unique_listeners,
        plays_per_day,
        top_artists,
        top_tracks,
    })
}

async fn database_stats(pool: &sqlx::PgPool) -> Result<DatabaseStatsDto, sqlx::Error> {
    let (total_bytes, work_items, play_events, pending_projection): (i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT pg_database_size(current_database())::bigint,
                    (SELECT count(*) FROM release_work_items),
                    (SELECT count(*) FROM native_track_plays),
                    (SELECT count(*) FROM catalog_projection_outbox WHERE processed_at IS NULL)",
        )
        .fetch_one(pool)
        .await?;

    let (asset_file_count, asset_file_bytes): (i64, i64) =
        sqlx::query_as("SELECT count(*), COALESCE(sum(size_bytes),0) FROM asset_files")
            .fetch_one(pool)
            .await?;
    // Include indexes and TOAST for every application table, then return the largest.
    let tables = sqlx::query_as::<_, (String, i64)>(
        "SELECT relation.relname,
                pg_total_relation_size(relation.oid)::bigint AS bytes
           FROM pg_class relation
           JOIN pg_namespace namespace ON namespace.oid = relation.relnamespace
          WHERE namespace.nspname = current_schema()
            AND relation.relkind = 'r'
          ORDER BY bytes DESC, relation.relname LIMIT 14",
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(name, bytes)| TableSizeDto { name, bytes })
    .collect();

    let (pool_capacity, pool_in_use): (i64, i64) = sqlx::query_as(
        "SELECT current_setting('max_connections')::bigint,
                count(*)::bigint
           FROM pg_stat_activity
          WHERE datname = current_database()",
    )
    .fetch_one(pool)
    .await?;

    Ok(DatabaseStatsDto {
        total_bytes,
        asset_file_count,
        asset_file_bytes,
        tables,
        work_items,
        play_events,
        pending_projection,
        pool_capacity,
        pool_in_use,
    })
}
