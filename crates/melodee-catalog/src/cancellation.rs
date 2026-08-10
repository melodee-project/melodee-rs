//! Sticky cancellation shared by the scheduler and one bounded import pass.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Cooperative cancellation for work that has not begun its atomic import.
#[derive(Debug, Clone, Default)]
pub(crate) struct CatalogCancellation {
    cancelled: Arc<AtomicBool>,
    signal: Arc<tokio::sync::Notify>,
}

impl CatalogCancellation {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // There is one scheduler waiter. A stored permit also closes the race
        // between its atomic check and registering the async notification.
        self.signal.notify_one();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        if !self.is_cancelled() {
            self.signal.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_is_sticky_even_before_a_waiter_registers() {
        let cancellation = CatalogCancellation::default();
        cancellation.cancel();

        cancellation.cancelled().await;

        assert!(cancellation.is_cancelled());
    }
}
