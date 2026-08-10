//! Deterministic, bounded evaluation of local Artist identity candidates.
//!
//! This service recommends a decision from already-collected evidence. It does
//! not query providers, persist a decision, or allocate an Artist identity.

use std::collections::{BTreeMap, BTreeSet};

use melodee_domain::{ArtistId, Confidence, EvidenceKind, EvidencePolarity};
use thiserror::Error;

/// Default maximum candidates considered for one observed Artist value.
pub const DEFAULT_MAX_ARTIST_CANDIDATES: usize = 64;
/// Default maximum evidence facts retained for one candidate.
pub const DEFAULT_MAX_EVIDENCE_PER_CANDIDATE: usize = 32;

const CONFIRMABLE_SCORE: i32 = 6_000;
const PROPOSED_SCORE: i32 = 3_000;
const MINIMUM_WINNING_MARGIN: i32 = 1_000;

/// Boundary at which an evidence fact entered local resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResolutionEvidenceSource {
    /// Evidence read from Melodee's authoritative catalog.
    PersistedCatalog,
    /// Evidence embedded in the immutable manifest-v3 handoff.
    Manifest,
    /// Evidence proposed by a provider-neutral external authority.
    ExternalAuthority,
}

/// One immutable input fact evaluated for a local Artist candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionEvidence {
    pub kind: EvidenceKind,
    pub polarity: EvidencePolarity,
    pub confidence: Confidence,
    pub source: ResolutionEvidenceSource,
}

/// A bounded persisted/local candidate and all evidence currently known for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalArtistCandidate {
    pub artist_id: ArtistId,
    pub evidence: Vec<ResolutionEvidence>,
}

/// Durable decisions that constrain automated evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalResolutionConstraints {
    /// A locked administrator choice is returned without rescoring candidates.
    pub locked_manual_artist_id: Option<ArtistId>,
    /// Candidates rejected for this observation are never recommended.
    pub negative_artist_ids: BTreeSet<ArtistId>,
}

/// Scored evidence retained for audit and persistence by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateAssessment {
    pub artist_id: ArtistId,
    pub supporting_score: i32,
    pub conflicting_score: i32,
    pub net_score: i32,
    pub suppressed_by_negative_assertion: bool,
    pub evidence: Vec<ResolutionEvidence>,
}

/// Why no automated positive recommendation is safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalProvisionalReason {
    NoEligibleCandidate,
    WeakOrAmbiguousEvidence,
}

/// Non-mutating recommendation produced by local evidence evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistResolutionRecommendation {
    /// An existing locked administrator decision remains authoritative.
    LockedManual { artist_id: ArtistId },
    /// Evidence is worth persisting as a proposal, but is not safe to confirm.
    Proposed { artist_id: ArtistId },
    /// Independent local evidence is sufficient for automated confirmation.
    Confirmable { artist_id: ArtistId },
    /// Keep or create a provisional identity instead of selecting a candidate.
    RetainProvisional { reason: LocalProvisionalReason },
}

/// Complete deterministic output: evidence assessments plus a recommendation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistResolutionEvaluation {
    pub assessments: Vec<CandidateAssessment>,
    pub recommendation: ArtistResolutionRecommendation,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ArtistResolutionError {
    #[error("candidate limit {limit} exceeded by {actual} Artist candidates")]
    CandidateLimitExceeded { limit: usize, actual: usize },
    #[error("evidence limit {limit} exceeded by {actual} facts for Artist {artist_id}")]
    EvidenceLimitExceeded {
        artist_id: ArtistId,
        limit: usize,
        actual: usize,
    },
    #[error("Artist candidate {0} was supplied more than once")]
    DuplicateCandidate(ArtistId),
}

/// Pure local resolver with explicit resource bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalArtistResolver {
    max_candidates: usize,
    max_evidence_per_candidate: usize,
}

impl Default for LocalArtistResolver {
    fn default() -> Self {
        Self {
            max_candidates: DEFAULT_MAX_ARTIST_CANDIDATES,
            max_evidence_per_candidate: DEFAULT_MAX_EVIDENCE_PER_CANDIDATE,
        }
    }
}

