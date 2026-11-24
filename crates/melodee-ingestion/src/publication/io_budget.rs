use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use super::PublicationError;

pub(crate) const DEFAULT_NFS_MIB_PER_SECOND: u32 = 8;

/// One timeline shared by the publication workers of a server. Reads and writes
/// use the same budget. Idle time never accumulates credit for a later burst.
#[derive(Debug, Clone)]
pub(crate) struct NfsIoBudget {
    bytes_per_second: u64,
    next: Arc<Mutex<Instant>>,
    #[cfg(test)]
    pub(super) charged_bytes: Arc<std::sync::atomic::AtomicU64>,
}

impl NfsIoBudget {
    pub(crate) fn new(mib_per_second: u32) -> Self {
        Self {
            bytes_per_second: u64::from(mib_per_second) * 1024 * 1024,
            next: Arc::new(Mutex::new(Instant::now())),
            #[cfg(test)]
            charged_bytes: Arc::default(),
        }
    }

    fn reserve(&self, bytes: usize, now: Instant) -> Instant {
        #[cfg(test)]
        self.charged_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        let mut next = self
            .next
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let start = (*next).max(now);
        // Requests are bounded by the 64 KiB publication buffer.
        let nanos = ((bytes as u64) * 1_000_000_000).div_ceil(self.bytes_per_second);
        *next = start + Duration::from_nanos(nanos);
        start
    }
}

impl Default for NfsIoBudget {
    fn default() -> Self {
        Self::new(DEFAULT_NFS_MIB_PER_SECOND)
    }
}

#[derive(Clone, Copy)]
pub(super) struct AssetIo<'a> {
    pub cancellation: &'a AtomicBool,
    pub budget: Option<&'a NfsIoBudget>,
}

impl AssetIo<'_> {
    pub fn before_chunk(self, bytes: usize) -> Result<(), PublicationError> {
        if self.cancellation.load(Ordering::SeqCst) {
            return Err(PublicationError::Canceled);
        }
        if let Some(budget) = self.budget.filter(|_| bytes > 0) {
            let start = budget.reserve(bytes, Instant::now());
            while let Some(wait) = start.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait.min(Duration::from_millis(50)));
                if self.cancellation.load(Ordering::SeqCst) {
                    return Err(PublicationError::Canceled);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_active_workers_wait_for_the_same_bandwidth() {
        let budget = NfsIoBudget::new(1);
        let cancellation = AtomicBool::new(false);
        let asset_io = AssetIo {
            cancellation: &cancellation,
            budget: Some(&budget),
        };
        let start = Instant::now();
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(move || {
                    for _ in 0..4 {
                        assert!(asset_io.before_chunk(64 * 1024).is_ok());
                    }
                });
            }
        });
        // The first chunk can start immediately. Seven further 64 KiB chunks
        // take at least 7/16 second at 1 MiB/s regardless of worker count.
        assert!(start.elapsed() >= Duration::from_micros(437_500));
        cancellation.store(true, Ordering::SeqCst);
        assert!(matches!(
            asset_io.before_chunk(65536),
            Err(PublicationError::Canceled)
        ));
    }

    #[test]
    fn concurrent_publishers_share_one_rate_and_idle_time_does_not_bank_credit() {
        let budget = NfsIoBudget::new(1);
        let second_worker = budget.clone();
        let now = Instant::now();
        for i in 0..16 {
            let worker = if i % 2 == 0 { &budget } else { &second_worker };
            assert_eq!(
                worker.reserve(65536, now),
                now + Duration::from_micros(i * 62500)
            );
        }
        assert_eq!(budget.reserve(65536, now), now + Duration::from_secs(1));
        let later = now + Duration::from_secs(60);
        assert_eq!(second_worker.reserve(65536, later), later);
        assert_eq!(
            budget.reserve(1, later),
            later + Duration::from_micros(62500)
        );
    }
}
