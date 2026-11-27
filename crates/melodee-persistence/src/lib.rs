//! PostgreSQL persistence for Melodee (ADR-0105, ADR-0110).
//!
//! Durable Release work items: schema migrations, transactional claiming
//! (`FOR UPDATE SKIP LOCKED`), leases, recovery, retries, failure evidence,
//! and append-only transition history. Domain rules live in `melodee-domain`;
//! this crate maps them to SQL and never invents state transitions itself.

#![forbid(unsafe_code)]

mod admin_media_completion;
pub use admin_media_completion::{AdminArchiveIntent, AdminArchiveRoot};
mod admin_media_page_intent;
pub mod asset_files;
pub use admin_media_page_intent::{
    AdminDeletionIntentEntry, AdminDeletionPageIntent, AdminDeletionPageKind,
    AdminMediaRootExpectation, AdminQuarantineBinding, AdminQuarantineIntent,
    AdminQuarantineObservation, AdminQuarantineRecovery, AdminQuarantineRequest,
};
mod admin_media_reservation;
mod admin_media_snapshot;
pub use admin_media_reservation::{AdminMediaReservation, AdminReservedLifecycle};
pub use admin_media_snapshot::{
    AdminCatalogDeleteKind, AdminMediaObjectKey, AdminMediaSnapshotEntry, AdminMediaSnapshotObject,
};
mod auth;
mod catalog;
mod catalog_browse;
mod catalog_hard_delete;
mod catalog_identity;
mod catalog_images;
mod catalog_mutations;
mod community;
mod config;
mod device_profiles;
mod discovery;
mod discovery_scan;
pub use discovery_scan::DiscoveryScanFence;
mod custody_hash_spool;
mod error;
mod group_admin;
mod identity_review;
mod inbound_custody;
pub use custody_hash_spool::{
    CUSTODY_HASH_PAGE_BYTES, CUSTODY_HASH_PAGE_SIZE, CustodyHashCursor, CustodyHashEntry,
    CustodyHashObservation, CustodyHashPurpose, CustodyHashSpool, CustodyLegacyCursor,
    CustodyLegacyEntry, SealedCustodyHash,
};
pub use inbound_custody::{InboundCustodyClaim, InboundCustodyIntent};
mod installation;
mod job_history;
pub mod manifest_evidence;
pub mod metrics;
mod migrate;
mod native_user_state;
mod notifications;
mod protected_publication;
pub use protected_publication::{ProtectedPublication, ProtectedRetentionSession};
mod protected_stage_cleanup;
pub use protected_stage_cleanup::{ProtectedCleanupLease, ProtectedCleanupRetryReason};
mod protected_stage_attempt;
pub use protected_stage_attempt::{
    ClaimedStageReceipt, ProtectedBaselineDirectoryEntry, ProtectedBaselineVerityEntry,
    ProtectedCleanedReceiptRecovery, ProtectedReceiptRecovery, ProtectedStageAttempt,
    ProtectedStageIdentity,
};
mod policy;
pub mod processing;
mod provider_eligibility;
mod provider_enrichment;
mod providers;
mod radio;
mod retained_baseline;
pub use retained_baseline::{
    RetainedBaselineChunk, RetainedBaselineFile, RetainedBaselineFrozen, RetainedFileVerity,
    RetainedProtectedBaselineFrozen,
};
mod retained_baseline_directory;
pub use retained_baseline_directory::{
    RetainedBaselineDirectory, RetainedDirectoryAdvance, RetainedDirectoryContinuation,
    RetainedDirectoryMember,
};
mod retained_baseline_walk;
pub use retained_baseline_walk::{
    RetainedBaselinePage, RetainedBaselinePass, RetainedBaselineRead,
    RetainedBaselineUnreadableReason,
};
mod retained_chunk_checkpoint;
pub use retained_chunk_checkpoint::RetainedChunkCheckpoint;
mod retained_sources;
pub use retained_sources::{
    MAX_RETAINED_OBLIGATION_LIST, MAX_RETAINED_SOURCE_CLAIM, RetainedObligationView,
    RetainedRecheckOutcome, RetainedRecheckSession, RetainedRetryReason, RetainedSourceLease,
};
pub mod review_correction;
mod review_delete;
pub use review_delete::ReviewDeletionAuthorization;
pub mod runtime_settings;
mod scripts;
mod search;
mod search_retention;
mod settling_capture;
pub use settling_capture::{
    SETTLING_CAPTURE_PAGE_SIZE, SettlingObservation, StagedSnapshot, StagedSnapshotCompletion,
    VerifiedStagedSnapshot,
};
mod settings;
mod user_admin;
mod user_profile;
mod work_item_lifecycle;
pub use work_item_lifecycle::{
    LifecycleMaintenance, LifecycleProvisionLease, LifecycleQuarantineReason,
    LifecycleQuiescenceAttestation, WorkItemLifecycleIdentity,
};
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
    MAX_CATALOG_CORRECTION_TRACKS, MAX_CATALOG_SPLIT_ASSIGNMENTS, MAX_RELEASE_MERGE_SOURCES,
    NameOnlyArtistMatchProposalCandidate, PolicyEligibleWorkItem, ProviderNegativeAssertion,
    ReleaseMerge, ReleaseMergeEvent, ReleaseMergeImpact, ReleaseMergePreview,
    ReleaseMergePreviewRelease, ReleaseMetadataPatch, RepeatedPrimaryReleaseArtistMergeCandidate,
    SameReleaseTrackArtistMergeCandidate, StoredArtistCandidate, StoredArtistTrackAssociation,
    StoredCatalogImport, StrongArtistMergeCandidate, StrongReleaseMergeCandidate,
    TrackMetadataPatch,
};
pub use catalog_browse::{
    ArtistAliasRow, ArtistCatalogDetail, ArtistContributionRow, ArtistExternalIdRow,
    ArtistMembershipRow, ArtistRelationshipRow, ArtistReleaseRow, ArtistTrackCreditRow,
    BilledArtistRow, BoundedSection, CatalogArtistListRow, CatalogArtistSort, CatalogAuditEventRow,
    CatalogBrowseError, CatalogReleaseListRow, CatalogReleaseSort, CatalogTrackListRow,
    CatalogTrackSort, ContributionRow, MAX_CATALOG_BROWSE_QUERY_CHARS, MAX_CATALOG_DETAIL_ITEMS,
    MediaAssetRow, ReleaseCatalogDetail, ReleaseExternalIdentityRow, ReleaseMergeEventRow,
    ReleaseProvenanceRow, ReleaseTrackRow, TrackCatalogDetail, contains_pattern,
};
pub use catalog_hard_delete::{
    ArtistHardDeleteStorage, HardDeleteArtistPreview, HardDeleteArtistPreviewArtist,
    HardDeleteImpact, HardDeleteOutcome, HardDeleteReleasePreview, HardDeleteReleasePreviewRelease,
    HardDeleteTrackPreview, HardDeleteTrackPreviewTrack, MAX_HARD_DELETE_SOURCES,
    ReleaseHardDeleteStorage, TrackHardDeleteStorage,
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
pub use installation::installation_id;
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
pub use provider_eligibility::{
    BACKFILL_PAGE_SIZE, BackfillPage, EligibilityRefreshOutcome, ProviderBackfill,
    ProviderEligibility, ProviderEligibilityError, ProviderEligibilityState,
    ProviderEligibilityStore, ProviderQueueStats, ResolvedProviderEligibilityOwned,
    schedule_catalog_enrichment,
};
pub use provider_enrichment::{
    ClaimedEnrichmentTask, DEFAULT_ENRICHMENT_LEASE_SECONDS, EnrichmentEntityType,
    EnrichmentOutcome, EnrichmentPurpose, MAX_ENRICHMENT_ATTEMPTS, MAX_ENRICHMENT_BATCH,
    MAX_PROVIDER_IMAGE_BYTES, ProviderArtworkStore, ProviderEnrichmentStore,
    ProviderEnrichmentStoreError, ProviderIdentityCandidate, ProviderIdentityCandidateObservation,
    ProviderIdentityCandidateStore, StoredProviderImage, schedule_in_transaction,
};
pub use providers::{
    KNOWN_PROVIDERS, LastfmLinkRecord, LastfmLinkStore, MAX_LASTFM_USERNAME_CHARS,
    MAX_PROVIDER_CHARS, MAX_PROVIDER_PAYLOAD_BYTES, MAX_REQUEST_KEY_CHARS,
    MAX_SESSION_KEY_CIPHERTEXT_BYTES, MELODEE_PROVIDER_WIRE_ID, MIN_SESSION_KEY_CIPHERTEXT_BYTES,
    ProviderCacheEntry, ProviderCacheStore, ProviderDescriptor, ProviderStoreError,
    ResolvedProviderEligibility, SESSION_KEY_DIGEST_BYTES, is_known_provider, provider_descriptors,
    resolve_provider_eligibility,
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
pub use search_retention::{
    DEFAULT_PROCESSED_OUTBOX_RETENTION_DAYS, HOURLY_TOTAL_RETENTION_DAYS,
    MAX_PROCESSED_OUTBOX_RETENTION_DAYS, MIN_PROCESSED_OUTBOX_RETENTION_DAYS, OutboxPruneResult,
    OutboxRetentionConfig, OutboxRetentionStatus, finalize_closed_hours, maintenance_snapshot,
    projection_high_water, prune_processed_outbox, retention_status,
};
pub use settings::{
    CONFIG_EXPORT_FORMAT, CONFIG_EXPORT_WARNING, ConfigExportDocument, ConfigImportPlan,
    ENV_CONFIG_FILE, EffectiveSettingEntry, KNOWN_SETTINGS, KnownSetting, MAX_IMPORT_AUDIT_ENTRIES,
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
    ActiveLease, HourlyCompletionCount, InboundPurgeGuard, InboundPurgeGuardInfo,
    LegacyInboundPath, LegacyPathResolution, ProtectedClaim, ProtectedClaimEpoch,
    PublicationJournalEntry, PurgeGuardRecoveryAttestation, SettlingSnapshot, SettlingSnapshotFile,
    StagingHandoff, StagingHandoffMethod, StagingHandoffStatus, StateCount, StoredManifest,
    StoredPublication, WorkItemListRow, WorkItemSort, WorkItemStore, WorkItemWindowCounts,
    inbound_path_requires_review,
};
pub use worker::WorkerId;
pub use worker_pool::{
    MAX_POOL_ACTOR_LENGTH, MAX_POOL_NAME_LENGTH, POOL_DISCOVERY, POOL_PODCAST, POOL_POLICY,
    WorkerPoolMetrics, WorkerPoolState, WorkerPoolStore, WorkerPoolStoreError, is_pool_paused,
    is_valid_pool_name,
};

/// SQL operations for shared onboarding preflight.
pub mod onboarding;
/// SQL operations for podcast workers and recovery.
pub mod podcast_work;