impl LocalArtistResolver {
    /// Uses caller-selected positive bounds, clamped to at least one.
    #[must_use]
    pub fn with_limits(max_candidates: usize, max_evidence_per_candidate: usize) -> Self {
        Self {
            max_candidates: max_candidates.max(1),
            max_evidence_per_candidate: max_evidence_per_candidate.max(1),
        }
    }

    /// Evaluate candidates without mutating catalog state.
    pub fn evaluate(
        &self,
        candidates: &[LocalArtistCandidate],
        constraints: &LocalResolutionConstraints,
    ) -> Result<ArtistResolutionEvaluation, ArtistResolutionError> {
        if let Some(artist_id) = constraints.locked_manual_artist_id {
            return Ok(ArtistResolutionEvaluation {
                assessments: Vec::new(),
                recommendation: ArtistResolutionRecommendation::LockedManual { artist_id },
            });
        }
        if candidates.len() > self.max_candidates {
            return Err(ArtistResolutionError::CandidateLimitExceeded {
                limit: self.max_candidates,
                actual: candidates.len(),
            });
        }

        let mut assessments = BTreeMap::new();
        for candidate in candidates {
            if candidate.evidence.len() > self.max_evidence_per_candidate {
                return Err(ArtistResolutionError::EvidenceLimitExceeded {
                    artist_id: candidate.artist_id,
                    limit: self.max_evidence_per_candidate,
                    actual: candidate.evidence.len(),
                });
            }
            let assessment = assess_candidate(candidate, constraints);
            if assessments
                .insert(candidate.artist_id, assessment)
                .is_some()
            {
                return Err(ArtistResolutionError::DuplicateCandidate(
                    candidate.artist_id,
                ));
            }
        }

        let assessments: Vec<_> = assessments.into_values().collect();
        let recommendation = recommend(&assessments);
        Ok(ArtistResolutionEvaluation {
            assessments,
            recommendation,
        })
    }
}

fn assess_candidate(
    candidate: &LocalArtistCandidate,
    constraints: &LocalResolutionConstraints,
) -> CandidateAssessment {
    let mut evidence = candidate.evidence.clone();
    evidence.sort_unstable_by_key(|fact| evidence_sort_key(*fact));
    evidence.dedup();
    let (supporting_score, conflicting_score) =
        evidence
            .iter()
            .fold((0_i32, 0_i32), |(supporting, conflicting), fact| {
                let score = weighted_score(*fact);
                match fact.polarity {
                    EvidencePolarity::Supports => (supporting.saturating_add(score), conflicting),
                    EvidencePolarity::Conflicts => (supporting, conflicting.saturating_add(score)),
                }
            });

    CandidateAssessment {
        artist_id: candidate.artist_id,
        supporting_score,
        conflicting_score,
        net_score: supporting_score.saturating_sub(conflicting_score),
        suppressed_by_negative_assertion: constraints
            .negative_artist_ids
            .contains(&candidate.artist_id),
        evidence,
    }
}

fn evidence_sort_key(
    evidence: ResolutionEvidence,
) -> (EvidenceKind, u8, Confidence, ResolutionEvidenceSource) {
    let polarity = match evidence.polarity {
        EvidencePolarity::Supports => 0,
        EvidencePolarity::Conflicts => 1,
    };
    (
        evidence.kind,
        polarity,
        evidence.confidence,
        evidence.source,
    )
}

fn weighted_score(evidence: ResolutionEvidence) -> i32 {
    let weight = match evidence.kind {
        EvidenceKind::NameSimilarity => 1_000,
        EvidenceKind::Alias => 4_000,
        EvidenceKind::EmbeddedExternalId => 8_500,
        EvidenceKind::ReleaseContext | EvidenceKind::TrackParticipation => 3_000,
        EvidenceKind::GroupMembership => 2_500,
        EvidenceKind::ArtistType | EvidenceKind::ActiveDate | EvidenceKind::Area => 1_000,
        EvidenceKind::ExternalProviderAgreement => 2_000,
        // A durable manual choice enters through locked_manual_artist_id. A
        // free-standing "manual" fact cannot impersonate that authority.
        EvidenceKind::Manual => 0,
    };
    weight * i32::from(evidence.confidence.basis_points()) / i32::from(Confidence::MAX_BASIS_POINTS)
}

