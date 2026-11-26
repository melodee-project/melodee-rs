use std::collections::BTreeSet;

use sqlx::{PgPool, Postgres, Transaction};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_CHART_CSV_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_CHART_ITEMS: usize = 5_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeChartImport {
    pub slug: String,
    pub title: String,
    pub source_name: Option<String>,
    pub source_url: Option<String>,
    pub chart_year: Option<i32>,
    pub is_visible: bool,
    pub is_generated_playlist_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeChartImportResult {
    pub chart_id: Uuid,
    pub item_count: usize,
    pub linked_count: usize,
}

#[derive(Debug, Error)]
pub enum NativeDiscoveryStoreError {
    #[error("invalid chart CSV: {0}")]
    InvalidCsv(String),
    #[error("invalid chart metadata: {0}")]
    InvalidMetadata(&'static str),
    #[error("chart slug already exists")]
    DuplicateSlug,
    #[error("chart persistence failed")]
    Sql(#[source] sqlx::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChartCsvItem {
    rank: i32,
    artist_name: String,
    release_title: String,
    release_year: Option<i32>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NativeDiscoveryStore;

impl NativeDiscoveryStore {
    /// Imports a bounded chart snapshot and links only unique exact catalog
    /// matches. Ambiguous names stay unlinked because names are never semantic
    /// identity in Melodee.
    pub async fn import_chart_csv(
        &self,
        pool: &PgPool,
        request: &NativeChartImport,
        bytes: &[u8],
    ) -> Result<NativeChartImportResult, NativeDiscoveryStoreError> {
        validate_chart_metadata(request)?;
        let items = parse_chart_csv(bytes)?;
        let chart_id = Uuid::now_v7();
        let mut transaction = pool.begin().await.map_err(map_sql)?;
        let inserted = sqlx::query(
            "INSERT INTO native_charts(\
               id,public_api_id,slug,title,source_name,source_url,chart_year,\
               is_visible,is_generated_playlist_enabled,created_at,updated_at\
             ) VALUES($1,$1,$2,$3,$4,$5,$6,$7,$8,now(),now())",
        )
        .bind(chart_id)
        .bind(request.slug.trim())
        .bind(request.title.trim())
        .bind(request.source_name.as_deref().map(str::trim))
        .bind(request.source_url.as_deref().map(str::trim))
        .bind(request.chart_year)
        .bind(request.is_visible)
        .bind(request.is_generated_playlist_enabled)
        .execute(&mut *transaction)
        .await;
        if let Err(error) = inserted {
            if error
                .as_database_error()
                .is_some_and(|database| database.constraint() == Some("native_charts_slug_key"))
            {
                return Err(NativeDiscoveryStoreError::DuplicateSlug);
            }
            return Err(map_sql(error));
        }

        let ranks = items.iter().map(|item| item.rank).collect::<Vec<_>>();
        let artists = items
            .iter()
            .map(|item| item.artist_name.as_str())
            .collect::<Vec<_>>();
        let releases = items
            .iter()
            .map(|item| item.release_title.as_str())
            .collect::<Vec<_>>();
        let years = items
            .iter()
            .map(|item| item.release_year)
            .collect::<Vec<_>>();
        sqlx::query(
            "INSERT INTO native_chart_items(\
               chart_id,rank,artist_name,release_title,release_year,created_at\
             )\
             SELECT $1,item.rank,item.artist_name,item.release_title,item.release_year,now()\
             FROM unnest($2::integer[],$3::text[],$4::text[],$5::integer[]) \
               AS item(rank,artist_name,release_title,release_year)\
             ORDER BY item.rank",
        )
        .bind(chart_id)
        .bind(&ranks)
        .bind(&artists)
        .bind(&releases)
        .bind(&years)
        .execute(&mut *transaction)
        .await
        .map_err(map_sql)?;
        let linked_count = sqlx::query(
            r#"UPDATE native_chart_items item SET
                 (linked_artist_id,linked_release_id)=(
                   SELECT artist.id,release.id
                   FROM artists artist
                   JOIN release_artists credit
                     ON credit.artist_id=artist.id AND credit.role='primary'
                   JOIN releases release
                     ON release.id=credit.release_id AND release.status='active'
                   WHERE lower(btrim(artist.canonical_name))=lower(btrim(item.artist_name))
                     AND lower(btrim(release.title))=lower(btrim(item.release_title))
                     AND (item.release_year IS NULL OR
                       EXTRACT(YEAR FROM release.release_date)::integer=item.release_year)
                   ORDER BY release.id LIMIT 1)
               WHERE item.chart_id=$1 AND 1=(
                 SELECT count(*) FROM artists artist
                 JOIN release_artists credit
                   ON credit.artist_id=artist.id AND credit.role='primary'
                 JOIN releases release
                   ON release.id=credit.release_id AND release.status='active'
                 WHERE lower(btrim(artist.canonical_name))=lower(btrim(item.artist_name))
                   AND lower(btrim(release.title))=lower(btrim(item.release_title))
                   AND (item.release_year IS NULL OR
                     EXTRACT(YEAR FROM release.release_date)::integer=item.release_year))"#,
        )
        .bind(chart_id)
        .execute(&mut *transaction)
        .await
        .map_err(map_sql)?
        .rows_affected();
        transaction.commit().await.map_err(map_sql)?;
        let item_count = items.len();
        let linked_count = usize::try_from(linked_count).map_err(|_| {
            NativeDiscoveryStoreError::InvalidCsv("linked row count overflow".to_owned())
        })?;
        Ok(NativeChartImportResult {
            chart_id,
            item_count,
            linked_count,
        })
    }
}
/// Observable result of one bounded chart/recommendation maintenance pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeDiscoveryMaintenanceStats {
    pub candidates_inserted: u64,
    pub candidates_corrected: u64,
    pub chart_items_relinked: u64,
    pub batches: u32,
    /// `true` when a work-class batch cap was reached while rows remained; the
    /// durable cursor resumes that class on the next pass.
    pub truncated: bool,
    /// Work classes that advanced their durable cursor during this pass.
    pub classes_advanced: u32,
    /// Work classes that reached the end of the current sweep and wrapped.
    pub passes_completed: u32,
}

const RECOMMENDATION_ENTITY_TYPES: [&str; 3] = ["track", "release", "artist"];

/// Transaction-scoped fence serializing every discovery work class across
/// processes. A concurrent pass either waits for the current batch or, after
/// the current transaction commits, resumes from the cursor that batch stored.
/// Cursors therefore cannot regress or overwrite each other.
const DISCOVERY_PASS_LOCK_KEY: i64 = (19_761_125_i64 << 32) | 311;

/// Five independent work classes share the configured per-pass batch cap:
/// missing-candidate backfill, three popularity classes, and chart relinking.
/// A large Track population can no longer consume the whole budget and starve
/// Releases, Artists, or charts.
fn per_class_budget(max_batches: u32) -> u32 {
    (max_batches / 5).max(1)
}

#[derive(Debug, Clone, Copy, Default)]
struct DiscoveryProgress {
    cursor_uuid: Option<Uuid>,
    cursor_chart_id: Option<Uuid>,
    cursor_rank: Option<i32>,
}

impl NativeDiscoveryStore {
    /// Reconciles the materialized discovery projections against durable
    /// catalog and listening facts in bounded, idempotent, set-based batches.
    ///
    /// `native_recommendation_candidates` is trigger-maintained; this pass is
    /// the convergence backstop for rows missed or drifted while triggers
    /// could not run (backup restore, binary downgrade window). It re-adds
    /// missing catalog candidates and recomputes popularity from
    /// `native_track_plays` only where the stored value differs. Unlinked
    /// chart items are retried against the catalog under the same
    /// unique-exact-match rule as import; existing links are never rewritten
    /// or cleared because names are never semantic identity.
    ///
    /// PERF-012: each work class keeps its own durable keyset cursor and its
    /// own bounded budget. Progress is stored in the same transaction as the
    /// batch effect, so a crash replays an idempotent batch instead of
    /// skipping it, and a completed sweep wraps to `nil` so rows inserted
    /// behind the cursor are reached on the next sweep.
    pub async fn refresh_discovery_aggregates(
        &self,
        pool: &PgPool,
        batch_size: u32,
        max_batches: u32,
    ) -> Result<NativeDiscoveryMaintenanceStats, NativeDiscoveryStoreError> {
        if batch_size == 0 || max_batches == 0 {
            return Err(NativeDiscoveryStoreError::InvalidMetadata(
                "discovery batch bounds must be positive",
            ));
        }
        let started = std::time::Instant::now();
        let per_class = per_class_budget(max_batches);
        let mut stats = NativeDiscoveryMaintenanceStats::default();

        // The missing-candidate backstop stays a periodic bounded anti-join.
        // Ordinary insert triggers cover normal creation, but a backup restore
        // or binary downgrade can leave rows missing, so this pass is retained
        // rather than replaced by triggers alone.
        let missing_per_type = per_class.div_ceil(RECOMMENDATION_ENTITY_TYPES.len() as u32);
        for entity_type in RECOMMENDATION_ENTITY_TYPES {
            insert_missing_candidates(pool, entity_type, batch_size, missing_per_type, &mut stats)
                .await?;
        }
        for entity_type in RECOMMENDATION_ENTITY_TYPES {
            correct_candidate_popularity(pool, entity_type, batch_size, per_class, &mut stats)
                .await?;
        }
        relink_chart_items(pool, batch_size, per_class, &mut stats).await?;
        crate::metrics::observe(crate::metrics::Metric::DiscoveryPass, started.elapsed());
        Ok(stats)
    }
}

async fn load_progress(
    tx: &mut Transaction<'_, Postgres>,
    work_class: &str,
) -> Result<DiscoveryProgress, NativeDiscoveryStoreError> {
    let row = sqlx::query_as::<_, (Option<Uuid>, Option<Uuid>, Option<i32>)>(
        "SELECT cursor_uuid, cursor_chart_id, cursor_rank
           FROM native_discovery_progress WHERE work_class = $1",
    )
    .bind(work_class)
    .fetch_optional(&mut **tx)
    .await
    .map_err(map_sql)?;
    let (cursor_uuid, cursor_chart_id, cursor_rank) = row.unwrap_or((None, None, None));
    Ok(DiscoveryProgress {
        cursor_uuid,
        cursor_chart_id,
        cursor_rank,
    })
}

async fn store_progress(
    tx: &mut Transaction<'_, Postgres>,
    work_class: &str,
    progress: DiscoveryProgress,
    pass_completed: bool,
) -> Result<(), NativeDiscoveryStoreError> {
    sqlx::query(
        "INSERT INTO native_discovery_progress (
             work_class, cursor_uuid, cursor_chart_id, cursor_rank,
             pass_completed, updated_at
         ) VALUES ($1,$2,$3,$4,$5,now())
         ON CONFLICT (work_class) DO UPDATE SET
             cursor_uuid = EXCLUDED.cursor_uuid,
             cursor_chart_id = EXCLUDED.cursor_chart_id,
             cursor_rank = EXCLUDED.cursor_rank,
             pass_completed = EXCLUDED.pass_completed,
             updated_at = EXCLUDED.updated_at",
    )
    .bind(work_class)
    .bind(progress.cursor_uuid)
    .bind(progress.cursor_chart_id)
    .bind(progress.cursor_rank)
    .bind(pass_completed)
    .execute(&mut **tx)
    .await
    .map_err(map_sql)?;
    Ok(())
}

fn missing_candidate_insert_sql(entity_type: &str) -> &'static str {
    match entity_type {
        "track" => {
            "INSERT INTO native_recommendation_candidates(\
               entity_type,entity_id,popularity,updated_at) \
             SELECT 'track',track.id,0,now() FROM tracks track \
             WHERE NOT EXISTS(SELECT 1 FROM native_recommendation_candidates candidate \
               WHERE candidate.entity_type='track' AND candidate.entity_id=track.id) \
             ORDER BY track.id LIMIT $1 \
             ON CONFLICT(entity_type,entity_id) DO NOTHING"
        }
        "release" => {
            "INSERT INTO native_recommendation_candidates(\
               entity_type,entity_id,popularity,updated_at) \
             SELECT 'release',release.id,0,now() FROM releases release \
             WHERE NOT EXISTS(SELECT 1 FROM native_recommendation_candidates candidate \
               WHERE candidate.entity_type='release' AND candidate.entity_id=release.id) \
             ORDER BY release.id LIMIT $1 \
             ON CONFLICT(entity_type,entity_id) DO NOTHING"
        }
        _ => {
            "INSERT INTO native_recommendation_candidates(\
               entity_type,entity_id,popularity,updated_at) \
             SELECT 'artist',artist.id,0,now() FROM artists artist \
             WHERE NOT EXISTS(SELECT 1 FROM native_recommendation_candidates candidate \
               WHERE candidate.entity_type='artist' AND candidate.entity_id=artist.id) \
             ORDER BY artist.id LIMIT $1 \
             ON CONFLICT(entity_type,entity_id) DO NOTHING"
        }
    }
}

async fn insert_missing_candidates(
    pool: &PgPool,
    entity_type: &str,
    batch_size: u32,
    budget: u32,
    stats: &mut NativeDiscoveryMaintenanceStats,
) -> Result<(), NativeDiscoveryStoreError> {
    let sql = missing_candidate_insert_sql(entity_type);
    let mut used = 0u32;
    loop {
        if used >= budget {
            stats.truncated = true;
            return Ok(());
        }
        let inserted = sqlx::query(sql)
            .bind(i64::from(batch_size))
            .execute(pool)
            .await
            .map_err(map_sql)?
            .rows_affected();
        used = used.saturating_add(1);
        stats.batches = stats.batches.saturating_add(1);
        stats.candidates_inserted = stats.candidates_inserted.saturating_add(inserted);
        if inserted == 0 {
            return Ok(());
        }
    }
}

fn candidate_popularity_correction_sql(entity_type: &str) -> &'static str {
    match entity_type {
        "track" => {
            "WITH actual AS (\
               SELECT candidate.entity_id,count(play.id) AS popularity \
               FROM native_recommendation_candidates candidate \
               LEFT JOIN native_track_plays play ON play.track_id=candidate.entity_id \
               WHERE candidate.entity_type='track' AND candidate.entity_id=ANY($1) \
               GROUP BY candidate.entity_id) \
             UPDATE native_recommendation_candidates candidate \
             SET popularity=actual.popularity,updated_at=now() \
             FROM actual \
             WHERE candidate.entity_type='track' AND candidate.entity_id=actual.entity_id \
               AND candidate.popularity<>actual.popularity"
        }
        "release" => {
            "WITH actual AS (\
               SELECT candidate.entity_id,count(play.id) AS popularity \
               FROM native_recommendation_candidates candidate \
               LEFT JOIN tracks track ON track.release_id=candidate.entity_id \
               LEFT JOIN native_track_plays play ON play.track_id=track.id \
               WHERE candidate.entity_type='release' AND candidate.entity_id=ANY($1) \
               GROUP BY candidate.entity_id) \
             UPDATE native_recommendation_candidates candidate \
             SET popularity=actual.popularity,updated_at=now() \
             FROM actual \
             WHERE candidate.entity_type='release' AND candidate.entity_id=actual.entity_id \
               AND candidate.popularity<>actual.popularity"
        }
        _ => {
            "WITH actual AS (\
               SELECT candidate.entity_id,count(play.id) AS popularity \
               FROM native_recommendation_candidates candidate \
               LEFT JOIN track_artists billing \
                 ON billing.artist_id=candidate.entity_id AND billing.role='primary' \
               LEFT JOIN native_track_plays play ON play.track_id=billing.track_id \
               WHERE candidate.entity_type='artist' AND candidate.entity_id=ANY($1) \
               GROUP BY candidate.entity_id) \
             UPDATE native_recommendation_candidates candidate \
             SET popularity=actual.popularity,updated_at=now() \
             FROM actual \
             WHERE candidate.entity_type='artist' AND candidate.entity_id=actual.entity_id \
               AND candidate.popularity<>actual.popularity"
        }
    }
}

