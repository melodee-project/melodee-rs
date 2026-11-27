//! Opt-in, fixed-cardinality media-operation counters.
//!
//! Set the `melodee::media_operations` tracing target to TRACE to enable.
//! No paths, users, or other unbounded labels are recorded. Counts represent
//! completed instrumented operations, not all operating-system I/O. External
//! subprocess reads and kernel/page-cache traffic are not visible here.

use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

/// Fixed operation identities shared by the media and ingestion crates.
#[derive(Debug, Clone, Copy)]
#[repr(usize)]
pub enum Operation {
    AcquisitionTreeHash,
    AcquisitionCopy,
    MediaFileHash,
    MediaBufferHash,
    MediaBoundedRead,
    MediaCopy,
    ValidationHash,
    PublicationCopy,
    PublicationHash,
    Ffprobe,
    Ffmpeg,
    Fpcalc,
    ToolVersion,
    OtherTool,
}

impl Operation {
    const ALL: [Self; 14] = [
        Self::AcquisitionTreeHash,
        Self::AcquisitionCopy,
        Self::MediaFileHash,
        Self::MediaBufferHash,
        Self::MediaBoundedRead,
        Self::MediaCopy,
        Self::ValidationHash,
        Self::PublicationCopy,
        Self::PublicationHash,
        Self::Ffprobe,
        Self::Ffmpeg,
        Self::Fpcalc,
        Self::ToolVersion,
        Self::OtherTool,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::AcquisitionTreeHash => "acquisition_tree_hash",
            Self::AcquisitionCopy => "acquisition_copy",
            Self::MediaFileHash => "media_file_hash",
            Self::MediaBufferHash => "media_buffer_hash",
            Self::MediaBoundedRead => "media_bounded_read",
            Self::MediaCopy => "media_copy",
            Self::ValidationHash => "validation_hash",
            Self::PublicationCopy => "publication_copy",
            Self::PublicationHash => "publication_hash",
            Self::Ffprobe => "ffprobe",
            Self::Ffmpeg => "ffmpeg",
            Self::Fpcalc => "fpcalc",
            Self::ToolVersion => "tool_version",
            Self::OtherTool => "other_tool",
        }
    }
}

struct Counter {
    operations: AtomicU64,
    succeeded: AtomicU64,
    read_bytes: AtomicU64,
    written_bytes: AtomicU64,
    incomplete_byte_counts: AtomicU64,
}

impl Counter {
    const fn new() -> Self {
        Self {
            operations: AtomicU64::new(0),
            succeeded: AtomicU64::new(0),
            read_bytes: AtomicU64::new(0),
            written_bytes: AtomicU64::new(0),
            incomplete_byte_counts: AtomicU64::new(0),
        }
    }
}

static COUNTERS: [Counter; Operation::ALL.len()] = [const { Counter::new() }; Operation::ALL.len()];

fn saturating_add(counter: &AtomicU64, value: u64) {
    let mut old = counter.load(Ordering::Relaxed);
    loop {
        match counter.compare_exchange_weak(
            old,
            old.saturating_add(value),
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(actual) => old = actual,
        }
    }
}

/// Fixed-size cumulative snapshot. Concurrent increments may straddle fields.
#[derive(Debug, Clone, Copy)]
pub struct OperationSnapshot {
    pub operation: &'static str,
    pub operations: u64,
    pub succeeded: u64,
    pub read_bytes: u64,
    pub written_bytes: u64,
    pub incomplete_byte_counts: u64,
}

/// Read the fixed-cardinality cumulative counters; inactive tracing records no work.
#[must_use]
pub fn snapshot() -> Vec<OperationSnapshot> {
    Operation::ALL
        .iter()
        .map(|operation| {
            let counter = &COUNTERS[*operation as usize];
            OperationSnapshot {
                operation: operation.name(),
                operations: counter.operations.load(Ordering::Relaxed),
                succeeded: counter.succeeded.load(Ordering::Relaxed),
                read_bytes: counter.read_bytes.load(Ordering::Relaxed),
                written_bytes: counter.written_bytes.load(Ordering::Relaxed),
                incomplete_byte_counts: counter.incomplete_byte_counts.load(Ordering::Relaxed),
            }
        })
        .collect()
}

/// Records at most one event on drop, including on early return or cancellation.
pub struct OperationGuard {
    operation: Operation,
    started: Option<Instant>,
    read_bytes: u64,
    written_bytes: u64,
    success: bool,
    complete_bytes: bool,
}

