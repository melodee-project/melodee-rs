//! bounded-resource instrumentation: a process-local registry of named
//! gauges, one per semaphore-bounded resource class (PRD-INGEST-009,
//! `IMPLMENTATION_OUTSTANDING.md` §7.5).
//!
//! Every request-path semaphore in the server (direct-play streams,
//! transcoding, image processing, podcast discovery, provider dispatch,
//! notification dispatch) reports through a [`ResourceGauge`] so operators can
//! see current/maximum utilization, queued waiters with their age, and
//! saturation counters through the existing admin health surface. Claim-based
//! background workers already expose durable queue metrics from PostgreSQL
//! (`WorkerPoolMetrics`) and are not duplicated here.
//!
//! Gauges are purely observational: they never change admission decisions.
//! Wiring is optional (`with_gauge` builder methods) so tests that construct
//! services directly keep working unchanged.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch, or 0 before the epoch (never in
/// practice; keeps the atomic encoding total).
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Serialization-friendly point-in-time snapshot of one resource class.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceBoundSnapshot {
    pub class: &'static str,
    pub max: usize,
    pub in_flight: usize,
    pub waiting: usize,
    /// Milliseconds the oldest current waiter has been queued; 0 when idle.
    pub oldest_wait_ms: u64,
    pub admitted_total: u64,
    /// Admissions rejected without queueing (non-blocking acquire paths).
    pub rejected_total: u64,
    /// `in_flight == max` or anyone is queued: the class is at capacity.
    pub saturated: bool,
}

/// Counters for one bounded resource class. Cheap to clone: all state lives
/// behind the `Arc`.
#[derive(Debug)]
pub struct ResourceGauge {
    class: &'static str,
    max: usize,
    in_flight: AtomicUsize,
    waiting: AtomicUsize,
    /// Epoch millis of the oldest waiter; 0 when nobody waits. Approximate:
    /// maintained on begin/end, not per-tick.
    oldest_wait_start: AtomicU64,
    admitted_total: AtomicU64,
    rejected_total: AtomicU64,
}

impl ResourceGauge {
    /// Marks the start of a (possibly zero-length) wait for admission.
    pub fn begin_wait(self: &Arc<Self>) {
        let previous = self.waiting.fetch_add(1, Ordering::Relaxed);
        if previous == 0 {
            self.oldest_wait_start
                .store(now_millis(), Ordering::Relaxed);
        }
    }

    /// Records a successful admission after a wait begun with
    /// [`begin_wait`](Self::begin_wait). Returns a guard whose drop releases
    /// the in-flight slot.
    pub fn admit(self: &Arc<Self>) -> GaugePermit {
        self.end_wait();
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.admitted_total.fetch_add(1, Ordering::Relaxed);
        GaugePermit {
            gauge: Arc::clone(self),
        }
    }

    /// Records an immediate admission on a non-blocking acquire path.
    pub fn admit_immediate(self: &Arc<Self>) -> GaugePermit {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.admitted_total.fetch_add(1, Ordering::Relaxed);
        GaugePermit {
            gauge: Arc::clone(self),
        }
    }

    /// Records a rejection on a non-blocking acquire path (no wait was begun).
    pub fn reject(&self) {
        self.rejected_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a failed/cancelled wait begun with
    /// [`begin_wait`](Self::begin_wait) (permit acquisition returned an error
    /// or timed out).
    pub fn wait_failed(self: &Arc<Self>) {
        self.end_wait();
        self.rejected_total.fetch_add(1, Ordering::Relaxed);
    }

    fn end_wait(&self) {
        if self.waiting.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.oldest_wait_start.store(0, Ordering::Relaxed);
        }
    }

    /// Point-in-time counters for diagnostics.
    pub fn snapshot(&self) -> ResourceBoundSnapshot {
        let in_flight = self.in_flight.load(Ordering::Relaxed);
        let waiting = self.waiting.load(Ordering::Relaxed);
        let oldest = self.oldest_wait_start.load(Ordering::Relaxed);
        ResourceBoundSnapshot {
            class: self.class,
            max: self.max,
            in_flight,
            waiting,
            oldest_wait_ms: if oldest == 0 {
                0
            } else {
                now_millis().saturating_sub(oldest)
            },
            admitted_total: self.admitted_total.load(Ordering::Relaxed),
            rejected_total: self.rejected_total.load(Ordering::Relaxed),
            saturated: in_flight >= self.max || waiting > 0,
        }
    }
}

