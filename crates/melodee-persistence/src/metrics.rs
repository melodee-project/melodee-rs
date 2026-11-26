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

use std::sync::Mutex;
use std::time::Duration;

/// Fixed metric identities. Order is part of the observable snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    SearchOutboxPass,
    SearchRebuildPage,
    SearchQuery,
    DiscoveryPass,
    CatalogImportPass,
    PoolAcquire,
}

impl Metric {
    pub const ALL: [Metric; 6] = [
        Metric::SearchOutboxPass,
        Metric::SearchRebuildPage,
        Metric::SearchQuery,
        Metric::DiscoveryPass,
        Metric::CatalogImportPass,
        Metric::PoolAcquire,
    ];

    const fn index(self) -> usize {
        match self {
            Metric::SearchOutboxPass => 0,
            Metric::SearchRebuildPage => 1,
            Metric::SearchQuery => 2,
            Metric::DiscoveryPass => 3,
            Metric::CatalogImportPass => 4,
            Metric::PoolAcquire => 5,
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
        }
    }
}

/// Upper bounds in milliseconds for the first buckets; the last bucket is the
/// unbounded overflow bucket.
pub const BUCKET_BOUNDS_MS: [u64; 8] = [1, 5, 25, 100, 500, 2_500, 10_000, 60_000];
const BUCKETS: usize = BUCKET_BOUNDS_MS.len() + 1;
const METRICS: usize = 6;

#[derive(Debug, Clone, Default)]
struct MetricsState {
    buckets: [[u64; BUCKETS]; METRICS],
    count: [u64; METRICS],
    sum_micros: [u64; METRICS],
    max_micros: [u64; METRICS],
}

static STATE: Mutex<MetricsState> = Mutex::new(MetricsState {
    buckets: [[0; BUCKETS]; METRICS],
    count: [0; METRICS],
    sum_micros: [0; METRICS],
    max_micros: [0; METRICS],
});

/// One metric's bounded snapshot. `buckets[i]` counts observations whose
/// duration is at or below `bucket_upper_ms(i)`; the final bucket is overflow.
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
    let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    let bucket = BUCKET_BOUNDS_MS
        .iter()
        .position(|bound| millis <= *bound)
        .unwrap_or(BUCKET_BOUNDS_MS.len());
    let index = metric.index();
    let mut state = STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.buckets[index][bucket] = state.buckets[index][bucket].saturating_add(1);
    state.count[index] = state.count[index].saturating_add(1);
    state.sum_micros[index] = state.sum_micros[index].saturating_add(micros);
    state.max_micros[index] = state.max_micros[index].max(micros);
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

    use super::{BUCKET_BOUNDS_MS, Metric, bucket_upper_ms, observe, snapshot};

    #[test]
    fn buckets_are_fixed_and_overflow_is_bounded() {
        observe(Metric::SearchQuery, std::time::Duration::from_micros(500));
        observe(
            Metric::SearchQuery,
            std::time::Duration::from_millis(10 * 60 * 1000),
        );
        let snapshot = snapshot();
        let query = snapshot
            .iter()
            .find(|entry| entry.name == "search_query")
            .expect("search_query metric");
        assert_eq!(query.count, 2);
        assert_eq!(query.buckets.len(), BUCKET_BOUNDS_MS.len() + 1);
        assert_eq!(query.buckets[0], 1, "0.5ms lands in the <=1ms bucket");
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
            ]
        );
    }
}
