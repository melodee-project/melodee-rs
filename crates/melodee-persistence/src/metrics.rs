//! Bounded in-process performance metrics (PERF-015 / RM-00).
//!
//! Design constraints:
//! - **Bounded cardinality by construction.** Metric identities are a fixed
//!   enum; there are no route strings, entity ids, paths, or user identifiers
//!   in labels, so a metric cannot grow with traffic or catalog size.
//! - **Fixed buckets.** Values are recorded into a small fixed histogram, so
//!   memory is constant regardless of observation count.
//! - **Cheap.** Recording takes one mutex for a few background passes/second;
//!   this is diagnostic instrumentation, not a hot-path counter.
//!
//! This complements the existing structured spans, job history, resource
//! gauges, and subprocess timing. It does not replace them.

use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

/// Fixed metric identities. Order is part of the observable snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    SearchOutboxPass,
    SearchRebuildPage,
    SearchQuery,
    DiscoveryPass,
    CatalogImportPass,
    PoolAcquire,
    CatalogImportAcquire,
    CatalogImportHold,
    CatalogClaimAcquire,
    CatalogClaimHold,
    InspectionClaimAcquire,
    InspectionClaimHold,
    HeartbeatAcquire,
    HeartbeatHold,
    SearchOutboxAcquire,
    SearchOutboxHold,
    SearchSnapshotAcquire,
    SearchSnapshotHold,
    AdminStatsHeadlineAcquire,
    AdminStatsHeadlineHold,
    ScrobbleWaitProbe,
    ScrobbleWaitAdmission,
    RateLimitAcquire,
    RateLimitHold,
    NormalizationClaimAcquire,
    NormalizationClaimHold,
    ValidationClaimAcquire,
    ValidationClaimHold,
    PublicationClaimAcquire,
    PublicationClaimHold,
    PolicyClaimAcquire,
    PolicyClaimHold,
    SearchRebuildAcquire,
    SearchRebuildHold,
    EnrichmentClaimAcquire,
    EnrichmentClaimHold,
    PodcastDownloadAcquire,
    PodcastDownloadHold,
    RetainedClaimAcquire,
    RetainedClaimHold,
}

impl Metric {
    pub const ALL: [Metric; 40] = [
        Metric::SearchOutboxPass,
        Metric::SearchRebuildPage,
        Metric::SearchQuery,
        Metric::DiscoveryPass,
        Metric::CatalogImportPass,
        Metric::PoolAcquire,
        Metric::CatalogImportAcquire,
        Metric::CatalogImportHold,
        Metric::CatalogClaimAcquire,
        Metric::CatalogClaimHold,
        Metric::InspectionClaimAcquire,
        Metric::InspectionClaimHold,
        Metric::HeartbeatAcquire,
        Metric::HeartbeatHold,
        Metric::SearchOutboxAcquire,
        Metric::SearchOutboxHold,
        Metric::SearchSnapshotAcquire,
        Metric::SearchSnapshotHold,
        Metric::AdminStatsHeadlineAcquire,
        Metric::AdminStatsHeadlineHold,
        Metric::ScrobbleWaitProbe,
        Metric::ScrobbleWaitAdmission,
        Metric::RateLimitAcquire,
        Metric::RateLimitHold,
        Metric::NormalizationClaimAcquire,
        Metric::NormalizationClaimHold,
        Metric::ValidationClaimAcquire,
        Metric::ValidationClaimHold,
        Metric::PublicationClaimAcquire,
        Metric::PublicationClaimHold,
        Metric::PolicyClaimAcquire,
        Metric::PolicyClaimHold,
        Metric::SearchRebuildAcquire,
        Metric::SearchRebuildHold,
        Metric::EnrichmentClaimAcquire,
        Metric::EnrichmentClaimHold,
        Metric::PodcastDownloadAcquire,
        Metric::PodcastDownloadHold,
        Metric::RetainedClaimAcquire,
        Metric::RetainedClaimHold,
    ];