/// RAII guard: releases the gauge's in-flight slot on drop (client
/// disconnect, handler completion, or cancellation all flow through here).
#[derive(Debug)]
pub struct GaugePermit {
    gauge: Arc<ResourceGauge>,
}

impl Drop for GaugePermit {
    fn drop(&mut self) {
        self.gauge.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Process-local registry of every instrumented resource class. Constructed
/// once in `main` and shared into the services that own semaphores; the
/// admin health surface snapshots it per request.
#[derive(Debug, Default)]
pub struct ResourceBounds {
    gauges: std::sync::Mutex<Vec<Arc<ResourceGauge>>>,
}

static GLOBAL: std::sync::OnceLock<Arc<ResourceBounds>> = std::sync::OnceLock::new();

impl ResourceBounds {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Installs the process-global registry. Must run once at startup before
    /// any instrumented service is built; a second install is ignored so test
    /// binaries (which construct services without installing) stay ungauged
    /// rather than panicking.
    pub fn install_global(this: Arc<Self>) {
        let _ = GLOBAL.set(this);
    }

    /// The installed process-global registry, when startup installed one.
    #[must_use]
    pub fn global() -> Option<Arc<ResourceBounds>> {
        GLOBAL.get().cloned()
    }

    /// Registers a class and returns its gauge. `max` is the class's fixed
    /// capacity; registering the same class twice would double-count in the
    /// health view, so each service registers exactly once at construction.
    #[must_use]
    pub fn register(&self, class: &'static str, max: usize) -> Arc<ResourceGauge> {
        let gauge = Arc::new(ResourceGauge {
            class,
            max,
            in_flight: AtomicUsize::new(0),
            waiting: AtomicUsize::new(0),
            oldest_wait_start: AtomicU64::new(0),
            admitted_total: AtomicU64::new(0),
            rejected_total: AtomicU64::new(0),
        });
        if let Ok(mut gauges) = self.gauges.lock() {
            gauges.push(Arc::clone(&gauge));
        }
        gauge
    }

    /// Snapshots every registered class for the health surface.
    #[must_use]
    pub fn snapshot(&self) -> Vec<ResourceBoundSnapshot> {
        self.gauges
            .lock()
            .map(|gauges| gauges.iter().map(|gauge| gauge.snapshot()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn gauge_tracks_admission_and_release() {
        let registry = ResourceBounds::new();
        let gauge = registry.register("test.class", 2);
        gauge.begin_wait();
        let permit = gauge.admit();
        let snapshot = gauge.snapshot();
        assert_eq!(snapshot.in_flight, 1);
        assert_eq!(snapshot.waiting, 0);
        assert_eq!(snapshot.admitted_total, 1);
        assert!(!snapshot.saturated);
        drop(permit);
        let snapshot = gauge.snapshot();
        assert_eq!(snapshot.in_flight, 0);
        assert!(!snapshot.saturated);
    }

    #[test]
    fn gauge_marks_saturation_at_capacity_and_while_waiting() {
        let registry = ResourceBounds::new();
        let gauge = registry.register("test.class", 1);
        let _permit = gauge.admit_immediate();
        assert!(gauge.snapshot().saturated);
        gauge.begin_wait();
        let snapshot = gauge.snapshot();
        assert_eq!(snapshot.waiting, 1);
        assert!(snapshot.saturated);
        gauge.wait_failed();
        let snapshot = gauge.snapshot();
        assert_eq!(snapshot.waiting, 0);
        assert_eq!(snapshot.rejected_total, 1);
    }

    #[test]
    fn registry_snapshots_every_registered_class() {
        let registry = ResourceBounds::new();
        let _a = registry.register("a", 1);
        let _b = registry.register("b", 4);
        let classes: Vec<&str> = registry.snapshot().iter().map(|s| s.class).collect();
        assert_eq!(classes, ["a", "b"]);
    }

    #[test]
    fn oldest_wait_age_is_zero_when_idle_and_positive_while_waiting() {
        let registry = ResourceBounds::new();
        let gauge = registry.register("test.class", 1);
        assert_eq!(gauge.snapshot().oldest_wait_ms, 0);
        gauge.begin_wait();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let waited = gauge.snapshot().oldest_wait_ms;
        assert!(waited >= 1, "expected a positive wait age, got {waited}");
        gauge.wait_failed();
        assert_eq!(gauge.snapshot().oldest_wait_ms, 0);
    }
}