async fn correct_candidate_popularity(
    pool: &PgPool,
    entity_type: &str,
    batch_size: u32,
    budget: u32,
    stats: &mut NativeDiscoveryMaintenanceStats,
) -> Result<(), NativeDiscoveryStoreError> {
    let work_class = format!("{entity_type}_popularity");
    let correction_sql = candidate_popularity_correction_sql(entity_type);
    let limit = i64::from(batch_size);
    let mut used = 0u32;
    loop {
        if used >= budget {
            stats.truncated = true;
            return Ok(());
        }
        let mut tx = pool.begin().await.map_err(map_sql)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(DISCOVERY_PASS_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(map_sql)?;
        let progress = load_progress(&mut tx, &work_class).await?;
        let batch = sqlx::query_scalar::<_, Uuid>(
            "SELECT entity_id FROM native_recommendation_candidates \
             WHERE entity_type=$1 AND entity_id>$2 ORDER BY entity_id LIMIT $3",
        )
        .bind(entity_type)
        .bind(progress.cursor_uuid.unwrap_or_else(Uuid::nil))
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_sql)?;
        used = used.saturating_add(1);
        stats.batches = stats.batches.saturating_add(1);
        let Some(last) = batch.last().copied() else {
            // Sweep complete: wrap so rows inserted behind the cursor are
            // visited on the next sweep. Durable with this transaction.
            store_progress(&mut tx, &work_class, DiscoveryProgress::default(), true).await?;
            tx.commit().await.map_err(map_sql)?;
            stats.passes_completed = stats.passes_completed.saturating_add(1);
            return Ok(());
        };
        let corrected = sqlx::query(correction_sql)
            .bind(&batch)
            .execute(&mut *tx)
            .await
            .map_err(map_sql)?
            .rows_affected();
        stats.candidates_corrected = stats.candidates_corrected.saturating_add(corrected);
        let complete = batch.len() < usize::try_from(batch_size).unwrap_or(usize::MAX);
        let next = if complete {
            DiscoveryProgress::default()
        } else {
            DiscoveryProgress {
                cursor_uuid: Some(last),
                ..DiscoveryProgress::default()
            }
        };
        store_progress(&mut tx, &work_class, next, complete).await?;
        tx.commit().await.map_err(map_sql)?;
        stats.classes_advanced = stats.classes_advanced.saturating_add(1);
        if complete {
            stats.passes_completed = stats.passes_completed.saturating_add(1);
            return Ok(());
        }
    }
}

