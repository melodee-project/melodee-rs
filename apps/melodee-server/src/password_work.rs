//! Bounded CPU work for password operations (ADR-0191).
//!
//! The synchronous compatibility primitives remain usable by offline tools. HTTP
//! handlers use this module so bcrypt cannot block Tokio's request workers.

use std::sync::{Arc, LazyLock};

use melodee_auth::{PasswordHashAlgorithm, PasswordVerificationError};
use tokio::sync::Semaphore;

const ACTIVE_OPERATIONS: usize = 4;
const ADMITTED_OPERATIONS: usize = 64;

static EXECUTOR: LazyLock<PasswordExecutor> =
    LazyLock::new(|| PasswordExecutor::new(ACTIVE_OPERATIONS, ADMITTED_OPERATIONS));

#[derive(Debug, thiserror::Error)]
pub enum PasswordWorkError {
    #[error("password service is busy")]
    Busy,
    #[error("password worker failed")]
    Worker(#[source] tokio::task::JoinError),
    #[error(transparent)]
    Verification(#[from] PasswordVerificationError),
}

struct PasswordExecutor {
    active: Arc<Semaphore>,
    admitted: Arc<Semaphore>,
}

impl PasswordExecutor {
    fn new(active: usize, admitted: usize) -> Self {
        Self {
            active: Arc::new(Semaphore::new(active)),
            admitted: Arc::new(Semaphore::new(admitted)),
        }
    }

    async fn run<T, F>(&self, operation: F) -> Result<T, PasswordWorkError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, PasswordVerificationError> + Send + 'static,
    {
        let admitted = self
            .admitted
            .clone()
            .try_acquire_owned()
            .map_err(|_| PasswordWorkError::Busy)?;
        let active = self
            .active
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PasswordWorkError::Busy)?;
        let span = tracing::Span::current();
        tokio::task::spawn_blocking(move || {
            // A cancelled HTTP request cannot stop already-running bcrypt. Keep
            // both permits until the CPU work ends, including on unwinding.
            let (_active, _admitted) = (active, admitted);
            let _entered = span.enter();
            operation()
        })
        .await
        .map_err(PasswordWorkError::Worker)?
        .map_err(PasswordWorkError::Verification)
    }
}

pub async fn verify_password(
    password: &[u8],
    algorithm: PasswordHashAlgorithm,
    hash: &str,
) -> Result<bool, PasswordWorkError> {
    let password = password.to_vec();
    let hash = hash.to_owned();
    EXECUTOR
        .run(move || melodee_auth::verify_password(&password, algorithm, &hash))
        .await
}

pub async fn hash_password(password: &[u8]) -> Result<String, PasswordWorkError> {
    let password = password.to_vec();
    EXECUTOR
        .run(move || melodee_auth::hash_password(&password))
        .await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_work_keeps_executor_responsive_and_cancellation_keeps_the_permit() {
        let executor = Arc::new(PasswordExecutor::new(1, 1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let running = executor.clone();
        let task = tokio::spawn(async move {
            running
                .run(move || {
                    let _ = started_tx.send(());
                    finish_rx
                        .recv_timeout(Duration::from_secs(5))
                        .expect("release worker");
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .expect("executor responsive")
            .expect("started");
        assert!(matches!(
            executor.run(|| Ok(())).await,
            Err(PasswordWorkError::Busy)
        ));
        task.abort();
        let _ = task.await;
        assert!(matches!(
            executor.run(|| Ok(())).await,
            Err(PasswordWorkError::Busy)
        ));
        finish_tx.send(()).expect("release");
        tokio::time::timeout(Duration::from_secs(2), async {
            while executor.admitted.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("worker releases permit");
        assert!(executor.run(|| Ok(())).await.is_ok());
    }

    #[tokio::test]
    async fn real_password_verification_preserves_compatibility() {
        let hash = bcrypt::hash("correct", 4).expect("fixture hash");
        assert!(
            verify_password(b"correct", PasswordHashAlgorithm::Bcrypt, &hash)
                .await
                .expect("verify")
        );
        assert!(
            !verify_password(b"incorrect", PasswordHashAlgorithm::Bcrypt, &hash)
                .await
                .expect("verify")
        );
        assert!(
            verify_password(b"correct", PasswordHashAlgorithm::Bcrypt, "invalid")
                .await
                .is_err()
        );
    }
}
