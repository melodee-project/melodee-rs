/**
 * Admin API DTOs (docs/src/content/docs/api/admin-api.md). The admin API is
 * Rust-native and camelCase; it is not a frozen compatibility surface, but
 * these shapes mirror the documented behavior.
 */

/** Stable error envelope returned by every admin endpoint. */
export interface ApiErrorBody {
  code: string;
  message: string;
  correlationId: string;
}

// ---- Catalog metadata patch types (PUT /api/admin/v1/catalog/{entity}/{id}/metadata) ----

export interface ReleaseMetadataPatch {
  title?: string | null;
  sortTitle?: string | null;
  displayArtist?: string | null;
  /** ISO date `YYYY-MM-DD`. */
  releaseDate?: string | null;
  releaseDateText?: string | null;
}

export interface ArtistMetadataPatch {
  canonicalName?: string | null;
  sortName?: string | null;
  artistType?: string | null;
  /**
   * Operator identity assertion. Import always creates `active` (usable, not
   * identity-verified); this is the audited path to `verified`.
   */
  status?: 'active' | 'verified' | null;
}

export interface TrackMetadataPatch {
  title?: string | null;
  sortTitle?: string | null;
  displayArtist?: string | null;
  ordinal?: number | null;
  durationMs?: number | null;
}

export interface MetadataFieldDelta {
  file: string;
  field: string;
  old: string | null;
  new: string | null;
}

export interface MetadataTransformPreview {
  previewHash: string;
  operation: string;
  affectedFiles: number;
  deltas: MetadataFieldDelta[];
  validationErrors: string[];
}

export interface MetadataTransformResult {
  auditId: string;
  status: string;
  previewHash: string;
  affectedFiles: number;
  deltas: MetadataFieldDelta[];
  recovery: unknown;
}

export interface MetadataTransformAudit {
  id: string;
  releaseId: string;
  operationName: string;
  operationParams: unknown;
  affectedFiles: number;
  fieldDeltas: MetadataFieldDelta[];
  appliedBy: string;
  status: string;
  recoveryEvidence: unknown;
  createdAt: string;
  completedAt: string | null;
}

/** Server-side pagination envelope shared by list endpoints. */
export interface Page<T> {
  items: T[];
  total: number;
  limit: number;
  offset: number;
}

export interface SystemInfo {
  service: string;
  version: string;
  status: string;
}

/**
 * Durable per-Release ingestion states (melodee-domain IngestionState,
 * snake_case wire form). The union is the full canonical set; the counts
 * endpoint reports only states that currently have items.
 */
export type IngestionState =
  | 'discovered'
  | 'settling'
  | 'ready'
  | 'inspecting'
  | 'normalizing'
  | 'validating'
  | 'needs_review'
  | 'staged'
  | 'publishing'
  | 'published'
  | 'cataloging'
  | 'cataloged'
  | 'indexing'
  | 'indexed'
  | 'failed'
  | 'quarantined'
  | 'ignored'
  | 'deleted';

/**
 * Operator-facing phase (melodee-domain IngestionPhase, ADR-0185): a derived
 * grouping of `IngestionState`. Operators filter and label by phase; the state
 * remains the precise detail.
 */
export type IngestionPhase =
  | 'inbound'
  | 'processing'
  | 'publishing'
  | 'catalog'
  | 'active'
  | 'review'
  | 'disposition'
  | 'failed';

/** One {state, count} row from GET /ingest/work-items/counts. */
export interface PurgeWorkItemFailure {
  id: string;
  code: string;
  message: string;
}

export interface PurgeWorkItemsResult {
  deleted: { id: string; paths: Record<string, string> }[];
  failed: PurgeWorkItemFailure[];
}

export interface WorkItemStateCount {
  state: IngestionState;
  count: number;
}

/**
 * Diagnostics support bundle (BundleReport,
 * apps/melodee-server/src/admin_diagnostics.rs). Only the fields the Admin UI
 * renders are typed.
 */
export interface DiagnosticsBundle {
  generatedAt: string;
  uptimeSeconds: number;
  server: { name: string; version: string; buildProfile: string; pid: number };
}

/**
 * WorkItemDto (apps/melodee-server/src/admin.rs). Diagnostics-only: paths,
 * state, attempts, lease evidence, last error, timestamps, correlation id.
 *
 * The optional `releaseId`/`releaseTitle`/`artistName`/`mediaBytes` fields are
 * display-only enrichment present once the item has a catalog import binding
 * (ADR-0184). They never identify the item and are absent otherwise — render
 * them as absent, never as zero or an empty title.
 */
export interface WorkItem {
  id: string;
  apiKey: string;
  sourceLibraryId: string | null;
  targetLibraryId: string | null;
  sourcePath: string;
  currentPath: string;
  targetPath: string | null;
  sourceFingerprint: string | null;
  manifestHash: string | null;
  state: IngestionState;
  stateReason: string | null;
  attemptCount: number;
  nextAttemptAt: string | null;
  leaseOwner: string | null;
  leaseAcquiredAt: string | null;
  leaseExpiresAt: string | null;
  lastErrorCode: string | null;
  lastErrorMessage: string | null;
  correlationId: string;
  discoveredAt: string;
  firstStartedAt: string | null;
  lastAttemptAt: string | null;
  updatedAt: string;
  createdAt: string;
  completedAt: string | null;
  /** Derived operator-facing phase (ADR-0185); always present from the API. */
  phase?: IngestionPhase;
  /**
   * True once client surfaces can see and stream the Release (policy accept has
   * set the Release status to `active`). This — not the search projection — is
   * the "ready to listen" signal an operator looks for.
   */
  clientVisible?: boolean;
  /** Authoritative client-facing Release status once the import is bound. */
  releaseStatus?: string | null;
  /** Bound catalog Release (display only; absent before an import binds). */
  releaseId?: string | null;
  releaseTitle?: string;
  artistName?: string;
  mediaBytes?: number;
}