fn recommend(assessments: &[CandidateAssessment]) -> ArtistResolutionRecommendation {
    let mut eligible: Vec<_> = assessments
        .iter()
        .filter(|candidate| !candidate.suppressed_by_negative_assertion)
        .collect();
    eligible.sort_by(|left, right| {
        right
            .net_score
            .cmp(&left.net_score)
            .then_with(|| left.artist_id.cmp(&right.artist_id))
    });
    let Some(winner) = eligible.first().copied() else {
        return ArtistResolutionRecommendation::RetainProvisional {
            reason: LocalProvisionalReason::NoEligibleCandidate,
        };
    };
    let runner_up_score = eligible.get(1).map_or(0, |candidate| candidate.net_score);
    let decisive = winner.net_score.saturating_sub(runner_up_score) >= MINIMUM_WINNING_MARGIN;

    if decisive && winner.net_score >= CONFIRMABLE_SCORE && has_confirmable_local_basis(winner) {
        ArtistResolutionRecommendation::Confirmable {
            artist_id: winner.artist_id,
        }
    } else if decisive && winner.net_score >= PROPOSED_SCORE {
        ArtistResolutionRecommendation::Proposed {
            artist_id: winner.artist_id,
        }
    } else {
        ArtistResolutionRecommendation::RetainProvisional {
            reason: LocalProvisionalReason::WeakOrAmbiguousEvidence,
        }
    }
}