impl OperationGuard {
    #[must_use]
    pub fn start(operation: Operation) -> Self {
        let enabled = tracing::enabled!(target: "melodee::media_operations", tracing::Level::TRACE);
        Self {
            operation,
            started: enabled.then(Instant::now),
            read_bytes: 0,
            written_bytes: 0,
            success: false,
            complete_bytes: true,
        }
    }

    pub fn add_read(&mut self, bytes: u64) {
        if self.started.is_some() {
            self.read_bytes = self.read_bytes.saturating_add(bytes);
        }
    }

    pub fn add_written(&mut self, bytes: u64) {
        if self.started.is_some() {
            self.written_bytes = self.written_bytes.saturating_add(bytes);
        }
    }

    /// Mark byte totals as lower bounds if an operation could fail after unreported I/O.
    pub fn mark_bytes_incomplete(&mut self) {
        self.complete_bytes = false;
    }

    /// Mark successful completion. Unfinished guards are recorded as failures.
    pub fn finish(mut self) {
        self.success = true;
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let Some(started) = self.started else { return };
        let counter = &COUNTERS[self.operation as usize];
        saturating_add(&counter.operations, 1);
        saturating_add(&counter.succeeded, u64::from(self.success));
        saturating_add(&counter.read_bytes, self.read_bytes);
        saturating_add(&counter.written_bytes, self.written_bytes);
        saturating_add(
            &counter.incomplete_byte_counts,
            u64::from(!self.complete_bytes),
        );
        static PROCESS_TOKEN: OnceLock<uuid::Uuid> = OnceLock::new();
        let token = PROCESS_TOKEN.get_or_init(uuid::Uuid::now_v7);
        // Suppress inherited worker spans: they may contain work-item or
        // source identifiers and are not metric labels.
        tracing::trace!(target: "melodee::media_operations", parent: None, operation = self.operation.name(),
            process_id = std::process::id(), process_token = %token, success = self.success,
            read_bytes = self.read_bytes, written_bytes = self.written_bytes,
            complete_bytes = self.complete_bytes,
            elapsed_micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            "media operation completed");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::{
        Event, Metadata, Subscriber,
        field::{Field, Visit},
        span::{Attributes, Id, Record},
    };

    #[derive(Debug, Default, PartialEq, Eq)]
    struct Captured {
        operation: String,
        success: bool,
        complete_bytes: bool,
        read: u64,
        written: u64,
    }
    impl Visit for Captured {
        fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "operation" {
                self.operation = value.to_owned();
            }
        }
        fn record_bool(&mut self, field: &Field, value: bool) {
            match field.name() {
                "success" => self.success = value,
                "complete_bytes" => self.complete_bytes = value,
                _ => {}
            }
        }
        fn record_u64(&mut self, field: &Field, value: u64) {
            match field.name() {
                "read_bytes" => self.read = value,
                "written_bytes" => self.written = value,
                _ => {}
            }
        }
    }
    struct Collector(Arc<Mutex<Vec<Captured>>>);
    impl Subscriber for Collector {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.target() == "melodee::media_operations"
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut captured = Captured::default();
            event.record(&mut captured);
            self.0.lock().unwrap().push(captured);
        }
    }

    #[test]
    fn emitted_events_count_success_failure_and_partial_bytes_without_paths() {
        let events = Arc::new(Mutex::new(Vec::new()));
        tracing::subscriber::with_default(Collector(events.clone()), || {
            let mut success = OperationGuard::start(Operation::ValidationHash);
            success.add_read(7);
            success.finish();
            let mut failed = OperationGuard::start(Operation::PublicationCopy);
            failed.add_read(5);
            failed.add_written(3);
            failed.mark_bytes_incomplete();
        });
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                Captured {
                    operation: "validation_hash".into(),
                    success: true,
                    complete_bytes: true,
                    read: 7,
                    written: 0
                },
                Captured {
                    operation: "publication_copy".into(),
                    success: false,
                    complete_bytes: false,
                    read: 5,
                    written: 3
                },
            ]
        );
        assert_eq!(snapshot().len(), Operation::ALL.len());
    }

    #[test]
    fn counters_saturate_instead_of_wrapping() {
        let counter = AtomicU64::new(u64::MAX - 1);
        saturating_add(&counter, 10);
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }
}