async fn relink_chart_items(
    pool: &PgPool,
    batch_size: u32,
    budget: u32,
    stats: &mut NativeDiscoveryMaintenanceStats,
) -> Result<(), NativeDiscoveryStoreError> {
    const WORK_CLASS: &str = "chart_relink";
    let limit = i64::from(batch_size);
    let mut used = 0u32;
    loop {
        if used >= budget {
            stats.truncated = true;
            return Ok(());
        }
        let mut tx = pool.begin().await.map_err(map_sql)?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(DISCOVERY_PASS_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(map_sql)?;
        let progress = load_progress(&mut tx, WORK_CLASS).await?;
        let after_chart = progress.cursor_chart_id.unwrap_or_else(Uuid::nil);
        let after_rank = progress.cursor_rank.unwrap_or(-1);
        let batch = sqlx::query_as::<_, (Uuid, i32)>(
            "SELECT chart_id, rank FROM native_chart_items
             WHERE (linked_artist_id IS NULL OR linked_release_id IS NULL)
               AND (chart_id, rank) > ($1, $2)
             ORDER BY chart_id, rank LIMIT $3",
        )
        .bind(after_chart)
        .bind(after_rank)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(map_sql)?;
        used = used.saturating_add(1);
        stats.batches = stats.batches.saturating_add(1);
        let Some(&(last_chart, last_rank)) = batch.last() else {
            store_progress(&mut tx, WORK_CLASS, DiscoveryProgress::default(), true).await?;
            tx.commit().await.map_err(map_sql)?;
            stats.passes_completed = stats.passes_completed.saturating_add(1);
            return Ok(());
        };
        // Only unique exact matches link, exactly as at import time. Rows that
        // stay ambiguous remain unlinked; later sweeps retry them as the
        // catalog changes. The cursor still advances past them so a run of
        // permanently ambiguous rows cannot starve the rest of the chart.
        let relinked = sqlx::query(
            r#"WITH batch AS (
                 SELECT item.chart_id,item.rank
                 FROM native_chart_items item
                 WHERE (item.linked_artist_id IS NULL OR item.linked_release_id IS NULL)
                   AND (item.chart_id, item.rank) > ($1, $2)
                 ORDER BY item.chart_id,item.rank LIMIT $3)
               UPDATE native_chart_items item SET
                 (linked_artist_id,linked_release_id)=(
                   SELECT artist.id,release.id
                   FROM artists artist
                   JOIN release_artists credit
                     ON credit.artist_id=artist.id AND credit.role='primary'
                   JOIN releases release
                     ON release.id=credit.release_id AND release.status='active'
                   WHERE lower(btrim(artist.canonical_name))=lower(btrim(item.artist_name))
                     AND lower(btrim(release.title))=lower(btrim(item.release_title))
                     AND (item.release_year IS NULL OR
                       EXTRACT(YEAR FROM release.release_date)::integer=item.release_year)
                   ORDER BY release.id LIMIT 1)
               FROM batch
               WHERE item.chart_id=batch.chart_id AND item.rank=batch.rank AND 1=(
                 SELECT count(*) FROM artists artist
                 JOIN release_artists credit
                   ON credit.artist_id=artist.id AND credit.role='primary'
                 JOIN releases release
                   ON release.id=credit.release_id AND release.status='active'
                 WHERE lower(btrim(artist.canonical_name))=lower(btrim(item.artist_name))
                   AND lower(btrim(release.title))=lower(btrim(item.release_title))
                   AND (item.release_year IS NULL OR
                     EXTRACT(YEAR FROM release.release_date)::integer=item.release_year))"#,
        )
        .bind(after_chart)
        .bind(after_rank)
        .bind(limit)
        .execute(&mut *tx)
        .await
        .map_err(map_sql)?
        .rows_affected();
        stats.chart_items_relinked = stats.chart_items_relinked.saturating_add(relinked);
        let complete = batch.len() < usize::try_from(batch_size).unwrap_or(usize::MAX);
        let next = if complete {
            DiscoveryProgress::default()
        } else {
            DiscoveryProgress {
                cursor_chart_id: Some(last_chart),
                cursor_rank: Some(last_rank),
                ..DiscoveryProgress::default()
            }
        };
        store_progress(&mut tx, WORK_CLASS, next, complete).await?;
        tx.commit().await.map_err(map_sql)?;
        stats.classes_advanced = stats.classes_advanced.saturating_add(1);
        if complete {
            stats.passes_completed = stats.passes_completed.saturating_add(1);
            return Ok(());
        }
    }
}

