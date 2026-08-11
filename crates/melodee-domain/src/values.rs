use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::Date;

/// Failures of pure Phase 4 catalog and identity invariants.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainInvariantError {
    #[error("{0} must be a UUIDv7 Melodee-owned identity")]
    InvalidInternalId(&'static str),
    #[error("{0} must not be blank")]
    BlankValue(&'static str),
    #[error("confidence basis points must be between 0 and 10000")]
    InvalidConfidence,
    #[error("date range start must not be after its end")]
    InvalidDateRange,
    #[error("an Artist relationship cannot connect an Artist to itself")]
    SelfArtistRelationship,
    #[error("a group membership must connect distinct member and group Artists")]
    SelfGroupMembership,
    #[error("a merge must connect distinct source and target identities")]
    SelfMerge,
    #[error("a split must create a distinct identity and move at least one assignment")]
    EmptyOrSelfSplit,
    #[error("split assignments must be unique")]
    DuplicateSplitAssignment,
    #[error("a playable Track must reference at least one playable MediaAsset")]
    MissingPlayableMediaAsset,
    #[error("a TrackMediaAsset must refer to its owning Track")]
    ForeignTrackMediaAsset,
    #[error("an ordinal must be greater than zero")]
    InvalidOrdinal,
    #[error("automatic Artist confirmation requires supporting non-name evidence")]
    AutomaticNameOnlyMatch,
    #[error("automatic Artist confirmation is forbidden while conflicting evidence exists")]
    ConflictingMatchEvidence,
    #[error("locked identity decisions must be made by an administrator")]
    InvalidDecisionLock,
    #[error("match evidence does not belong to the decision observation and candidate")]
    ForeignMatchEvidence,
    #[error("AudioIdentity evidence must compare distinct source and candidate identities")]
    SelfAudioIdentityEvidence,
    #[error("AudioIdentity evidence does not belong to the decision source and candidate")]
    ForeignAudioIdentityEvidence,
    #[error(
        "automatic AudioIdentity confirmation requires multiple independent supporting signals"
    )]
    InsufficientAudioIdentityEvidence,
    #[error("a BLAKE3 digest must be 64 lowercase hexadecimal characters")]
    InvalidBlake3,
    #[error("the canonical storage-relative path must not be blank")]
    InvalidCanonicalPath,
    #[error("an overlay cannot name itself as its predecessor")]
    SelfOverlayPredecessor,
    #[error("semantic overlay references must be unique")]
    DuplicateOverlayReference,
    #[error("classification evidence confidence must be between 0 and 10000 basis points")]
    InvalidClassificationConfidence,
    #[error("a policy rule identifier must not be blank")]
    BlankPolicyRuleId,
    #[error("a policy ruleset version must be greater than zero")]
    InvalidPolicyVersion,
    #[error("policy decision evidence summary must not be blank")]
    BlankPolicyEvidenceSummary,
    #[error("a policy decision actor must not be blank")]
    BlankPolicyActor,
    #[error("classification evidence does not belong to the owning classification")]
    ForeignClassificationEvidence,
    #[error(
        "every classification trait must have evidence and every evidence trait must be classified"
    )]
    IncompleteClassificationEvidence,
    #[error("classification evidence records must be unique")]
    DuplicateClassificationEvidence,
    #[error("classification {dimension} exceeds the limit of {limit}")]
    ClassificationScopeExceeded {
        dimension: &'static str,
        limit: usize,
    },
    #[error("novelty evaluation category counts do not match the total track count")]
    NoveltyCountMismatch,
    #[error("policy rule identifiers must be unique within a rule set")]
    DuplicatePolicyRuleId,
    #[error("the policy rule disposition is unsafe for its rule type")]
    InvalidPolicyRuleDisposition,
    #[error("a Delete policy decision requires a nonblank source fingerprint")]
    MissingPolicySourceFingerprint,
    #[error("policy decision completion state and timestamp are inconsistent")]
    InvalidDecisionCompletionState,
    #[error("the policy decision actor does not match the evaluated rule-set version")]
    PolicyActorVersionMismatch,
    #[error("policy decision completion state transition is invalid")]
    InvalidDecisionCompletionTransition,
    #[error("policy test summary aggregates are inconsistent")]
    InvalidPolicyTestSummary,
    #[error("policy test summary {dimension} exceeds the limit of {limit}")]
    PolicyTestScopeExceeded {
        dimension: &'static str,
        limit: usize,
    },
}

/// Inclusive confidence in basis points, avoiding floating-point persistence ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Confidence(u16);

impl Confidence {
    pub const MAX_BASIS_POINTS: u16 = 10_000;

    /// Zero confidence, for use as a safe fallback in const contexts.
    pub const fn zero() -> Self {
        Self(0)
    }

    pub fn new(basis_points: u16) -> Result<Self, DomainInvariantError> {
        if basis_points > Self::MAX_BASIS_POINTS {
            Err(DomainInvariantError::InvalidConfidence)
        } else {
            Ok(Self(basis_points))
        }
    }

    #[must_use]
    pub const fn basis_points(self) -> u16 {
        self.0
    }
}

impl TryFrom<u16> for Confidence {
    type Error = DomainInvariantError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Confidence> for u16 {
    fn from(value: Confidence) -> Self {
        value.basis_points()
    }
}

/// Inclusive, optionally open-ended date range used for active dates and membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "DateRangeRepresentation")]
pub struct DateRange {
    start: Option<Date>,
    end: Option<Date>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct DateRangeRepresentation {
    start: Option<Date>,
    end: Option<Date>,
}

impl TryFrom<DateRangeRepresentation> for DateRange {
    type Error = DomainInvariantError;

    fn try_from(value: DateRangeRepresentation) -> Result<Self, Self::Error> {
        Self::new(value.start, value.end)
    }
}

impl DateRange {
    pub fn new(start: Option<Date>, end: Option<Date>) -> Result<Self, DomainInvariantError> {
        if start.zip(end).is_some_and(|(start, end)| start > end) {
            return Err(DomainInvariantError::InvalidDateRange);
        }
        Ok(Self { start, end })
    }

    #[must_use]
    pub const fn start(self) -> Option<Date> {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> Option<Date> {
        self.end
    }

    /// Tests an exact known date. Unknown dates must not infer membership.
    #[must_use]
    pub fn contains(self, date: Date) -> bool {
        self.start.is_none_or(|start| date >= start) && self.end.is_none_or(|end| date <= end)
    }

    #[must_use]
    pub fn contains_known(self, date: Option<Date>) -> bool {
        date.is_some_and(|date| self.contains(date))
    }
}

pub(crate) fn validate_nonblank(
    field: &'static str,
    value: &str,
) -> Result<(), DomainInvariantError> {
    if value.trim().is_empty() {
        Err(DomainInvariantError::BlankValue(field))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_internal_id(
    valid: bool,
    name: &'static str,
) -> Result<(), DomainInvariantError> {
    if valid {
        Ok(())
    } else {
        Err(DomainInvariantError::InvalidInternalId(name))
    }
}

pub(crate) fn is_blake3(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
