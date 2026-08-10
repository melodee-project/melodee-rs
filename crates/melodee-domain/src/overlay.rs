use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::values::{is_blake3, validate_internal_id, validate_nonblank};
use crate::{
    ArtistId, ArtistMatchDecisionId, AudioIdentityId, CatalogImportId, CatalogSemanticOverlayId,
    ContributionId, DisplayArtist, DomainInvariantError, ObservedArtistId, ReleaseArtistId,
    ReleaseId, TrackArtistId, TrackId,
};

/// Resolved Artist reference captured by one immutable semantic overlay revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OverlayArtistResolution {
    pub observed_artist_id: ObservedArtistId,
    pub artist_id: ArtistId,
    pub decision_id: Option<ArtistMatchDecisionId>,
}

/// Track occurrence to underlying audible identity link captured by an overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OverlayAudioIdentityLink {
    pub track_id: TrackId,
    pub audio_identity_id: AudioIdentityId,
}

/// Catalog entity whose exact DisplayArtist presentation is snapshotted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayArtistOwner {
    Release(ReleaseId),
    Track(TrackId),
}

/// Exact presentation captured independently from structured Artist links.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OverlayDisplayArtist {
    pub owner: DisplayArtistOwner,
    pub value: DisplayArtist,
}

/// Stable relational references included in a semantic overlay revision.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OverlayReferences {
    pub artist_resolutions: Vec<OverlayArtistResolution>,
    pub release_artists: Vec<ReleaseArtistId>,
    pub track_artists: Vec<TrackArtistId>,
    pub contributions: Vec<ContributionId>,
    pub audio_identities: Vec<OverlayAudioIdentityLink>,
    pub display_artists: Vec<OverlayDisplayArtist>,
}

impl OverlayReferences {
    fn normalize(&mut self) -> Result<(), DomainInvariantError> {
        sort_unique(&mut self.artist_resolutions)?;
        sort_unique(&mut self.release_artists)?;
        sort_unique(&mut self.track_artists)?;
        sort_unique(&mut self.contributions)?;
        sort_unique(&mut self.audio_identities)?;
        sort_unique(&mut self.display_artists)?;
        Ok(())
    }
}

/// Input facts for one immutable semantic overlay revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCatalogSemanticOverlay {
    pub id: CatalogSemanticOverlayId,
    pub release_id: ReleaseId,
    pub source_manifest_id: Uuid,
    pub source_semantic_manifest_hash: String,
    pub publication_plan_hash: String,
    pub catalog_import_id: CatalogImportId,
    pub resolver_version: String,
    pub predecessor_id: Option<CatalogSemanticOverlayId>,
    pub references: OverlayReferences,
    pub created_at: OffsetDateTime,
    pub created_by: String,
}

/// Immutable, versioned explanation of how manifest v3 became a catalog revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogSemanticOverlay {
    schema_name: &'static str,
    schema_version: u16,
    id: CatalogSemanticOverlayId,
    release_id: ReleaseId,
    source_manifest_id: Uuid,
    source_semantic_manifest_hash: String,
    publication_plan_hash: String,
    catalog_import_id: CatalogImportId,
    resolver_version: String,
    predecessor_id: Option<CatalogSemanticOverlayId>,
    references: OverlayReferences,
    created_at: OffsetDateTime,
    created_by: String,
}

impl CatalogSemanticOverlay {
    pub const SCHEMA_NAME: &'static str = "catalog.semantic";
    pub const SCHEMA_VERSION: u16 = 1;

