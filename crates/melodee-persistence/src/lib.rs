//! PostgreSQL persistence for Melodee (ADR-0105, ADR-0110).
//!
//! Durable Release work items: schema migrations, transactional claiming
//! (`FOR UPDATE SKIP LOCKED`), leases, recovery, retries, failure evidence,
//! and append-only transition history. Domain rules live in `melodee-domain`;
//! this crate maps them to SQL and never invents state transitions itself.

#![forbid(unsafe_code)]

mod auth;
mod catalog;
mod catalog_browse;
mod catalog_identity;
mod catalog_images;
mod catalog_mutations;
mod community;
mod config;
mod device_profiles;
mod discovery;
mod error;
mod group_admin;
mod identity_review;
mod job_history;
mod migrate;
mod native_user_state;
mod notifications;
mod policy;
mod provider_enrichment;
mod providers;
mod radio;
mod scripts;
mod search;
mod settings;
mod user_admin;
mod user_profile;
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
    ArtistMetadataPatch, ArtistReconciliationEvidence, CatalogCorrectionDimension, CatalogCounts,
    CatalogImportRequest, CatalogImportResult, CatalogStore, CatalogStoreError,
    LegacyCatalogEntityType, LegacyCatalogIdMapping, MAX_CATALOG_CORRECTION_IDENTITY_ROWS,
    MAX_CATALOG_CORRECTION_OVERLAY_REFERENCES, MAX_CATALOG_CORRECTION_RELEASES,
    MAX_CATALOG_CORRECTION_TRACKS, MAX_CATALOG_SPLIT_ASSIGNMENTS,
    NameOnlyArtistMatchProposalCandidate, PolicyEligibleWorkItem, ProviderNegativeAssertion,
    ReleaseMetadataPatch, RepeatedPrimaryReleaseArtistMergeCandidate,
    SameReleaseTrackArtistMergeCandidate, StoredArtistCandidate, StoredArtistTrackAssociation,
    StoredCatalogImport, StrongArtistMergeCandidate, StrongReleaseMergeCandidate,
    TrackMetadataPatch,
};
pub use catalog_browse::{
    ArtistAliasRow, ArtistCatalogDetail, ArtistContributionRow, ArtistExternalIdRow,
    ArtistMembershipRow, ArtistRelationshipRow, ArtistReleaseRow, BilledArtistRow, BoundedSection,
    CatalogArtistListRow, CatalogArtistSort, CatalogAuditEventRow, CatalogBrowseError,
    CatalogReleaseListRow, CatalogReleaseSort, CatalogTrackListRow, CatalogTrackSort,
    ContributionRow, MAX_CATALOG_BROWSE_QUERY_CHARS, MAX_CATALOG_DETAIL_ITEMS, MediaAssetRow,
    ReleaseCatalogDetail, ReleaseProvenanceRow, ReleaseTrackRow, TrackCatalogDetail,
    contains_pattern,
};
pub use catalog_identity::{
    ArtistMergeImpact, ArtistMergePreview, ArtistMergePreviewArtist, MAX_ARTIST_MERGE_SOURCES,
};
pub use catalog_images::{
    CatalogImageAudit, CatalogImageEntity, CatalogImageError, CatalogImageStore,
    MAX_ENTITY_IMAGE_BYTES, StoredCatalogEntityImage,
};
pub use community::{
    NativeShareImport, NativeShareImportResult, NativeShareStore, NativeShareStoreError,
};
pub use config::{
    DEFAULT_DATABASE_ACQUIRE_TIMEOUT_SECONDS, DEFAULT_DATABASE_IDLE_TIMEOUT_SECONDS,
    DEFAULT_DATABASE_MAX_CONNECTIONS, DEFAULT_DATABASE_MIN_CONNECTIONS, DatabasePoolConfig,
    ENV_CLAIM_BATCH_SIZE, ENV_DATABASE_ACQUIRE_TIMEOUT_SECONDS, ENV_DATABASE_IDLE_TIMEOUT_SECONDS,
    ENV_DATABASE_MAX_CONNECTIONS, ENV_DATABASE_MIN_CONNECTIONS, ENV_DATABASE_URL,
    ENV_LEASE_SECONDS, ENV_MAX_ATTEMPTS, ENV_RETRY_BASE_SECONDS, ENV_RETRY_MAX_SECONDS,
    MAX_DATABASE_CONNECTIONS_CEILING, WorkConfigError, WorkEngineConfig,
};
pub use device_profiles::{
    DeviceProfile, DeviceProfileStore, DeviceProfileStoreError, DeviceProfileUpdate,
    MAX_CLIENT_MATCH_VALUE_CHARS, MAX_DEVICE_PROFILE_LIST, MAX_PROFILE_JSON_BYTES,
    MAX_PROFILE_NAME_CHARS, NewDeviceProfile,
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
pub use identity_review::{
    ArtistIdentityRef, IdentityDecisionFeedEntry, ObservedArtistCandidate,
    ObservedArtistCurrentDecision, ObservedArtistDetail, ObservedArtistMatchEvidence,
    ObservedArtistNegativeAssertion, ObservedArtistSort, UnresolvedObservedArtist,
};
pub use job_history::{
    DEFAULT_JOB_HISTORY_RETENTION_DAYS, JobHistoryStoreError, JobRunHistory, JobRunHistoryStore,
    MAX_JOB_HISTORY_PAGE, scheduled_tick,
};
pub use migrate::{MIGRATOR, MigrationStatus, migrate, migration_status};
pub use native_user_state::{
    MAX_NATIVE_PLAYLIST_ITEMS, MAX_NATIVE_PLAYLIST_PAGE_SIZE, NativeGenrePreference,
    NativePlaylist, NativePlaylistCreate, NativePlaylistItem, NativePlaylistUpdate,
    NativeScrobbleResult, NativeSmartPlaylist, NativeSmartPlaylistCreate,
    NativeSmartPlaylistUpdate, NativeUserState, NativeUserStateMutation, NativeUserStateStore,
    NativeUserStateStoreError,
};
pub use notifications::{
    DeliveryStatus, MAX_BODY_BYTES, MAX_DELIVERY_ERROR_CHARS, MAX_DELIVERY_LIST, MAX_LOCALE_CHARS,
    MAX_RECIPIENT_CHARS, MAX_SUBJECT_CHARS, MAX_TEMPLATE_KEY_CHARS, MAX_TEMPLATE_LIST,
    NewNotificationDelivery, NotificationDelivery, NotificationDeliveryStore,
    NotificationStoreError, NotificationTemplate, NotificationTemplateStore,
    NotificationTemplateUpsert, is_valid_locale,
};
pub use policy::{
    CatalogTrackFact, MAX_POLICY_CLASSIFICATION_EVIDENCE, MAX_POLICY_TEST_CLASSIFICATION_FACTS,
    MAX_POLICY_TEST_RELEASES, MAX_POLICY_TEST_SAMPLES, MAX_POLICY_TEST_TRACK_FACTS,
    MAX_POLICY_TRACKS_PER_RELEASE, PolicyAdminOverrideRequest,
    PolicyAudioIdentityAlternateAssertion, PolicyAudioIdentityNoMatchAssertion,
    PolicyCatalogContext, PolicyClaim, PolicyCommitResult, PolicyDecisionAuditEntry,
    PolicyDestructiveCompletion, PolicyEvaluationCommit, PolicyReleaseCatalogTraitAssertion,
    PolicyStore, PolicyStoreError, PolicyTestSampleRelease, StoredNoveltyEvaluation,
    StoredPolicyDecision, StoredReleaseClassification, StoredTrackNovelty,
};
pub use provider_enrichment::{
    ClaimedEnrichmentTask, DEFAULT_ENRICHMENT_LEASE_SECONDS, EnrichmentEntityType,
    EnrichmentOutcome, EnrichmentPurpose, MAX_ENRICHMENT_ATTEMPTS, MAX_ENRICHMENT_BATCH,
    MAX_PROVIDER_IMAGE_BYTES, ProviderArtworkStore, ProviderEnrichmentStore,
    ProviderEnrichmentStoreError, ProviderIdentityCandidateObservation,
    ProviderIdentityCandidateStore, StoredProviderImage, schedule_in_transaction,
};
pub use providers::{
    KNOWN_PROVIDERS, LastfmLinkRecord, LastfmLinkStore, MAX_LASTFM_USERNAME_CHARS,
    MAX_PROVIDER_CHARS, MAX_PROVIDER_PAYLOAD_BYTES, MAX_REQUEST_KEY_CHARS,
    MAX_SESSION_KEY_CIPHERTEXT_BYTES, MIN_SESSION_KEY_CIPHERTEXT_BYTES, ProviderCacheEntry,
    ProviderCacheStore, ProviderStoreError, SESSION_KEY_DIGEST_BYTES, is_known_provider,
};
pub use radio::{
    MAX_RADIO_NAME_CHARS, MAX_RADIO_URL_CHARS, NewRadioStation, RadioStation, RadioStationStore,
    RadioStationUpdate, RadioStoreError,
};
pub use scripts::{
    MAX_MEMORY_BYTES, MAX_RUN_ERROR_CHARS, MAX_RUN_LIST, MAX_SCRIPT_EVENT_CHARS, MAX_SCRIPT_LIST,
    MAX_SCRIPT_NAME_CHARS, MAX_SCRIPT_SOURCE_BYTES, MAX_TIMEOUT_MS, MIN_MEMORY_BYTES, NewScriptRun,
    RegisteredScript, ScriptRegistryStore, ScriptRun, ScriptRunOutcome, ScriptRunStore,
    ScriptStoreError, ScriptUpsert,
};
pub use search::{
    MAX_SEARCH_OUTBOX_ATTEMPTS, MAX_SEARCH_OUTBOX_BATCH, MAX_SEARCH_SNAPSHOT_PAGE,
    SEARCH_PROJECTION_NAME, SearchArtistRecord, SearchCatalogRecord, SearchDocumentCounts,
    SearchEntityType, SearchOperation, SearchOutboxItem, SearchProjectionStatus,
    SearchProjectionStore, SearchReleaseRecord, SearchSnapshotReader, SearchStoreError,
    SearchTrackRecord, StoredSearchGeneration, StoredSearchRebuildJob,
};
pub use settings::{
    CONFIG_EXPORT_FORMAT, CONFIG_EXPORT_WARNING, ConfigExportDocument, ConfigImportPlan,
    ENV_CONFIG_FILE, KNOWN_SETTINGS, KnownSetting, MAX_IMPORT_AUDIT_ENTRIES,
    MAX_IMPORT_DOCUMENT_BYTES, MAX_IMPORT_KEYS, MAX_SETTING_KEY_CHARS, MAX_SETTING_VALUE_BYTES,
    MAX_SETTINGS_LIST, OnboardingState, OnboardingStore, ResolvedSetting, SECRET_REDACTION_MARKER,
    ServerSetting, SettingKind, SettingSource, SettingValueView, SettingsError, SettingsResolver,
    SettingsStore, apply_config_import, env_var_name, export_config, is_secret_key, known_setting,
    plan_config_import,
};
pub use user_admin::{
    MAX_AVATAR_BYTES, MAX_USER_IDENTITY_CHARS, MAX_USER_ROLES, NativeUserAdminUpdate,
    NativeUserCreate, StoredAvatar, UserAdminError, UserAdminStore, UserListPage, UserSort,
};
pub use user_profile::{
    MAX_DESCRIPTION_CHARS, MAX_EMAIL_CHARS, MAX_LANGUAGE_CHARS, MAX_THEME_CHARS,
    MAX_TIME_ZONE_CHARS, MAX_USERNAME_CHARS, MIN_PASSWORD_CHARS, ProfileUpdate,
    RegistrationRequest, UserProfileError, UserProfileStore, valid_time_zone_id,
};
pub use work_items::{
    ActiveLease, HourlyCompletionCount, PublicationJournalEntry, SettlingSnapshot,
    SettlingSnapshotFile, StagingHandoff, StagingHandoffMethod, StagingHandoffStatus, StateCount,
    StoredManifest, StoredPublication, WorkItemListRow, WorkItemSort, WorkItemStore,
    WorkItemWindowCounts,
};
pub use worker::WorkerId;
pub use worker_pool::{
    MAX_POOL_ACTOR_LENGTH, MAX_POOL_NAME_LENGTH, POOL_DISCOVERY, POOL_PODCAST, POOL_POLICY,
    WorkerPoolMetrics, WorkerPoolState, WorkerPoolStore, WorkerPoolStoreError, is_pool_paused,
    is_valid_pool_name,
};