fn validate_chart_metadata(request: &NativeChartImport) -> Result<(), NativeDiscoveryStoreError> {
    let slug = request.slug.trim();
    if slug.is_empty()
        || slug.len() > 255
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(NativeDiscoveryStoreError::InvalidMetadata(
            "slug must contain 1..255 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    if request.title.trim().is_empty() || request.title.len() > 500 {
        return Err(NativeDiscoveryStoreError::InvalidMetadata(
            "title must contain 1..500 bytes",
        ));
    }
    if request
        .source_name
        .as_ref()
        .is_some_and(|value| value.trim().is_empty() || value.len() > 500)
        || request
            .source_url
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 4096)
    {
        return Err(NativeDiscoveryStoreError::InvalidMetadata(
            "source metadata is empty or exceeds its storage bound",
        ));
    }
    Ok(())
}

fn parse_chart_csv(bytes: &[u8]) -> Result<Vec<ChartCsvItem>, NativeDiscoveryStoreError> {
    if bytes.is_empty() || bytes.len() > MAX_CHART_CSV_BYTES {
        return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
            "input must contain 1..={MAX_CHART_CSV_BYTES} bytes"
        )));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| NativeDiscoveryStoreError::InvalidCsv("input must be UTF-8".to_owned()))?;
    let mut items = Vec::new();
    let mut seen_ranks = BTreeSet::new();
    for (line_index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let columns = parse_csv_line(line).map_err(|message| {
            NativeDiscoveryStoreError::InvalidCsv(format!("row {}: {message}", line_index + 1))
        })?;
        if items.is_empty()
            && columns
                .first()
                .is_some_and(|value| value.eq_ignore_ascii_case("rank"))
        {
            continue;
        }
        if columns.len() < 3 || columns.len() > 4 {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: expected Rank, ArtistName, ReleaseTitle, optional ReleaseYear",
                line_index + 1
            )));
        }
        let rank = columns[0].trim().parse::<i32>().map_err(|_| {
            NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: rank must be a positive integer",
                line_index + 1
            ))
        })?;
        if rank < 1 || !seen_ranks.insert(rank) {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: rank must be positive and unique",
                line_index + 1
            )));
        }
        let artist_name = columns[1].trim().to_owned();
        let release_title = columns[2].trim().to_owned();
        if artist_name.is_empty()
            || release_title.is_empty()
            || artist_name.len() > 500
            || release_title.len() > 500
            || artist_name.contains('\0')
            || release_title.contains('\0')
        {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "row {}: artist and release names must contain 1..500 bytes",
                line_index + 1
            )));
        }
        let release_year = columns
            .get(3)
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                value.trim().parse::<i32>().map_err(|_| {
                    NativeDiscoveryStoreError::InvalidCsv(format!(
                        "row {}: release year must be an integer",
                        line_index + 1
                    ))
                })
            })
            .transpose()?;
        items.push(ChartCsvItem {
            rank,
            artist_name,
            release_title,
            release_year,
        });
        if items.len() > MAX_CHART_ITEMS {
            return Err(NativeDiscoveryStoreError::InvalidCsv(format!(
                "input exceeds {MAX_CHART_ITEMS} items"
            )));
        }
    }
    if items.is_empty() {
        return Err(NativeDiscoveryStoreError::InvalidCsv(
            "input contains no chart items".to_owned(),
        ));
    }
    Ok(items)
}