    pub fn new(mut value: NewCatalogSemanticOverlay) -> Result<Self, DomainInvariantError> {
        validate_internal_id(value.id.is_uuid_v7(), "CatalogSemanticOverlayId")?;
        validate_internal_id(value.release_id.is_uuid_v7(), "ReleaseId")?;
        validate_internal_id(value.catalog_import_id.is_uuid_v7(), "CatalogImportId")?;
        if let Some(predecessor_id) = value.predecessor_id {
            validate_internal_id(predecessor_id.is_uuid_v7(), "CatalogSemanticOverlayId")?;
        }
        if value.predecessor_id == Some(value.id) {
            return Err(DomainInvariantError::SelfOverlayPredecessor);
        }
        if !is_blake3(&value.source_semantic_manifest_hash)
            || !is_blake3(&value.publication_plan_hash)
        {
            return Err(DomainInvariantError::InvalidBlake3);
        }
        validate_nonblank("overlay resolver version", &value.resolver_version)?;
        validate_nonblank("overlay audit actor", &value.created_by)?;
        for resolution in &value.references.artist_resolutions {
            validate_internal_id(
                resolution.observed_artist_id.is_uuid_v7(),
                "ObservedArtistId",
            )?;
            validate_internal_id(resolution.artist_id.is_uuid_v7(), "ArtistId")?;
            if let Some(decision_id) = resolution.decision_id {
                validate_internal_id(decision_id.is_uuid_v7(), "ArtistMatchDecisionId")?;
            }
        }
        for id in &value.references.release_artists {
            validate_internal_id(id.is_uuid_v7(), "ReleaseArtistId")?;
        }
        for id in &value.references.track_artists {
            validate_internal_id(id.is_uuid_v7(), "TrackArtistId")?;
        }
        for id in &value.references.contributions {
            validate_internal_id(id.is_uuid_v7(), "ContributionId")?;
        }
        for link in &value.references.audio_identities {
            validate_internal_id(link.track_id.is_uuid_v7(), "TrackId")?;
            validate_internal_id(link.audio_identity_id.is_uuid_v7(), "AudioIdentityId")?;
        }
        for presentation in &value.references.display_artists {
            match presentation.owner {
                DisplayArtistOwner::Release(id) => {
                    validate_internal_id(id.is_uuid_v7(), "ReleaseId")?;
                }
                DisplayArtistOwner::Track(id) => {
                    validate_internal_id(id.is_uuid_v7(), "TrackId")?;
                }
            }
        }
        value.references.normalize()?;
        Ok(Self {
            schema_name: Self::SCHEMA_NAME,
            schema_version: Self::SCHEMA_VERSION,
            id: value.id,
            release_id: value.release_id,
            source_manifest_id: value.source_manifest_id,
            source_semantic_manifest_hash: value.source_semantic_manifest_hash,
            publication_plan_hash: value.publication_plan_hash,
            catalog_import_id: value.catalog_import_id,
            resolver_version: value.resolver_version,
            predecessor_id: value.predecessor_id,
            references: value.references,
            created_at: value.created_at,
            created_by: value.created_by,
        })
    }

    #[must_use]
    pub const fn id(&self) -> CatalogSemanticOverlayId {
        self.id
    }

    #[must_use]
    pub const fn release_id(&self) -> ReleaseId {
        self.release_id
    }

    #[must_use]
    pub const fn source_manifest_id(&self) -> Uuid {
        self.source_manifest_id
    }

    #[must_use]
    pub fn source_semantic_manifest_hash(&self) -> &str {
        &self.source_semantic_manifest_hash
    }

    #[must_use]
    pub fn publication_plan_hash(&self) -> &str {
        &self.publication_plan_hash
    }

    #[must_use]
    pub const fn catalog_import_id(&self) -> CatalogImportId {
        self.catalog_import_id
    }

    #[must_use]
    pub fn resolver_version(&self) -> &str {
        &self.resolver_version
    }

    #[must_use]
    pub const fn predecessor_id(&self) -> Option<CatalogSemanticOverlayId> {
        self.predecessor_id
    }

    #[must_use]
    pub fn references(&self) -> &OverlayReferences {
        &self.references
    }

    #[must_use]
    pub const fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }

    #[must_use]
    pub fn created_by(&self) -> &str {
        &self.created_by
    }

    /// Deterministic semantic hash. Execution time, actor, and overlay row ID are audit
    /// provenance and do not change the resolved semantic snapshot.
    pub fn overlay_hash(&self) -> Result<String, serde_json::Error> {
        #[derive(Serialize)]
        struct SemanticOverlay<'a> {
            schema_name: &'a str,
            schema_version: u16,
            release_id: ReleaseId,
            source_manifest_id: Uuid,
            source_semantic_manifest_hash: &'a str,
            publication_plan_hash: &'a str,
            catalog_import_id: CatalogImportId,
            resolver_version: &'a str,
            predecessor_id: Option<CatalogSemanticOverlayId>,
            references: &'a OverlayReferences,
        }

        let semantic = SemanticOverlay {
            schema_name: self.schema_name,
            schema_version: self.schema_version,
            release_id: self.release_id,
            source_manifest_id: self.source_manifest_id,
            source_semantic_manifest_hash: &self.source_semantic_manifest_hash,
            publication_plan_hash: &self.publication_plan_hash,
            catalog_import_id: self.catalog_import_id,
            resolver_version: &self.resolver_version,
            predecessor_id: self.predecessor_id,
            references: &self.references,
        };
        serde_json::to_vec(&semantic).map(|bytes| blake3::hash(&bytes).to_hex().to_string())
    }
}

fn sort_unique<T: Ord>(values: &mut [T]) -> Result<(), DomainInvariantError> {
    values.sort_unstable();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        Err(DomainInvariantError::DuplicateOverlayReference)
    } else {
        Ok(())
    }
}
