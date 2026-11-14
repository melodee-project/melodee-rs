//! Correlation propagation shared by HTTP adapters and background use cases.
use std::future::Future;
use uuid::Uuid;

tokio::task_local! { static CURRENT_CORRELATION_ID: Uuid; }

/// Executes a future within the initiating operation's correlation scope.
pub async fn scope<F: Future>(id: Uuid, future: F) -> F::Output {
    CURRENT_CORRELATION_ID.scope(id, future).await
}

#[must_use]
pub fn current() -> Option<Uuid> {
    CURRENT_CORRELATION_ID.try_with(|id| *id).ok()
}

#[must_use]
pub fn envelope_id() -> Uuid {
    current().unwrap_or_else(Uuid::now_v7)
}