fn has_confirmable_local_basis(candidate: &CandidateAssessment) -> bool {
    if candidate.conflicting_score > 0 {
        return false;
    }
    let locally_supports = |kind| {
        candidate.evidence.iter().any(|fact| {
            fact.kind == kind
                && fact.polarity == EvidencePolarity::Supports
                && fact.source != ResolutionEvidenceSource::ExternalAuthority
        })
    };
    locally_supports(EvidenceKind::EmbeddedExternalId)
        || (locally_supports(EvidenceKind::Alias)
            && (locally_supports(EvidenceKind::ReleaseContext)
                || locally_supports(EvidenceKind::TrackParticipation)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    fn confidence(value: u16) -> Confidence {
        Confidence::new(value).expect("test confidence is valid")
    }

    fn evidence(
        kind: EvidenceKind,
        polarity: EvidencePolarity,
        source: ResolutionEvidenceSource,
    ) -> ResolutionEvidence {
        ResolutionEvidence {
            kind,
            polarity,
            confidence: confidence(10_000),
            source,
        }
    }

    fn candidate(artist_id: ArtistId, evidence: Vec<ResolutionEvidence>) -> LocalArtistCandidate {
        LocalArtistCandidate {
            artist_id,
            evidence,
        }
    }

    #[test]
    fn exact_same_name_candidates_remain_ambiguous() {
        let first = ArtistId::new();
        let second = ArtistId::new();
        let name = evidence(
            EvidenceKind::NameSimilarity,
            EvidencePolarity::Supports,
            ResolutionEvidenceSource::PersistedCatalog,
        );

        let result = LocalArtistResolver::default()
            .evaluate(
                &[candidate(first, vec![name]), candidate(second, vec![name])],
                &LocalResolutionConstraints::default(),
            )
            .expect("bounded evaluation");

        assert_eq!(
            result.recommendation,
            ArtistResolutionRecommendation::RetainProvisional {
                reason: LocalProvisionalReason::WeakOrAmbiguousEvidence
            }
        );
    }

    #[test]
    fn alias_plus_release_context_is_confirmable() {
        let artist_id = ArtistId::new();
        let result = LocalArtistResolver::default()
            .evaluate(
                &[candidate(
                    artist_id,
                    vec![
                        evidence(
                            EvidenceKind::Alias,
                            EvidencePolarity::Supports,
                            ResolutionEvidenceSource::PersistedCatalog,
                        ),
                        evidence(
                            EvidenceKind::ReleaseContext,
                            EvidencePolarity::Supports,
                            ResolutionEvidenceSource::Manifest,
                        ),
                    ],
                )],
                &LocalResolutionConstraints::default(),
            )
            .expect("bounded evaluation");

        assert_eq!(
            result.recommendation,
            ArtistResolutionRecommendation::Confirmable { artist_id }
        );
    }

    #[test]
    fn conflicting_evidence_lowers_confirmation_to_proposal() {
        let artist_id = ArtistId::new();
        let result = LocalArtistResolver::default()
            .evaluate(
                &[candidate(
                    artist_id,
                    vec![
                        evidence(
                            EvidenceKind::Alias,
                            EvidencePolarity::Supports,
                            ResolutionEvidenceSource::PersistedCatalog,
                        ),
                        evidence(
                            EvidenceKind::ReleaseContext,
                            EvidencePolarity::Supports,
                            ResolutionEvidenceSource::Manifest,
                        ),
                        evidence(
                            EvidenceKind::Area,
                            EvidencePolarity::Conflicts,
                            ResolutionEvidenceSource::PersistedCatalog,
                        ),
                    ],
                )],
                &LocalResolutionConstraints::default(),
            )
            .expect("bounded evaluation");

        assert_eq!(
            result.recommendation,
            ArtistResolutionRecommendation::Proposed { artist_id }
        );
    }

    #[test]
    fn embedded_external_id_is_sufficient_without_name_matching() {
        let artist_id = ArtistId::new();
        let result = LocalArtistResolver::default()
            .evaluate(
                &[candidate(
                    artist_id,
                    vec![evidence(
                        EvidenceKind::EmbeddedExternalId,
                        EvidencePolarity::Supports,
                        ResolutionEvidenceSource::Manifest,
                    )],
                )],
                &LocalResolutionConstraints::default(),
            )
            .expect("bounded evaluation");

        assert_eq!(
            result.recommendation,
            ArtistResolutionRecommendation::Confirmable { artist_id }
        );
    }

    #[test]
    fn weak_name_evidence_retains_provisional_identity() {
        let artist_id = ArtistId::new();
        let result = LocalArtistResolver::default()
            .evaluate(
                &[candidate(
                    artist_id,
                    vec![evidence(
                        EvidenceKind::NameSimilarity,
                        EvidencePolarity::Supports,
                        ResolutionEvidenceSource::PersistedCatalog,
                    )],
                )],
                &LocalResolutionConstraints::default(),
            )
            .expect("bounded evaluation");

        assert_eq!(
            result.recommendation,
            ArtistResolutionRecommendation::RetainProvisional {
                reason: LocalProvisionalReason::WeakOrAmbiguousEvidence
            }
        );
    }

    #[test]
    fn provider_evidence_cannot_confirm_an_identity() {
        let artist_id = ArtistId::new();
        let result = LocalArtistResolver::default()
            .evaluate(
                &[candidate(
                    artist_id,
                    vec![
                        evidence(
                            EvidenceKind::EmbeddedExternalId,
                            EvidencePolarity::Supports,
                            ResolutionEvidenceSource::ExternalAuthority,
                        ),
                        evidence(
                            EvidenceKind::ExternalProviderAgreement,
                            EvidencePolarity::Supports,
                            ResolutionEvidenceSource::ExternalAuthority,
                        ),
                    ],
                )],
                &LocalResolutionConstraints::default(),
            )
            .expect("bounded evaluation");

        assert_eq!(
            result.recommendation,
            ArtistResolutionRecommendation::Proposed { artist_id }
        );
    }

    #[test]
    fn locked_manual_and_negative_assertions_dominate_scores() {
        let locked = ArtistId::new();
        let rejected = ArtistId::new();
        let resolver = LocalArtistResolver::default();
        let strong = candidate(
            rejected,
            vec![evidence(
                EvidenceKind::EmbeddedExternalId,
                EvidencePolarity::Supports,
                ResolutionEvidenceSource::Manifest,
            )],
        );

        let locked_result = resolver
            .evaluate(
                std::slice::from_ref(&strong),
                &LocalResolutionConstraints {
                    locked_manual_artist_id: Some(locked),
                    negative_artist_ids: BTreeSet::new(),
                },
            )
            .expect("locked resolution");
        assert_eq!(
            locked_result.recommendation,
            ArtistResolutionRecommendation::LockedManual { artist_id: locked }
        );

        let rejected_result = resolver
            .evaluate(
                &[strong],
                &LocalResolutionConstraints {
                    locked_manual_artist_id: None,
                    negative_artist_ids: BTreeSet::from([rejected]),
                },
            )
            .expect("negative assertion resolution");
        assert_eq!(
            rejected_result.recommendation,
            ArtistResolutionRecommendation::RetainProvisional {
                reason: LocalProvisionalReason::NoEligibleCandidate
            }
        );
        assert!(rejected_result.assessments[0].suppressed_by_negative_assertion);
    }
}
