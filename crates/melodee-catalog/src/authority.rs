//! Provider-neutral Artist evidence boundary.
//!
//! External authorities can propose evidence candidates, but they cannot
//! allocate or select Melodee Artist identities. Initial manifest import is
//! local-only; this boundary is used when catalog resolution is enriched.

use std::collections::BTreeSet;

use melodee_domain::ArtistId;

/// Raw observation and bounded catalog context offered to an authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityArtistObservation {
    pub raw_value: String,
    pub normalized_value: String,
    pub release_title: Option<String>,
    pub track_title: Option<String>,
}

/// Provider-neutral stable key for one external evidence candidate.
///
/// Neither field becomes a Melodee Artist ID.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AuthorityCandidateKey {
    pub authority: String,
    pub external_id: String,
}

/// Evidence candidate returned by one configured authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityArtistCandidate {
    pub candidate_key: AuthorityCandidateKey,
    pub display_name: String,
}

/// Durable local decisions that constrain an external consultation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthorityDecisionContext {
    /// A locked manual decision wins without contacting an authority.
    pub locked_manual_artist_id: Option<ArtistId>,
    /// Stable authority candidate keys rejected for this exact observation.
    pub negative_candidate_keys: BTreeSet<AuthorityCandidateKey>,
}

/// Why catalog resolution must retain a provisional local identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvisionalReason {
    AuthorityUnavailable,
    NoEligibleAuthorityCandidate,
}

/// Evidence-only result. Candidate results still require local scoring and a
/// Melodee-owned decision; they never authorize an automatic identity merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityConsultation {
    LockedManual { artist_id: ArtistId },
    EvidenceCandidates(Vec<AuthorityArtistCandidate>),
    RetainProvisional { reason: ProvisionalReason },
}

/// Pluggable source of external Artist evidence.
///
/// The associated error and opaque candidate key keep provider SDKs and
/// provider-specific identifiers outside the catalog application contract.
#[allow(async_fn_in_trait)]
pub trait ArtistAuthority: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    async fn find_artists(
        &self,
        observation: &AuthorityArtistObservation,
    ) -> Result<Vec<AuthorityArtistCandidate>, Self::Error>;
}

/// Consult external evidence without allowing it to outrank durable local
/// manual or negative decisions.
pub async fn consult_authority_for_catalog<A: ArtistAuthority>(
    authority: &A,
    observation: &AuthorityArtistObservation,
    decisions: &AuthorityDecisionContext,
) -> AuthorityConsultation {
    if let Some(artist_id) = decisions.locked_manual_artist_id {
        return AuthorityConsultation::LockedManual { artist_id };
    }

    let Ok(mut candidates) = authority.find_artists(observation).await else {
        return AuthorityConsultation::RetainProvisional {
            reason: ProvisionalReason::AuthorityUnavailable,
        };
    };
    candidates.retain(|candidate| {
        !candidate.candidate_key.authority.trim().is_empty()
            && !candidate.candidate_key.external_id.trim().is_empty()
            && !candidate.display_name.trim().is_empty()
            && !decisions
                .negative_candidate_keys
                .contains(&candidate.candidate_key)
    });
    candidates.sort_by(|left, right| {
        left.candidate_key
            .cmp(&right.candidate_key)
            .then_with(|| left.display_name.cmp(&right.display_name))
    });
    candidates.dedup_by(|left, right| left.candidate_key == right.candidate_key);

    if candidates.is_empty() {
        AuthorityConsultation::RetainProvisional {
            reason: ProvisionalReason::NoEligibleAuthorityCandidate,
        }
    } else {
        AuthorityConsultation::EvidenceCandidates(candidates)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use thiserror::Error;

    use super::*;

    #[derive(Debug, Clone)]
    enum FakeResponse {
        Candidates(Vec<AuthorityArtistCandidate>),
        Unavailable,
    }

    #[derive(Debug, Error)]
    #[error("deterministic fake authority unavailable")]
    struct FakeAuthorityError;

    #[derive(Debug)]
    struct FakeAuthority {
        response: FakeResponse,
        calls: AtomicUsize,
    }

    impl FakeAuthority {
        fn new(response: FakeResponse) -> Self {
            Self {
                response,
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl ArtistAuthority for FakeAuthority {
        type Error = FakeAuthorityError;

        async fn find_artists(
            &self,
            _observation: &AuthorityArtistObservation,
        ) -> Result<Vec<AuthorityArtistCandidate>, Self::Error> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            match &self.response {
                FakeResponse::Candidates(candidates) => Ok(candidates.clone()),
                FakeResponse::Unavailable => Err(FakeAuthorityError),
            }
        }
    }

    fn observation() -> AuthorityArtistObservation {
        AuthorityArtistObservation {
            raw_value: "Observed Artist".to_owned(),
            normalized_value: "OBSERVEDARTIST".to_owned(),
            release_title: Some("Private Release".to_owned()),
            track_title: Some("Track One".to_owned()),
        }
    }

    fn candidate(key: &str) -> AuthorityArtistCandidate {
        AuthorityArtistCandidate {
            candidate_key: AuthorityCandidateKey {
                authority: "fake-authority".to_owned(),
                external_id: key.to_owned(),
            },
            display_name: "Provider Candidate".to_owned(),
        }
    }

    #[tokio::test]
    async fn provider_failure_retains_provisional_catalog_identity() {
        let authority = FakeAuthority::new(FakeResponse::Unavailable);

        let result = consult_authority_for_catalog(
            &authority,
            &observation(),
            &AuthorityDecisionContext::default(),
        )
        .await;

        assert_eq!(
            result,
            AuthorityConsultation::RetainProvisional {
                reason: ProvisionalReason::AuthorityUnavailable
            }
        );
        assert_eq!(authority.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn locked_manual_decision_short_circuits_provider() {
        let authority = FakeAuthority::new(FakeResponse::Unavailable);
        let artist_id = ArtistId::new();
        let decisions = AuthorityDecisionContext {
            locked_manual_artist_id: Some(artist_id),
            ..AuthorityDecisionContext::default()
        };

        let result = consult_authority_for_catalog(&authority, &observation(), &decisions).await;

        assert_eq!(result, AuthorityConsultation::LockedManual { artist_id });
        assert_eq!(authority.calls.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn negative_assertion_filters_candidate_deterministically() {
        let authority = FakeAuthority::new(FakeResponse::Candidates(vec![candidate("rejected")]));
        let decisions = AuthorityDecisionContext {
            locked_manual_artist_id: None,
            negative_candidate_keys: BTreeSet::from([candidate("rejected").candidate_key]),
        };

        let result = consult_authority_for_catalog(&authority, &observation(), &decisions).await;

        assert_eq!(
            result,
            AuthorityConsultation::RetainProvisional {
                reason: ProvisionalReason::NoEligibleAuthorityCandidate
            }
        );
    }

    #[tokio::test]
    async fn candidates_are_sorted_and_deduplicated_without_selecting_identity() {
        let authority = FakeAuthority::new(FakeResponse::Candidates(vec![
            candidate("z"),
            candidate("a"),
            candidate("a"),
        ]));

        let result = consult_authority_for_catalog(
            &authority,
            &observation(),
            &AuthorityDecisionContext::default(),
        )
        .await;

        assert_eq!(
            result,
            AuthorityConsultation::EvidenceCandidates(vec![candidate("a"), candidate("z")])
        );
    }
}