/** Append-only transition/audit event (WorkItemEventDto). */
export interface WorkItemEvent {
  id: number;
  workItemId: string;
  eventType: string;
  fromState: IngestionState | null;
  toState: IngestionState | null;
  reasonCode: string | null;
  workerId: string | null;
  correlationId: string;
  createdAt: string;
  metadata: unknown;
}

/** Active (unexpired) lease evidence — ActiveLeaseDto; returned as a bare array. */
export interface Lease {
  workItemId: string;
  apiKey: string;
  state: IngestionState;
  leaseOwner: string;
  leaseAcquiredAt: string;
  leaseExpiresAt: string;
  lastActivityAt: string;
}

export interface UserSummary {
  id: string;
  username: string;
  email: string;
  isAdmin: boolean;
  isEditor: boolean;
  enabled: boolean;
  locked: boolean;
  roles: string[];
  description: string | null;
  timeZoneId: string;
  preferredLanguage: string | null;
  preferredTheme: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface UserCreateRequest {
  username: string;
  email: string;
  password: string;
  isAdmin?: boolean;
  isEditor?: boolean;
  roles?: string[];
}

export interface UserUpdateRequest {
  isAdmin?: boolean;
  isEditor?: boolean;
  enabled?: boolean;
  locked?: boolean;
  roles?: string[];
  username?: string;
  email?: string;
  /** `null` clears the description; omission leaves it unchanged. */
  description?: string | null;
  timeZoneId?: string;
  preferredLanguage?: string | null;
  preferredTheme?: string | null;
}

/** Access group list row (AccessGroupDto in admin_access.rs). */
export interface AccessGroup {
  id: string;
  name: string;
  memberCount: number;
  bindingCount: number;
  createdAt: string;
  updatedAt: string;
}

/** Access group detail (AccessGroupDetailDto; group fields flattened). */
export interface AccessGroupDetail extends AccessGroup {
  members: { userId: string; username: string }[];
  libraryIds: string[];
}

/** Internet radio station (RadioStationDto). The id is immutable once issued. */
export interface RadioStation {
  id: string;
  name: string;
  streamUrl: string;
  homePageUrl: string | null;
  createdAt: string;
  updatedAt: string;
}

/**
 * Device/transcoding profile (DeviceProfileDto). Metadata only: `profile`
 * records transcoding intent for a future resolver; it does not control
 * streaming today (PRD-DEVICE-002/003).
 */
export interface DeviceProfile {
  id: string;
  name: string;
  clientMatch: Record<string, string>;
  profile: Record<string, unknown>;
  isDefault: boolean;
  createdAt: string;
  updatedAt: string;
}

/**
 * Notification template (NotificationTemplateDto). `subject`/`bodyText`/
 * `bodyHtml` are the full operator-manageable text; null means unset.
 */
export interface NotificationTemplate {
  templateKey: string;
  locale: string;
  subject: string | null;
  bodyText: string | null;
  bodyHtml: string | null;
  version: number;
  updatedAt: string;
}

/**
 * Delivery audit row (NotificationDeliveryDto). Contains no message
 * content — the audit never stores subjects or bodies (PRD-NOTIF-002).
 */
export interface NotificationDelivery {
  id: string;
  templateKey: string;
  recipient: string;
  status: 'sent' | 'failed' | 'skipped';
  error: string | null;
  createdAt: string;
}

/**
 * Configuration export document (ConfigExportDocument). Every secret value
 * is replaced by the fixed redaction marker; environment/config-file
 * overlays are deployment concerns and are not captured.
 */
export interface ConfigExport {
  format: string;
  warning: string;
  exportedAt: string;
  settings: Record<string, unknown>;
}

/**
 * Import preview/apply result (ConfigImportResponse): the diff plan plus
 * whether anything was written. Secret keys appear BY NAME ONLY
 * (PRD-AUTH-007); values never cross the document/DB boundary.
 */
export interface ConfigImportResult {
  applied: boolean;
  /** Known keys with no stored row whose value differs from the default. */
  added: string[];
  /** Known keys whose stored value would change. */
  changed: string[];
  /** Known keys whose effective value already matches. */
  unchanged: string[];
  /** Secret keys with a real (non-marker) value to store. */
  secretsReentered: string[];
  /** Secret keys present as the redaction marker: skipped on apply. */
  secretsSkipped: string[];
  /** Keys outside the known-settings registry. */
  unknown: string[];
  warnings: string[];
}

/** One preflight check (PreflightCheckDto); remediation is null when passed. */
export interface OnboardingCheck {
  name: string;
  required: boolean;
  passed: boolean;
  detail: string;
  remediation: string | null;
}

/** Onboarding status (OnboardingStatusDto). */
export interface OnboardingStatus {
  completed: boolean;
  bypassed: boolean;
  canComplete: boolean;
  completedBy: string | null;
  checks: OnboardingCheck[];
}

/**
 * One registered script (ScriptDto). `wired` reports whether the event has
 * a live server-side evaluation point in this build; scripts registered for
 * unwired events are inert until their hook lands.
 */
export interface AdminScript {
  id: string;
  event: string;
  name: string;
  source: string;
  enabled: boolean;
  wired: boolean;
  timeoutMs: number;
  memoryBytes: number;
  version: number;
  createdAt: string;
  updatedAt: string;
}

/** GET /scripts envelope (ScriptListResponse) with the wired-event registry. */
export interface ScriptListResponse {
  scripts: AdminScript[];
  wiredEvents: string[];
}

/** One script execution audit row (ScriptRunDto). */
export interface ScriptRun {
  id: string;
  scriptId: string | null;
  event: string;
  outcome: string;
  durationMs: number;
  error: string | null;
  createdAt: string;
}

/** Sandbox evaluation result (EvaluationDto). */
export interface ScriptEvaluation {
  outcome: string;
  message: string | null;
  error: string | null;
  durationMs: number;
}

/** Operator test-run result (TestRunResponse). */
export interface ScriptTestRunResult {
  evaluation: ScriptEvaluation;
  run: ScriptRun;
  /** Effective deny action for this build (legacy `delete` → `skip`). */
  effectiveOnDeny: string;
}

// ---- Release Review (PRD-API-ADMIN-002) ----
// Shapes mirror apps/melodee-server/src/admin_review.rs (camelCase serde).

/** One review-queue row: the work item plus its catalog Release binding. */
export interface ReviewQueueItem extends WorkItem {
  releaseId: string | null;
}

/** Paginated review queue with per-review-state counts in the envelope. */
export interface ReviewQueuePage extends Page<ReviewQueueItem> {
  countsByState: WorkItemStateCount[];
}

/** One manifest track row as shown in review (ReviewTrackDto). */
export interface ReviewTrack {
  index: number;
  filePath: string;
  title: string | null;
  durationMs: number | null;
  container: string | null;
  codec: string | null;
  mimeType: string | null;
  /** `captured` | `missing` | `pending` (normalization fingerprint state). */
  fingerprintStatus: string;
}

export interface ReviewArtwork {
  relativePath: string;
  source: string;
  width: number | null;
  height: number | null;
  sizeBytes: number;
  mimeType: string | null;
  isEmbedded: boolean;
}

export interface ReviewSidecar {
  relativePath: string;
  sizeBytes: number;
  mimeType: string | null;
}

/** Manifest summary for review — never the raw payload blob. */
export interface ReviewManifestSummary {
  manifestHash: string;
  schemaVersion: number;
  generatedAt: string;
  sourcePath: string;
  releaseTitle: string | null;
  releaseArtists: string[];
  tracks: ReviewTrack[];
  artwork: ReviewArtwork[];
  sidecars: ReviewSidecar[];
  findingCount: number;
}

/** One classification evidence row; confidence is basis points (0–10000). */
export interface ClassificationEvidence {
  trait: string;
  source: string;
  kind: string;
  confidence: number;
  detail: string;
}

export interface ReviewClassification {
  traits: string[];
  /** Basis points (0–10000). */
  confidence: number;
  classifiedAt: string;
  classifierVersion: string;
  evidence: ClassificationEvidence[];
}

/** Per-Track novelty row (TrackNoveltyCategory wire values, snake_case). */
export interface NoveltyTrack {
  trackId: string;
  category: string;
  /** Basis points (0–10000). */
  confidence: number;
  evidenceKind: string;
  detail: string;
}

/** Catalog comparison aggregates plus per-Track rows (CatalogComparisonDto). */
export interface CatalogComparison {
  known: number;
  new: number;
  alternate: number;
  uncertain: number;
  exactDuplicates: number;
  knownAudioIdentities: number;
  alternateRecordings: number;
  newTracks: number;
  uncertainTracks: number;
  totalTracks: number;
  evaluatedAt: string;
  evaluatorVersion: string;
  tracks: NoveltyTrack[];
}

/** The durable policy decision for the bound Release (DecisionDto). */
export interface ReviewDecision {
  id: string;
  releaseId: string;
  ruleSetId: string;
  ruleSetVersion: number;
  disposition: string;
  completionState: string;
  matchedRuleIds: string[];
  evidenceSummary: string;
  overrideReason: string | null;
  actor: unknown;
  decidedAt: string;
  completedAt: string | null;
}

/** Full release review detail (ReviewDetailDto). */
export interface ReviewDetail {
  workItem: WorkItem;
  releaseId: string | null;
  manifest: ReviewManifestSummary | null;
  classification: ReviewClassification | null;
  catalogComparison: CatalogComparison | null;
  decision: ReviewDecision | null;
  timeline: WorkItemEvent[];
}

/** Result of an applied review decision (ReviewDecisionResultDto). */
export interface ReviewDecisionResult {
  workItem: WorkItem;
  decision: ReviewDecision | null;
  /** True when a delete authorization was recorded and execution is pending. */
  pendingExecution: boolean;
}

/** Sort parameter in the documented `key:asc|desc` whitelist form. */
export type SortParam = `${string}:asc` | `${string}:desc`;

export interface ListParams {
  limit?: number;
  offset?: number;
  sort?: SortParam;
}

// ---- Artist identity review (apps/melodee-server/src/admin_identity.rs) ----

/**
 * One unresolved observation row (UnresolvedObservationDto). A string is
 * never an Artist identity: `displayValue`/`rawValue` are observed strings
 * awaiting resolution to a Melodee-owned Artist.
 */
export interface UnresolvedObservation {
  id: string;
  displayValue: string;
  rawValue: string;
  normalizedValue: string;
  scope: string;
  sourceOrdinal: number;
  sourcePointer: string;
  releaseId: string;
  releaseTitle: string;
  trackId: string | null;
  trackTitle: string | null;
  pendingMatchCount: number;
  evidenceCount: number;
  createdAt: string;
}

/** The observation's current (latest) decision, when one exists. */
export interface IdentityCurrentDecision {
  artistId: string;
  artistName: string;
  outcome: string;
  authority: string;
  /** Basis points out of 10_000. */
  confidence: number;
  reason: string;
  locked: boolean;
  decidedAt: string;
}

/** One persisted match-evidence fact for a candidate (MatchEvidenceDto). */
export interface MatchEvidence {
  kind: string;
  polarity: string;
  /** Basis points out of 10_000. */
  confidence: number;
  source: string;
  payload: unknown;
  createdAt: string;
}

/** A candidate Artist match with its evidence breakdown (CandidateDto). */
export interface IdentityCandidate {
  artistId: string;
  artistName: string;
  artistDisambiguation: string | null;
  artistStatus: string;
  latestOutcome: string;
  /** Basis points out of 10_000. */
  latestConfidence: number;
  latestReason: string;
  latestLocked: boolean;
  latestDecidedAt: string;
  decisionCount: number;
  evidence: MatchEvidence[];
}

/** A durable do-not-match assertion (NegativeAssertionDto). */
export interface NegativeAssertion {
  id: string;
  candidateArtistId: string | null;
  candidateArtistName: string | null;
  candidateProvider: string | null;
  candidateExternalId: string | null;
  authority: string;
  reason: string;
  locked: boolean;
  active: boolean;
  createdAt: string;
}

/** Observation detail with candidates and assertions (ObservationDetailDto). */
export interface ObservationDetail {
  observation: UnresolvedObservation;
  current: IdentityCurrentDecision | null;
  candidates: IdentityCandidate[];
  negativeAssertions: NegativeAssertion[];
}

/** Resolution verbs accepted by the resolve endpoint. `artistId` is required
 *  for `confirm` and forbidden for the other two. */
export type IdentityResolution = 'confirm' | 'create-new' | 'leave-unresolved';

export interface ResolveObservationResult {
  observationId: string;
  resolution: string;
  artistId: string | null;
  changed: boolean;
}

export interface NeverMatchResult {
  observationId: string;
  artistId: string;
  recorded: boolean;
}

/** One identity audit-feed entry (IdentityDecisionEntryDto). */
export interface IdentityDecisionEntry {
  eventId: string;
  entityType: string;
  entityId: string;
  eventType: string;
  actor: string;
  reason: string;
  payload: unknown;
  createdAt: string;
}

export interface MergeArtistsResult {
  mergeId: string;
  sourceArtistId: string;
  targetArtistId: string;
}

export interface ArtistMergePreviewArtist {
  id: string;
  name: string;
  sortName: string;
  artistType: CatalogArtistType;
  disambiguation: string | null;
  beginDate: string | null;
  endDate: string | null;
  status: CatalogArtistStatus;
}

export interface ArtistMergeImpact {
  releases: number;
  tracks: number;
  releaseCredits: number;
  trackCredits: number;
  contributions: number;
  aliases: number;
  externalIdentities: number;
  relationships: number;
  memberships: number;
  observations: number;
  userStates: number;
  userStateConflicts: number;
  chartLinks: number;
  operatorImages: number;
  providerImages: number;
  providerImageConflicts: number;
  providerCandidates: number;
  providerTasks: number;
}

export interface ArtistMergePreview {
  target: ArtistMergePreviewArtist;
  sources: ArtistMergePreviewArtist[];
  impact: ArtistMergeImpact;
  metadataConflicts: string[];
}

export interface BatchMergeArtistsResult {
  mergeIds: string[];
  sourceArtistIds: string[];
  targetArtistId: string;
}

/** Split assignment kinds accepted by the backend (SplitAssignment). */
export type SplitAssignmentKind =
  | 'releaseArtist'
  | 'trackArtist'
  | 'contribution'
  | 'alias'
  | 'externalIdentity';

export interface SplitAssignmentInput {
  kind: SplitAssignmentKind;
  /** Internal durable row identity of the selected link (not a public id). */
  id: string;
}

export interface NewArtistInput {
  canonicalName: string;
  sortName?: string;
  artistType?: string;
  disambiguation?: string;
}

export interface SplitArtistResult {
  splitId: string;
  sourceArtistId: string;
  newArtistId: string;
  movedAssignments: number;
}

// ---- Dashboard / system / workers / search ----
// Shapes mirror apps/melodee-server/src/admin_system.rs (camelCase serde).

/** One subsystem's health verdict (SubsystemHealthDto). `disabled` is a
 * deliberately unconfigured optional subsystem, never a failure. Additional states include
 *  `unavailable` (a required dependency cannot be reached), `readOnly`,
 *  `misconfigured`, and `capacityExhausted` so operators can distinguish
 *  failure modes without reading detail strings. */
export type SubsystemHealthStatus =
  | 'ok'
  | 'degraded'
  | 'disabled'
  | 'unavailable'
  | 'readOnly'
  | 'misconfigured'
  | 'capacityExhausted';

export interface SubsystemHealth {
  status: SubsystemHealthStatus;
  detail: string;
}

/** One mounted API surface (ApiSurfaceDto). */
export interface ApiSurfaceStatus {
  surface: string;
  status: string;
}

/** Point-in-time utilization of one semaphore-bounded resource class
 * (ResourceBoundSnapshot). Purely observational. */
export interface ResourceBoundSnapshot {
  class: string;
  max: number;
  inFlight: number;
  waiting: number;
  oldestWaitMs: number;
  admittedTotal: number;
  rejectedTotal: number;
  saturated: boolean;
}

/** SQLx PostgreSQL pool utilization exposed without connection secrets. */
export interface PostgresqlPoolSnapshot {
  maxConnections: number;
  openConnections: number;
  idleConnections: number;
  inUseConnections: number;
  utilizationPercent: number;
}

/** Aggregated subsystem health (HealthDto, SPEC §22 panel content). The
 *  top-level `status` is `ok` when PostgreSQL is reachable and Tantivy is
 * healthy-or-disabled; subsystems report their own statuses without
 *  flipping the top-level indicator. */
export interface SystemHealth {
  status: 'ok' | 'degraded';
  postgresql: SubsystemHealth;
  postgresqlPool: PostgresqlPoolSnapshot;
  tantivy: SubsystemHealth;
  storage: SubsystemHealth;
  ffmpeg: SubsystemHealth;
  /** podcast storage-root writability (disabled when the podcast
   *  worker is unconfigured). */
  podcastStorage: SubsystemHealth;
  /** free space across storage/work-area/podcast roots. */
  disk: SubsystemHealth;
  /** stale leases and queue depth across the durable worker pools. */
  workers: SubsystemHealth;
  apiSurfaces: ApiSurfaceStatus[];
  /** bounded-resource classes registered at startup (empty in tests). */
  resourceBounds: ResourceBoundSnapshot[];
}

export interface DashboardCatalog {
  artists: number;
  releases: number;
  tracks: number;
  mediaAssets: number;
  pendingSearchOutbox: number;
}

export interface DashboardWorkItems {
  countsByState: WorkItemStateCount[];
  failedLast24h: number;
  completedLast24h: number;
}

export interface DashboardAttention {
  unresolvedIdentities: number;
  pendingPolicyDecisions: number;
  failedProcessing: number;
  quarantined: number;
}

/** One completed-per-hour bucket (DashboardActivityBucketDto). Hours without
 *  completions are omitted by the backend. */
export interface DashboardActivityBucket {
  hour: string;
  count: number;
}

/**
 * Recent-change counters behind the KPI cards (DashboardDeltasDto, ADR-0184).
 * There is no historical catalog counter, so catalog week-deltas are absent
 * rather than approximated.
 */
export interface DashboardDeltas {
  /** Work items that entered the pipeline since UTC midnight. */
  inboundToday: number;
  /** Work items that transitioned into `needs_review` in the last 24h. */
  newForReview24h: number;
}

/** Aggregated overview dashboard payload (DashboardDto, PRD-UI-001). */
export interface Dashboard {
  generatedAt: string;
  catalog: DashboardCatalog;
  workItems: DashboardWorkItems;
  attention: DashboardAttention;
  deltas?: DashboardDeltas;
  health: SystemHealth;
  activity: {
    completedPerHour: DashboardActivityBucket[];
    /** Work items that reached `indexed` per hour (ADR-0184 second series). */
    indexedPerHour?: DashboardActivityBucket[];
  };
}

/**
 * Library/listening/database statistics (StatsDto) for the Stats page. Every
 * aggregate is computed server-side over a bounded window; the client renders it
 * and never re-aggregates.
 */
export interface Stats {
  generatedAt: string;
  library: {
    artists: number;
    releases: number;
    tracks: number;
    mediaAssets: number;
    totalBytes: number;
    totalDurationMs: number;
    averageTrackDurationMs: number;
    losslessTracks: number;
    formats: { container: string; codec: string; tracks: number; bytes: number; lossless: boolean }[];
    decades: { decade: number; releases: number }[];
    largestReleases: { id: string; title: string; artist: string | null; tracks: number; bytes: number }[];
  };
  listening: {
    totalPlays: number;
    totalPlaytimeSeconds: number;
    uniqueTracks: number;
    uniqueListeners: number;
    playsPerDay: { day: string; plays: number; seconds: number }[];
    topArtists: { name: string; plays: number; seconds: number }[];
    topTracks: { id: string; title: string; artist: string | null; plays: number }[];
  };
  database: {
    totalBytes: number;
    tables: { name: string; bytes: number }[];
    workItems: number;
    playEvents: number;
    pendingProjection: number;
    poolCapacity: number;
    poolInUse: number;
  };
}

/**
 * One durable worker pool with queue metrics (WorkerPoolDto). When
 * `metricsAvailable` is false the pool's worker has no durable queue table
 * and every metric field is null — render honest absence, never zeros.
 * `running` is unexpired-lease evidence, not a worker liveness claim.
 */
export interface WorkerPool {
  name: string;
  paused: boolean;
  pausedBy: string | null;
  pausedAt: string | null;
  resumeRequestedAt: string | null;
  updatedAt: string;
  metricsAvailable: boolean;
  queued: number | null;
  running: number | null;
  oldestQueuedAgeSeconds: number | null;
  completedLastHour: number | null;
  throughputPerMinute: number | null;
  failedLast24h: number | null;
}

export interface JobRunHistory {
  id: string;
  jobName: string;
  scheduledTickAt: string | null;
  startedAt: string;
  completedAt: string | null;
  durationMs: number | null;
  success: boolean | null;
  errorMessage: string | null;
  wasManualTrigger: boolean;
  triggeredBy: string | null;
  status?: 'running' | 'completed' | 'failed' | 'stale';
}

export interface SearchProjection {
  pending: number;
  failed: number;
  leased: number;
  oldestAvailableAt: string | null;
  lastProcessedOutboxId: number;
  activeGenerationId: string | null;
  latestRebuildStatus: string | null;
  latestRebuildError: string | null;
}

export interface SearchGeneration {
  artists: number;
  releases: number;
  tracks: number;
  total: number;
  schemaVersion: number;
}

/** Search projection/generation status (SearchStatusDto). When `enabled` is
 *  false every other field is null. */
export interface SearchStatus {
  enabled: boolean;
  projection: SearchProjection | null;
  generation: SearchGeneration | null;
  generationErrorCode: string | null;
  generationError: string | null;
}

/** Durable rebuild request acknowledgement (SearchRebuildDto, HTTP 202). */
export interface SearchRebuild {
  jobId: string;
  status: string;
  requestedBy: string;
}

// ---- Catalog browsing + detail (apps/melodee-server/src/admin_catalog.rs) ----
// Shapes mirror the admin catalog DTOs (camelCase serde). Read-only views
// over the catalog schema; public ids are the operator-safe keys.

/** Bounded detail section: up to 100 items plus the full section total. */
export interface Section<T> {
  items: T[];
  total: number;
}

/** Artist catalog status (artists_status_check, ADR-0190). */
export type CatalogArtistStatus = 'active' | 'verified' | 'merged' | 'deprecated';

/** Release catalog status (releases_status_check). */
export type CatalogReleaseStatus = 'active' | 'deprecated' | 'deleted' | 'merged';

/** Policy/workflow context behind the catalog visibility projection. */
export type ReleaseWorkflowState =
  | 'cataloged'
  | 'indexing'
  | 'indexed'
  | 'needs_review'
  | 'quarantined'
  | 'ignored'
  | 'deleted'
  | string;

/** Artist type taxonomy (artists_type_check). */
export type CatalogArtistType =
  | 'person'
  | 'group'
  | 'orchestra'
  | 'ensemble'
  | 'choir'
  | 'character'
  | 'other'
  | 'unknown';

/** GET /catalog/artists row (ArtistListItemDto). */
export interface CatalogArtistListItem {
  id: string;
  name: string;
  sortName: string;
  artistType: CatalogArtistType;
  disambiguation: string | null;
  status: CatalogArtistStatus;
  releaseCount: number;
  trackCount: number;
  /** Consumable by the frozen native `/images/{id}/{size}` endpoint. */
  imageId: string;
  createdAt: string;
  updatedAt: string;
}

/** GET /catalog/releases row (ReleaseListItemDto). */
export interface CatalogReleaseListItem {
  id: string;
  title: string;
  displayArtist: string | null;
  /** Extended ISO date, or null when only `releaseDateText` is known. */
  releaseDate: string | null;
  releaseDateText: string | null;
  status: CatalogReleaseStatus;
  workflowState: ReleaseWorkflowState | null;
  policyDisposition: PolicyDisposition | null;
  policyEvidenceSummary: string | null;
  trackCount: number;
  durationMs: number;
  imageId: string;
  createdAt: string;
  updatedAt: string;
}

/** GET /catalog/tracks row (TrackListItemDto). */
export interface CatalogTrackListItem {
  id: string;
  title: string;
  displayArtist: string | null;
  ordinal: number;
  discNumber: number | null;
  trackNumber: number | null;
  durationMs: number | null;
  releaseId: string;
  releaseTitle: string;
  imageId: string;
  createdAt: string;
}

/** Typed artist alias (ArtistAliasDto); confidence is basis points 0–10000. */
export interface ArtistAlias {
  alias: string;
  aliasType: string;
  locale: string | null;
  evidenceSource: string;
  confidence: number;
  locked: boolean;
  createdAt: string;
}

/** Artist external identity (ArtistExternalIdDto). */
export interface ArtistExternalId {
  provider: string;
  externalId: string;
  confidence: number;
  evidenceSource: string;
  verifiedAt: string | null;
  locked: boolean;
}

/** A Release the Artist is billed on (ArtistReleaseDto). */
export interface ArtistRelease {
  releaseId: string;
  title: string;
  role: string;
  presentationOrder: number;
  displayValue: string;
  releaseDate: string | null;
  trackCount: number;
}

/** One provider-found identity candidate for an Artist (enrichment evidence). */
export interface ArtistProviderCandidate {
  provider: string;
  externalId: string;
  query: string;
  candidateRank: number;
  /** Basis points (0–10000). */
  confidence: number;
  observedAt: string;
}

/**
 * One Track credit on an Artist (ArtistTrackCreditDto). This is the drill-down
 * behind the Artist statistics: which Track, on which Release, in which role.
 */
export interface ArtistTrackCredit {
  trackId: string;
  trackTitle: string;
  releaseId: string;
  releaseTitle: string;
  role: string | null;
  isPerformance: boolean;
  associationType: string;
  displayValue: string | null;
  joinPhrase: string | null;
  presentationOrder: number | null;
}

/** One credit row (ArtistContributionDto): release-level when trackId is null. */
export interface ArtistContribution {
  role: string;
  roleDetail: string | null;
  isPerformance: boolean;
  evidenceSource: string;
  releaseId: string;
  releaseTitle: string;
  trackId: string | null;
  trackTitle: string | null;
}

/** Artist-to-artist relationship (ArtistRelationshipDto). */
export interface ArtistRelationship {
  sourceArtistId: string;
  sourceName: string;
  targetArtistId: string;
  targetName: string;
  relationshipType: string;
  role: string | null;
  startDate: string | null;
  endDate: string | null;
  confidence: number;
  evidenceSource: string;
  locked: boolean;
}

/** Group membership (ArtistMembershipDto). */
export interface ArtistMembership {
  memberArtistId: string;
  memberName: string;
  groupArtistId: string;
  groupName: string;
  role: string;
  startDate: string | null;
  endDate: string | null;
  confidence: number;
  evidenceSource: string;
  locked: boolean;
}

/** Bounded catalog audit event (CatalogAuditEventDto). */
export interface CatalogAuditEvent {
  eventId: string;
  eventType: string;
  actor: string;
  reason: string;
  payload: unknown;
  createdAt: string;
}

/** Set-based catalog stats for an Artist (ArtistStatsDto). */
export interface ArtistStats {
  releaseCount: number;
  billedTrackCount: number;
  performanceTrackCount: number;
  allCreditTrackCount: number;
}

/** Full Artist detail (ArtistDetailDto). */
export interface ArtistDetail {
  id: string;
  name: string;
  sortName: string;
  artistType: CatalogArtistType;
  disambiguation: string | null;
  status: CatalogArtistStatus;
  beginDate: string | null;
  endDate: string | null;
  /** Merge target public id; non-null exactly when status is `merged`. */
  mergedIntoArtistId: string | null;
  revision: number;
  imageId: string;
  createdAt: string;
  updatedAt: string;
  stats: ArtistStats;
  aliases: Section<ArtistAlias>;
  externalIds: Section<ArtistExternalId>;
  releases: Section<ArtistRelease>;
  contributions: Section<ArtistContribution>;
  /** Track credits backing `stats` (the drill-down for the Artist numbers). */
  trackCredits: Section<ArtistTrackCredit>;
  /**
   * Provider identity candidates recorded by enrichment. Evidence only — these
   * are not external identities and never authorize a merge on their own.
   */
  providerCandidates: Section<ArtistProviderCandidate>;
  relationships: Section<ArtistRelationship>;
  memberships: Section<ArtistMembership>;
  history: Section<CatalogAuditEvent>;
}

/** Ordered Track row on a Release detail (ReleaseTrackDto). */
export interface ReleaseTrack {
  id: string;
  ordinal: number;
  title: string;
  displayArtist: string | null;
  discNumber: number | null;
  trackNumber: number | null;
  durationMs: number | null;
  sourcePath: string | null;
  container: string | null;
  codec: string | null;
  byteSize: number | null;
  mimeType: string | null;
  /**
   * Signed audio preview URL on the native `/song/stream/…` route (same-origin
   * when the admin UI is served by the API); null when the Track has no
   * playable asset or the acting administrator is not stream-authorized.
   */
  streamUrl: string | null;
}

/** Billed artist with role and join phrase (BilledArtistDto). */
export interface BilledArtist {
  artistId: string;
  name: string;
  status: CatalogArtistStatus;
  role: string;
  presentationOrder: number;
  displayValue: string;
  joinPhrase: string;
}

/** Credit row on a Release/Track detail (ContributionDto). */
export interface CatalogContribution {
  artistId: string;
  artistName: string;
  role: string;
  roleDetail: string | null;
  isPerformance: boolean;
  evidenceSource: string;
  displayValue: string | null;
  presentationOrder: number;
  trackId: string | null;
  trackTitle: string | null;
}

/** Stored media file facts (MediaAssetDto). */
export interface MediaAsset {
  id: string;
  canonicalRelativePath: string;
  byteSize: number;
  mimeType: string;
  container: string | null;
  codec: string | null;
  sampleRateHz: number | null;
  channels: number | null;
  bitrateBps: number | null;
  durationMs: number | null;
}

/** Track media asset with its link purpose (TrackMediaAssetDto, flattened). */
export interface TrackMediaAsset extends MediaAsset {
  purpose: string;
}

/** Import provenance for a cataloged Release (ReleaseProvenanceDto). */
export interface ReleaseProvenance {
  workItemId: string;
  manifestId: string;
  manifestSchemaVersion: number;
  semanticManifestHash: string;
  publicationPlanHash: string;
  sealedManifestHash: string;
  resolverVersion: string;
  importedAt: string;
}

/** Persisted classification (ReleaseClassificationDto). */
export interface ReleaseClassification {
  traits: string[];
  /** Basis points (0–10000). */
  confidence: number;
  classifiedAt: string;
  classifierVersion: string;
}

/** Full Release detail (ReleaseDetailDto). */
export interface ReleaseDetail {
  id: string;
  title: string;
  sortTitle: string;
  displayArtist: string | null;
  releaseDate: string | null;
  releaseDateText: string | null;
  status: CatalogReleaseStatus;
  workflowState: ReleaseWorkflowState | null;
  policyDisposition: PolicyDisposition | null;
  policyEvidenceSummary: string | null;
  revision: number;
  trackCount: number;
  durationMs: number;
  mediaBytes: number;
  imageId: string;
  createdAt: string;
  updatedAt: string;
  tracks: Section<ReleaseTrack>;
  artists: BilledArtist[];
  contributors: Section<CatalogContribution>;
  mediaAssets: Section<MediaAsset>;
  /** Null until policy classification has run (traits). */
  classification: ReleaseClassification | null;
  /** Null for Releases that predate manifest-based import. */
  provenance: ReleaseProvenance | null;
  history: Section<CatalogAuditEvent>;
}

// ---- Policies / rule editor ----

/** Policy outcome for a Release (Disposition::as_str, SPEC §15). */
export type PolicyDisposition = 'accept' | 'review' | 'ignore' | 'quarantine' | 'delete';

/** Engine rule kinds (PolicyRuleType::as_str in melodee-domain). */
export type PolicyRuleType =
  | 'greatest_hits_redundancy'
  | 'greatest_hits_novelty'
  | 'greatest_hits_uncertain'
  | 'corrupt_media'
  | 'administrator_override'
  | 'unknown_classification'
  | 'trait_disposition';

/** Release trait values (ReleaseTrait::as_str in melodee-domain). */
export type ReleaseTraitValue =
  | 'studio'
  | 'ep'
  | 'single'
  | 'live'
  | 'compilation'
  | 'greatest_hits'
  | 'collection'
  | 'box_set'
  | 'bootleg'
  | 'demo'
  | 'promo'
  | 'broadcast'
  | 'radio_show'
  | 'dj_mix'
  | 'remix'
  | 'tribute'
  | 'soundtrack'
  | 'cast_recording'
  | 'spoken_word'
  | 'comedy'
  | 'audiobook'
  | 'reissue'
  | 'remaster'
  | 'other'
  | 'unknown';

/**
 * Engine predicate schema (RulePredicate in melodee-policy). The deserializer
 * denies unknown fields, so only these two snake_case keys are ever sent:
 * `required_traits` (every listed trait must be present on the Release) and
 * `existing_dispositions` (when present, the Release's existing disposition
 * must be one of the listed values).
 */
export interface PolicyPredicate {
  required_traits?: ReleaseTraitValue[];
  existing_dispositions?: PolicyDisposition[];
}

/** One rule as the operator sees it (PolicyRuleDto). */
export interface PolicyRule {
  id: string;
  ruleType: PolicyRuleType;
  disposition: PolicyDisposition;
  /** Confidence floor in basis points (0–10000); required for delete. */
  confidenceThreshold: number | null;
  predicate: PolicyPredicate;
  /** Zero-based evaluation position; null for disabled rules. */
  priority: number | null;
  enabled: boolean;
  /** Durable policy decisions this rule produced. */
  matchCount: number;
}

/** Immutable rule-set version metadata (PolicyRuleSetDto). */
export interface PolicyRuleSet {
  id: string;
  name: string;
  version: number;
  createdAt: string;
  /** PolicyActor JSON; shape is a domain detail, rendered generically. */
  createdBy: unknown;
}

/** GET /policies/rules envelope (PolicyRulePageDto). */
export interface PolicyRulePage {
  ruleSet: PolicyRuleSet;
  items: PolicyRule[];
  total: number;
  limit: number;
  offset: number;
}

/** Rule definition accepted by create/update/test (RuleInput). */
export interface PolicyRuleInput {
  id: string;
  ruleType: PolicyRuleType;
  disposition: PolicyDisposition;
  confidenceThreshold?: number | null;
  predicate: PolicyPredicate;
}

/** Result of a rule mutation: the rule plus the new immutable version. */
export interface PolicyRuleMutation {
  ruleSet: PolicyRuleSet;
  rule: PolicyRule;
  /** False when the request was an idempotent no-op. */
  changed: boolean;
}

export interface PolicyTestSample {
  /** Internal Release identity (not the catalog public id). */
  releaseId: string;
  title: string;
  artist: string | null;
}

/**
 * Test-run request: exactly one of `ruleId` (stored rule of the active set)
 * or `rule` (ad-hoc definition, never persisted).
 */
export interface PolicyTestRequest {
  ruleId?: string;
  rule?: PolicyRuleInput;
  /** Maximum Releases to evaluate; clamped server-side to 500. */
  limit?: number;
}

/** Read-only dry-run result over a bounded catalog snapshot. */
export interface PolicyTestRun {
  /** Null when the run could not be persisted (ad-hoc rule candidate). */
  testRunId: string | null;
  persisted: boolean;
  ruleSetId: string;
  ruleSetVersion: number;
  evaluated: number;
  matchedCount: number;
  matchedRuleCounts: Record<string, number>;
  proposedDispositionCounts: Record<string, number>;
  existingDispositionCounts: Record<string, number>;
  /** Releases whose proposed disposition is destructive (delete). */
  destructiveImpact: number;
  /** Whether the tested rule's action is destructive-class. */
  destructive: boolean;
  sample: PolicyTestSample[];
}

/** Full Track detail (TrackDetailDto). */
export interface TrackDetail {
  id: string;
  title: string;
  sortTitle: string;
  displayArtist: string | null;
  ordinal: number;
  discNumber: number | null;
  trackNumber: number | null;
  durationMs: number | null;
  sourcePath: string | null;
  audioIdentityId: string;
  revision: number;
  imageId: string;
  /**
   * Signed audio preview URL on the native `/song/stream/…` route (same-origin
   * when the admin UI is served by the API); null when the Track has no
   * playable asset or the acting administrator is not stream-authorized.
   */
  streamUrl: string | null;
  createdAt: string;
  updatedAt: string;
  releaseId: string;
  releaseTitle: string;
  artists: BilledArtist[];
  contributors: Section<CatalogContribution>;
  mediaAssets: TrackMediaAsset[];
}

// ---- Native theme packs (PRD-THEME-004; apps/melodee-server/src/native/discovery.rs) ----

/** ThemeInfo DTO from `GET /api/v1/themes` (camelCase serde rename). */
export interface ThemePackInfo {
  id: string;
  name: string;
  author: string | null;
  version: string | null;
  description: string | null;
  isBuiltIn: boolean;
  previewImage: string | null;
  hasWarnings: boolean;
  warningDetails: string[] | null;
}

/** Response of `POST /api/v1/themes/import`. */
export interface ThemeImportResult {
  themeId: string;
  message: string;
}