    const fn index(self) -> usize {
        match self {
            Metric::SearchOutboxPass => 0,
            Metric::SearchRebuildPage => 1,
            Metric::SearchQuery => 2,
            Metric::DiscoveryPass => 3,
            Metric::CatalogImportPass => 4,
            Metric::PoolAcquire => 5,
            Metric::CatalogImportAcquire => 6,
            Metric::CatalogImportHold => 7,
            Metric::CatalogClaimAcquire => 8,
            Metric::CatalogClaimHold => 9,
            Metric::InspectionClaimAcquire => 10,
            Metric::InspectionClaimHold => 11,
            Metric::HeartbeatAcquire => 12,
            Metric::HeartbeatHold => 13,
            Metric::SearchOutboxAcquire => 14,
            Metric::SearchOutboxHold => 15,
            Metric::SearchSnapshotAcquire => 16,
            Metric::SearchSnapshotHold => 17,
            Metric::AdminStatsHeadlineAcquire => 18,
            Metric::AdminStatsHeadlineHold => 19,
            Metric::ScrobbleWaitProbe => 20,
            Metric::ScrobbleWaitAdmission => 21,
            Metric::RateLimitAcquire => 22,
            Metric::RateLimitHold => 23,
            Metric::NormalizationClaimAcquire => 24,
            Metric::NormalizationClaimHold => 25,
            Metric::ValidationClaimAcquire => 26,
            Metric::ValidationClaimHold => 27,
            Metric::PublicationClaimAcquire => 28,
            Metric::PublicationClaimHold => 29,
            Metric::PolicyClaimAcquire => 30,
            Metric::PolicyClaimHold => 31,
            Metric::SearchRebuildAcquire => 32,
            Metric::SearchRebuildHold => 33,
            Metric::EnrichmentClaimAcquire => 34,
            Metric::EnrichmentClaimHold => 35,
            Metric::PodcastDownloadAcquire => 36,
            Metric::PodcastDownloadHold => 37,
            Metric::RetainedClaimAcquire => 38,
            Metric::RetainedClaimHold => 39,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Metric::SearchOutboxPass => "search_outbox_pass",
            Metric::SearchRebuildPage => "search_rebuild_page",
            Metric::SearchQuery => "search_query",
            Metric::DiscoveryPass => "discovery_pass",
            Metric::CatalogImportPass => "catalog_import_pass",
            Metric::PoolAcquire => "pool_acquire",
            Metric::CatalogImportAcquire => "catalog_import_acquire",
            Metric::CatalogImportHold => "catalog_import_hold",
            Metric::CatalogClaimAcquire => "catalog_claim_acquire",
            Metric::CatalogClaimHold => "catalog_claim_hold",
            Metric::InspectionClaimAcquire => "inspection_claim_acquire",
            Metric::InspectionClaimHold => "inspection_claim_hold",
            Metric::HeartbeatAcquire => "heartbeat_acquire",
            Metric::HeartbeatHold => "heartbeat_hold",
            Metric::SearchOutboxAcquire => "search_outbox_acquire",
            Metric::SearchOutboxHold => "search_outbox_hold",
            Metric::SearchSnapshotAcquire => "search_snapshot_acquire",
            Metric::SearchSnapshotHold => "search_snapshot_hold",
            Metric::AdminStatsHeadlineAcquire => "admin_stats_headline_acquire",
            Metric::AdminStatsHeadlineHold => "admin_stats_headline_hold",
            Metric::ScrobbleWaitProbe => "scrobble_wait_probe",
            Metric::ScrobbleWaitAdmission => "scrobble_wait_admission",
            Metric::RateLimitAcquire => "rate_limit_acquire",
            Metric::RateLimitHold => "rate_limit_hold",
            Metric::NormalizationClaimAcquire => "normalization_claim_acquire",
            Metric::NormalizationClaimHold => "normalization_claim_hold",
            Metric::ValidationClaimAcquire => "validation_claim_acquire",
            Metric::ValidationClaimHold => "validation_claim_hold",
            Metric::PublicationClaimAcquire => "publication_claim_acquire",
            Metric::PublicationClaimHold => "publication_claim_hold",
            Metric::PolicyClaimAcquire => "policy_claim_acquire",
            Metric::PolicyClaimHold => "policy_claim_hold",
            Metric::SearchRebuildAcquire => "search_rebuild_acquire",
            Metric::SearchRebuildHold => "search_rebuild_hold",
            Metric::EnrichmentClaimAcquire => "enrichment_claim_acquire",
            Metric::EnrichmentClaimHold => "enrichment_claim_hold",
            Metric::PodcastDownloadAcquire => "podcast_download_acquire",
            Metric::PodcastDownloadHold => "podcast_download_hold",
            Metric::RetainedClaimAcquire => "retained_claim_acquire",
            Metric::RetainedClaimHold => "retained_claim_hold",
        }
    }
}