fn parse_csv_line(line: &str) -> Result<Vec<String>, &'static str> {
    let mut columns = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '"' if quoted && chars.peek() == Some(&'"') => {
                if let Some(column) = columns.last_mut() {
                    column.push('"');
                }
                let _ = chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => columns.push(String::new()),
            value => {
                if let Some(column) = columns.last_mut() {
                    column.push(value);
                }
            }
        }
    }
    if quoted {
        return Err("unterminated quoted field");
    }
    Ok(columns)
}

fn map_sql(error: sqlx::Error) -> NativeDiscoveryStoreError {
    NativeDiscoveryStoreError::Sql(error)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{MAX_CHART_CSV_BYTES, parse_chart_csv};

    #[test]
    fn chart_csv_parser_handles_headers_quotes_and_rejects_ambiguity() {
        let items = parse_chart_csv(
            b"Rank,ArtistName,ReleaseTitle,ReleaseYear\n1,\"Artist, The\",\"A \"\"Release\"\"\",2020\n",
        )
        .expect("valid CSV");
        assert_eq!(items[0].artist_name, "Artist, The");
        assert_eq!(items[0].release_title, "A \"Release\"");
        assert!(parse_chart_csv(b"1,Artist,Release\n1,Other,Other").is_err());
        assert!(parse_chart_csv(&vec![b'x'; MAX_CHART_CSV_BYTES + 1]).is_err());
    }
}
