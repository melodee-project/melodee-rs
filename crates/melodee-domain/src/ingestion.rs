use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Durable per-Release ingestion state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionState {
    Discovered,
    Settling,
    Ready,
    Inspecting,
    Normalizing,
    Validating,
    NeedsReview,
    Staged,
    Publishing,
    Published,
    Cataloging,
    Cataloged,
    Indexing,
    Indexed,
    Failed,
    Quarantined,
    Ignored,
    Deleted,
}

impl IngestionState {
    /// Returns true for states that do not advance without an explicit operator/reprocess action.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Indexed | Self::Quarantined | Self::Ignored | Self::Deleted
        )
    }

    /// Validates a normal forward state transition.
    pub fn ensure_transition(self, next: Self) -> Result<(), InvalidStateTransition> {
        let allowed = matches!(
            (self, next),
            (Self::Discovered, Self::Settling)
                | (Self::Settling, Self::Ready)
                | (Self::Ready, Self::Inspecting)
                | (Self::Inspecting, Self::Normalizing)
                | (Self::Normalizing, Self::Validating)
                | (Self::Validating, Self::NeedsReview)
                | (Self::Validating, Self::Staged)
                | (Self::NeedsReview, Self::Staged)
                | (Self::Staged, Self::Publishing)
                | (Self::Publishing, Self::Published)
                | (Self::Published, Self::Cataloging)
                | (Self::Cataloging, Self::Cataloged)
                | (Self::Cataloged, Self::Indexing)
                | (Self::Indexing, Self::Indexed)
        );

        if allowed {
            Ok(())
        } else {
            Err(InvalidStateTransition { current: self, next })
        }
    }
}

/// Policy outcome for a Release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    Accept,
    Review,
    Ignore,
    Quarantine,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid ingestion transition from {current:?} to {next:?}")]
pub struct InvalidStateTransition {
    pub current: IngestionState,
    pub next: IngestionState,
}

#[cfg(test)]
mod tests {
    use super::IngestionState;

    #[test]
    fn normal_happy_path_is_valid() {
        let states = [
            IngestionState::Discovered,
            IngestionState::Settling,
            IngestionState::Ready,
            IngestionState::Inspecting,
            IngestionState::Normalizing,
            IngestionState::Validating,
            IngestionState::Staged,
            IngestionState::Publishing,
            IngestionState::Published,
            IngestionState::Cataloging,
            IngestionState::Cataloged,
            IngestionState::Indexing,
            IngestionState::Indexed,
        ];

        for pair in states.windows(2) {
            assert!(pair[0].ensure_transition(pair[1]).is_ok());
        }
    }

    #[test]
    fn skipping_work_is_rejected() {
        let result = IngestionState::Discovered.ensure_transition(IngestionState::Indexed);
        assert!(result.is_err());
    }
}