/// Upper bounds in milliseconds for the first buckets; the last bucket is the
/// unbounded overflow bucket.
pub const BUCKET_BOUNDS_MS: [u64; 8] = [1, 5, 25, 100, 500, 2_500, 10_000, 60_000];
const BUCKETS: usize = BUCKET_BOUNDS_MS.len() + 1;
const METRICS: usize = Metric::ALL.len();

/// Records checkout hold time on all exits, including errors and cancellation.
pub struct HoldTimer {
    metric: Metric,
    started: std::time::Instant,
}

impl HoldTimer {
    #[must_use]
    pub fn new(metric: Metric) -> Self {
        Self {
            metric,
            started: std::time::Instant::now(),
        }
    }
}

impl Drop for HoldTimer {
    fn drop(&mut self) {
        observe(self.metric, self.started.elapsed());
    }
}

/// Check out one pooled connection for a claim and record acquisition wait for
/// a fixed workload class. The returned `HoldTimer` records the claim's hold
/// time when the claim transaction is dropped, including error and
/// cancellation paths. `Metric::PoolAcquire` still records the global checkout
/// so the unclassified/HTTP remainder stays observable.
pub async fn acquire_for_claim(
    pool: &sqlx::PgPool,
    acquire: Metric,
    hold: Metric,
) -> Result<(sqlx::pool::PoolConnection<sqlx::Postgres>, HoldTimer), sqlx::Error> {
    let started = Instant::now();
    let connection = pool.acquire().await?;
    let duration = started.elapsed();
    observe(Metric::PoolAcquire, duration);
    observe(acquire, duration);
    Ok((connection, HoldTimer::new(hold)))
}

#[derive(Debug, Clone)]
struct MetricsState {
    buckets: [[u64; BUCKETS]; METRICS],
    count: [u64; METRICS],
    sum_micros: [u64; METRICS],
    max_micros: [u64; METRICS],
}

impl Default for MetricsState {
    fn default() -> Self {
        Self {
            buckets: [[0; BUCKETS]; METRICS],
            count: [0; METRICS],
            sum_micros: [0; METRICS],
            max_micros: [0; METRICS],
        }
    }
}

static STATE: Mutex<MetricsState> = Mutex::new(MetricsState {
    buckets: [[0; BUCKETS]; METRICS],
    count: [0; METRICS],
    sum_micros: [0; METRICS],
    max_micros: [0; METRICS],
});

/// One metric's bounded snapshot. Each bucket counts only observations above
/// the previous bound and at or below its own; the final bucket is overflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricSnapshot {
    pub name: &'static str,
    pub count: u64,
    pub sum_micros: u64,
    pub max_micros: u64,
    pub buckets: [u64; BUCKETS],
}

#[must_use]
pub fn bucket_upper_ms(index: usize) -> Option<u64> {
    BUCKET_BOUNDS_MS.get(index).copied()
}

/// Records one observation. Saturating arithmetic keeps a long-lived process
/// from wrapping counters.
pub fn observe(metric: Metric, duration: Duration) {
    let micros = u64::try_from(duration.as_micros()).unwrap_or(u64::MAX);
    let bucket = BUCKET_BOUNDS_MS
        .iter()
        .position(|bound| duration <= Duration::from_millis(*bound))
        .unwrap_or(BUCKET_BOUNDS_MS.len());
    let index = metric.index();
    let mut state = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.buckets[index][bucket] = state.buckets[index][bucket].saturating_add(1);
    state.count[index] = state.count[index].saturating_add(1);
    state.sum_micros[index] = state.sum_micros[index].saturating_add(micros);
    state.max_micros[index] = state.max_micros[index].max(micros);
    drop(state);
    emit_if_due();
}

