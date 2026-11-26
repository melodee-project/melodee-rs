//! Process-wide admission for expensive ingestion blocking operations.
//!
//! Acquire before scheduling a blocking closure, then move the permit into
//! the closure. Dropping an async waiter does not stop a running blocking task,
//! so retaining the permit in that task is essential for a real bound.

use std::sync::{Arc, OnceLock};
use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

// A conservative interim bound, independent of per-stage worker counts.
// It must be revisited with the representative codec/concurrency benchmark.
const HEAVY_INGESTION_LIMIT: usize = 4;
static HEAVY_INGESTION: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[cfg(feature = "test-support")]
pub async fn saturate_for_test() -> Result<OwnedSemaphorePermit, AcquireError> {
    semaphore()
        .acquire_many_owned(HEAVY_INGESTION_LIMIT as u32)
        .await
}

fn semaphore() -> Arc<Semaphore> {
    HEAVY_INGESTION
        .get_or_init(|| Arc::new(Semaphore::new(HEAVY_INGESTION_LIMIT)))
        .clone()
}

pub(crate) async fn heavy_ingestion() -> Result<OwnedSemaphorePermit, AcquireError> {
    semaphore().acquire_owned().await
}

/// Run filesystem work without occupying an async executor thread. The
/// closure owns the admission permit even when its awaiting future is dropped.
pub(crate) async fn run<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let permit = heavy_ingestion().await.map_err(|error| error.to_string())?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::{Arc, mpsc};
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn blocking_task_keeps_permit_after_waiter_is_cancelled() {
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            started_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
        });
        started_rx.recv().unwrap();
        task.abort();
        assert!(semaphore.clone().try_acquire_owned().is_err());
        finish_tx.send(()).unwrap();
        let permit = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            semaphore.clone().acquire_owned(),
        )
        .await
        .unwrap()
        .unwrap();
        drop(permit);
        assert_eq!(semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn cancelled_queued_waiter_does_not_consume_capacity() {
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let waiter = tokio::spawn(semaphore.clone().acquire_owned());
        tokio::task::yield_now().await;
        waiter.abort();
        let _ = waiter.await;
        drop(permit);
        assert_eq!(semaphore.available_permits(), 1);
    }
}
