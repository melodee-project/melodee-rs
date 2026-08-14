//! PostgreSQL persistence for Melodee (ADR-0105, ADR-0110).
//!
//! Durable Release work items: schema migrations, transactional claiming
//! (`FOR UPDATE SKIP LOCKED`), leases, recovery, retries, failure evidence,
//! and append-only transition history. Domain rules live in `melodee-domain`;
//! this crate maps them to SQL and never invents state transitions itself.

#![forbid(unsafe_code)]

mod auth;
mod catalog;
mod catalog_identity;
mod catalog_mutations;
mod community;
mod config;
mod discovery;
mod error;
mod group_admin;
mod migrate;
mod native_user_state;
mod policy;
mod search;
mod user_admin;
mod work_items;
mod worker;
mod worker_pool;

pub use auth::{
    AuthLookup, AuthStore, AuthStoreError, AuthenticationRecord, BootstrapAdminRequest,
    MigrationIdentityBinding, NativeUserImport, PasswordResetIssue, PasswordResetOutcome,
    RateLimitOutcome, RateLimitPolicy, RefreshFamilyIssue, RefreshRotationOutcome,
    RefreshRotationRequest, SocialLoginLink, StoredNativeUser, StoredSocialLogin, StreamPrincipal,
    normalize_login,
};
pub use catalog::{
    CatalogCorrectionDimension, CatalogCounts, CatalogImportRequest, CatalogImportResult,
    CatalogStore, CatalogStoreError, LegacyCatalogEntityType, LegacyCatalogIdMapping,
    MAX_CATALOG_CORRECTION_IDENTITY_ROWS, MAX_CATALOG_CORRECTION_OVERLAY_REFERENCES,
    MAX_CATALOG_CORRECTION_RELEASES, MAX_CATALOG_CORRECTION_TRACKS, MAX_CATALOG_SPLIT_ASSIGNMENTS,
    PolicyEligibleWorkItem, ProviderNegativeAssertion, StoredArtistCandidate,
    StoredArtistTrackAssociation, StoredCatalogImport,
};
pub use community::{
    NativeShareImport, NativeShareImportResult, NativeShareStore, NativeShareStoreError,
};
pub use config::{
    ENV_CLAIM_BATCH_SIZE, ENV_DATABASE_URL, ENV_LEASE_SECONDS, ENV_MAX_ATTEMPTS,
    ENV_RETRY_BASE_SECONDS, ENV_RETRY_MAX_SECONDS, WorkConfigError, WorkEngineConfig,
};
pub use discovery::{
    MAX_CHART_CSV_BYTES, MAX_CHART_ITEMS, NativeChartImport, NativeChartImportResult,
    NativeDiscoveryMaintenanceStats, NativeDiscoveryStore, NativeDiscoveryStoreError,
};
pub use error::WorkStoreError;
pub use group_admin::{
    AccessGroupDetail, AccessGroupMember, GroupAdminError, GroupAdminStore, MAX_GROUP_NAME_LENGTH,
    StoredAccessGroup,
};
pub use migrate::{MIGRATOR, migrate};
pub use native_user_state::{
    MAX_NATIVE_PLAYLIST_ITEMS, MAX_NATIVE_PLAYLIST_PAGE_SIZE, NativeGenrePreference,
    NativePlaylist, NativePlaylistCreate, NativePlaylistItem, NativePlaylistUpdate,
    NativeScrobbleResult, NativeSmartPlaylist, NativeSmartPlaylistCreate,
    NativeSmartPlaylistUpdate, NativeUserState, NativeUserStateMutation, NativeUserStateStore,
    NativeUserStateStoreError,
};
pub use policy::{
    CatalogTrackFact, MAX_POLICY_CLASSIFICATION_EVIDENCE, MAX_POLICY_TEST_CLASSIFICATION_FACTS,
    MAX_POLICY_TEST_RELEASES, MAX_POLICY_TEST_SAMPLES, MAX_POLICY_TEST_TRACK_FACTS,
    MAX_POLICY_TRACKS_PER_RELEASE, PolicyAdminOverrideRequest,
    PolicyAudioIdentityAlternateAssertion, PolicyAudioIdentityNoMatchAssertion,
    PolicyCatalogContext, PolicyClaim, PolicyCommitResult, PolicyDecisionAuditEntry,
    PolicyDestructiveCompletion, PolicyEvaluationCommit, PolicyReleaseCatalogTraitAssertion,
    PolicyStore, PolicyStoreError, StoredPolicyDecision, StoredReleaseClassification,
};
pub use search::{
    MAX_SEARCH_OUTBOX_ATTEMPTS, MAX_SEARCH_OUTBOX_BATCH, MAX_SEARCH_SNAPSHOT_PAGE,
    SEARCH_PROJECTION_NAME, SearchArtistRecord, SearchCatalogRecord, SearchDocumentCounts,
    SearchEntityType, SearchOperation, SearchOutboxItem, SearchProjectionStatus,
    SearchProjectionStore, SearchReleaseRecord, SearchSnapshotReader, SearchStoreError,
    SearchTrackRecord, StoredSearchGeneration, StoredSearchRebuildJob,
};
pub use user_admin::{
    MAX_AVATAR_BYTES, MAX_USER_IDENTITY_CHARS, MAX_USER_ROLES, NativeUserAdminUpdate,
    NativeUserCreate, StoredAvatar, UserAdminError, UserAdminStore,
};
pub use work_items::{
    ActiveLease, PublicationJournalEntry, SettlingSnapshot, SettlingSnapshotFile, StateCount,
    StoredManifest, StoredPublication, WorkItemStore,
};
pub use worker::WorkerId;
pub use worker_pool::{
    MAX_POOL_ACTOR_LENGTH, MAX_POOL_NAME_LENGTH, POOL_DISCOVERY, POOL_PODCAST, POOL_POLICY,
    WorkerPoolState, WorkerPoolStore, WorkerPoolStoreError, is_pool_paused, is_valid_pool_name,
};