/// Exports cumulative fixed-bucket snapshots through opt-in structured TRACE
/// logging. Output is bounded to one snapshot per process per interval; no
/// request, user, path, or work-item values become metric labels.
fn emit_if_due() {
    if !tracing::enabled!(target: "melodee::performance", tracing::Level::TRACE) {
        return;
    }
    static START: OnceLock<Instant> = OnceLock::new();
    static NEXT_MS: AtomicU64 = AtomicU64::new(0);
    const EMIT_INTERVAL_MS: u64 = 10_000;
    let elapsed =
        u64::try_from(START.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX);
    if !reserve_emission(elapsed, &NEXT_MS, EMIT_INTERVAL_MS) {
        return;
    }
    emit_snapshot();
}

fn reserve_emission(elapsed: u64, next_emission: &AtomicU64, interval: u64) -> bool {
    let next = next_emission.load(Ordering::Relaxed);
    elapsed >= next
        && next_emission
            .compare_exchange(
                next,
                elapsed.saturating_add(interval),
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .is_ok()
}

fn emit_snapshot() {
    // Distinguish process restarts even if the operating system reuses a PID.
    static PROCESS_TOKEN: OnceLock<uuid::Uuid> = OnceLock::new();
    let token = PROCESS_TOKEN.get_or_init(uuid::Uuid::now_v7);
    for entry in snapshot() {
        tracing::trace!(target: "melodee::performance", process_id = std::process::id(), process_token = %token, metric = entry.name, count = entry.count,
            sum_micros = entry.sum_micros, max_micros = entry.max_micros,
            bucket_1ms = entry.buckets[0], bucket_5ms = entry.buckets[1],
            bucket_25ms = entry.buckets[2], bucket_100ms = entry.buckets[3],
            bucket_500ms = entry.buckets[4], bucket_2500ms = entry.buckets[5],
            bucket_10000ms = entry.buckets[6], bucket_60000ms = entry.buckets[7],
            bucket_overflow = entry.buckets[8], "bounded performance snapshot");
    }
}

/// Returns a bounded snapshot of every metric.
#[must_use]
pub fn snapshot() -> Vec<MetricSnapshot> {
    let state = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Metric::ALL
        .into_iter()
        .map(|metric| {
            let index = metric.index();
            MetricSnapshot {
                name: metric.as_str(),
                count: state.count[index],
                sum_micros: state.sum_micros[index],
                max_micros: state.max_micros[index],
                buckets: state.buckets[index],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        BUCKET_BOUNDS_MS, HoldTimer, Metric, bucket_upper_ms, emit_snapshot, observe,
        reserve_emission, snapshot,
    };

    #[test]
    fn buckets_are_fixed_and_overflow_is_bounded() {
        observe(Metric::SearchQuery, std::time::Duration::from_micros(500));
        observe(Metric::SearchQuery, std::time::Duration::from_micros(1900));
        observe(
            Metric::SearchQuery,
            std::time::Duration::from_nanos(1_000_001),
        );
        observe(
            Metric::SearchQuery,
            std::time::Duration::from_millis(10 * 60 * 1000),
        );
        let snapshot = snapshot();
        let query = snapshot
            .iter()
            .find(|entry| entry.name == "search_query")
            .expect("search_query metric");
        assert_eq!(query.count, 4);
        assert_eq!(query.buckets.len(), BUCKET_BOUNDS_MS.len() + 1);
        assert_eq!(query.buckets[0], 1, "0.5ms lands in the <=1ms bucket");
        assert_eq!(
            query.buckets[1], 2,
            "durations above 1ms land in the <=5ms bucket"
        );
        assert_eq!(
            *query.buckets.last().expect("overflow bucket"),
            1,
            "a very large duration lands in the single overflow bucket"
        );
        assert!(bucket_upper_ms(0) == Some(1));
        assert!(bucket_upper_ms(BUCKET_BOUNDS_MS.len()).is_none());
    }

    #[test]
    fn labels_are_a_fixed_bounded_set() {
        let names = snapshot()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert_eq!(names.len(), Metric::ALL.len());
        assert_eq!(
            names,
            vec![
                "search_outbox_pass",
                "search_rebuild_page",
                "search_query",
                "discovery_pass",
                "catalog_import_pass",
                "pool_acquire",
                "catalog_import_acquire",
                "catalog_import_hold",
                "catalog_claim_acquire",
                "catalog_claim_hold",
                "inspection_claim_acquire",
                "inspection_claim_hold",
                "heartbeat_acquire",
                "heartbeat_hold",
                "search_outbox_acquire",
                "search_outbox_hold",
                "search_snapshot_acquire",
                "search_snapshot_hold",
                "admin_stats_headline_acquire",
                "admin_stats_headline_hold",
                "scrobble_wait_probe",
                "scrobble_wait_admission",
                "rate_limit_acquire",
                "rate_limit_hold",
                "normalization_claim_acquire",
                "normalization_claim_hold",
                "validation_claim_acquire",
                "validation_claim_hold",
                "publication_claim_acquire",
                "publication_claim_hold",
                "policy_claim_acquire",
                "policy_claim_hold",
                "search_rebuild_acquire",
                "search_rebuild_hold",
                "enrichment_claim_acquire",
                "enrichment_claim_hold",
                "podcast_download_acquire",
                "podcast_download_hold",
                "retained_claim_acquire",
                "retained_claim_hold",
            ]
        );
    }

    #[test]
    fn claim_acquisition_records_global_and_class_wait() {
        // A missing pool is not required: use a real single-connection pool
        // against the configured test database is out of scope for this unit
        // test, so assert the fixed identities are distinct and class-scoped.
        let classes = [
            (
                Metric::NormalizationClaimAcquire,
                Metric::NormalizationClaimHold,
            ),
            (Metric::ValidationClaimAcquire, Metric::ValidationClaimHold),
            (
                Metric::PublicationClaimAcquire,
                Metric::PublicationClaimHold,
            ),
            (Metric::PolicyClaimAcquire, Metric::PolicyClaimHold),
            (Metric::SearchRebuildAcquire, Metric::SearchRebuildHold),
            (Metric::EnrichmentClaimAcquire, Metric::EnrichmentClaimHold),
            (Metric::PodcastDownloadAcquire, Metric::PodcastDownloadHold),
            (Metric::RetainedClaimAcquire, Metric::RetainedClaimHold),
        ];
        for (acquire, hold) in classes {
            observe(acquire, std::time::Duration::from_micros(10));
            observe(hold, std::time::Duration::from_micros(10));
        }
        let snapshot = snapshot();
        for (acquire, hold) in classes {
            assert!(snapshot.iter().any(|entry| entry.name == acquire.as_str()));
            assert!(snapshot.iter().any(|entry| entry.name == hold.as_str()));
            assert_ne!(acquire.as_str(), hold.as_str());
        }
    }

    #[test]
    fn hold_timer_records_early_exit() {
        fn early_exit() {
            let _timer = HoldTimer::new(Metric::CatalogImportHold);
        }
        let before = snapshot()
            .into_iter()
            .find(|entry| entry.name == "catalog_import_hold")
            .expect("hold metric")
            .count;
        early_exit();
        let after = snapshot()
            .into_iter()
            .find(|entry| entry.name == "catalog_import_hold")
            .expect("hold metric")
            .count;
        assert_eq!(after, before + 1);
    }

    #[test]
    fn snapshot_rate_limit_is_bounded_and_monotonic() {
        let next = std::sync::atomic::AtomicU64::new(0);
        assert!(reserve_emission(0, &next, 10_000));
        assert!(!reserve_emission(9_999, &next, 10_000));
        assert!(reserve_emission(10_000, &next, 10_000));
    }

    #[test]
    fn json_export_contains_only_fixed_metric_rows() {
        #[derive(Clone, Default)]
        struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Capture {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("capture lock").extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
            type Writer = Capture;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }
        let output = Capture::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_ansi(false)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(output.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, emit_snapshot);
        let bytes = output.0.lock().expect("capture lock");
        let text = std::str::from_utf8(&bytes).expect("utf8 events");
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("json event"))
            .collect();
        assert_eq!(events.len(), Metric::ALL.len());
        for (entry, metric) in events.iter().zip(Metric::ALL) {
            assert_eq!(entry["target"], "melodee::performance");
            assert_eq!(entry["fields"]["metric"], metric.as_str());
            assert!(entry["fields"]["bucket_overflow"].is_u64());
        }
    }
}
