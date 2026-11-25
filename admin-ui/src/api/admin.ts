import type { ReviewCorrection, ReviewMatch } from './types';
import { apiFetch, buildQuery } from './client';
import type {
  AccessGroup,
  Stats,
  ProcessingStageHealth,
  AccessGroupDetail,
  AdminScript,
  ArtistMergePreview,
  ArtistMetadataPatch,
  ArtistDetail,
  CatalogArtistListItem,
  CatalogReleaseListItem,
  CatalogTrackListItem,
  BatchMergeArtistsResult,
  BatchMergeReleasesResult,
  ArtistDeletePreview,
  ArtistDeleteResult,
  CatalogDeleteMode,
  ReleaseDeletePreview,
  ReleaseDeleteResult,
  ConfigChange,
  ConfigEntry,
  ConfigExport,
  ConfigImportResult,
  Dashboard,
  DiagnosticsBundle,
  DeviceProfile,
  IdentityDecisionEntry,
  IdentityResolution,
  Lease,
  JobRunHistory,
  ListParams,
  MergeArtistsResult,
  MetadataTransformAudit,
  MetadataTransformPreview,
  MetadataTransformResult,
  NeverMatchResult,
  NewArtistInput,
  NotificationDelivery,
  NotificationTemplate,
  ObservationDetail,
  OnboardingStatus,
  Page,
  PolicyRuleInput,
  PolicyRuleMutation,
  PolicyRulePage,
  PolicyTestRequest,
  PolicyTestRun,
  RadioStation,
  ReleaseMergePreview,
  ReleaseMetadataPatch,
  ReleaseDetail,
  ResolveObservationResult,
  ReviewDecisionResult,
  ReviewDetail,
  ReviewQueuePage,
  ScriptListResponse,
  ScriptRun,
  ScriptTestRunResult,
  SearchRebuild,
  SearchStatus,
  SplitArtistResult,
  SplitAssignmentInput,
  SystemHealth,
  SystemInfo,
  TrackDetail,
  TrackMetadataPatch,
  UnresolvedObservation,
  UserCreateRequest,
  UserSummary,
  PurgeWorkItemsResult,
  UserUpdateRequest,
  WorkItem,
  WorkItemEvent,
  WorkItemStateCount,
  WorkerPool,
} from './types';
export type {
  AccessGroup,
  AccessGroupDetail,
  AdminScript,
  ArtistMetadataPatch,
  ArtistDetail,
  CatalogArtistListItem,
  CatalogReleaseListItem,
  CatalogTrackListItem,
  ConfigExport,
  ConfigImportResult,
  Dashboard,
  DiagnosticsBundle,
  DeviceProfile,
  IdentityDecisionEntry,
  IdentityResolution,
  Lease,
  JobRunHistory,
  ListParams,
  MergeArtistsResult,
  MetadataTransformAudit,
  MetadataTransformPreview,
  MetadataTransformResult,
  NeverMatchResult,
  NewArtistInput,
  NotificationDelivery,
  NotificationTemplate,
  ObservationDetail,
  OnboardingStatus,
  Page,
  PolicyRuleInput,
  PolicyRuleMutation,
  PolicyRulePage,
  PolicyTestRequest,
  PolicyTestRun,
  RadioStation,
  ReleaseMergePreview,
  ReleaseMetadataPatch,
  ReleaseDetail,
  ResolveObservationResult,
  ReviewDecisionResult,
  ReviewDetail,
  ReviewQueuePage,
  ScriptListResponse,
  ScriptRun,
  ScriptTestRunResult,
  SearchRebuild,
  SearchStatus,
  SplitArtistResult,
  SplitAssignmentInput,
  SystemHealth,
  SystemInfo,
  TrackDetail,
  TrackMetadataPatch,
  UnresolvedObservation,
  UserCreateRequest,
  UserSummary,
  UserUpdateRequest,
  WorkItem,
  WorkItemEvent,
  WorkItemStateCount,
  WorkerPool,
} from './types';

/**
 * Typed functions for the documented admin endpoint groups
 * (docs/api/admin-api.md). Paths are relative and same-origin.
 */

const ADMIN = '/api/admin/v1';

// ---- System ----

export function getSystemInfo(options?: { suppressAuthEvent?: boolean }): Promise<SystemInfo> {
  return apiFetch<SystemInfo>(`${ADMIN}/system/info`, {
    suppressAuthEvent: options?.suppressAuthEvent ?? false,
  });
}

/** Aggregated subsystem health (SPEC §22). */
export function getSystemHealth(): Promise<SystemHealth> {
  return apiFetch<SystemHealth>(`${ADMIN}/system/health`);
}

// ---- Overview dashboard ----

/** Aggregated KPIs, attention counts, health, and per-hour activity. */
export function getDashboard(): Promise<Dashboard> {
  return apiFetch<Dashboard>(`${ADMIN}/dashboard`);
}

/** Library, listening, and database statistics (Stats page). */
export function getStats(): Promise<Stats> {
  return apiFetch<Stats>(`${ADMIN}/stats`);
}

// ---- Worker pools and search administration ----

/** Durable pool table with queue metrics; the backend returns every pool. */
export function listWorkerPools(): Promise<Page<WorkerPool>> {
  return apiFetch<Page<WorkerPool>>(`${ADMIN}/workers/pools`);
}

/**
 * Lease-coordinated durable pool control (the same control plane as
 * `melodee worker-pools pause|resume`). No request body; the acting admin is
 * the durable audit recorded server-side.
 */
export function pauseWorkerPool(name: string): Promise<WorkerPool> {
  return apiFetch<WorkerPool>(
    `${ADMIN}/workers/pools/${encodeURIComponent(name)}/pause`,
    { method: 'POST' },
  );
}

export function resumeWorkerPool(name: string): Promise<WorkerPool> {
  return apiFetch<WorkerPool>(
    `${ADMIN}/workers/pools/${encodeURIComponent(name)}/resume`,
    { method: 'POST' },
  );
}

/** Recent failed work items; bounded, paginated, whitelisted sort. */
export function listWorkerFailures(params: ListParams = {}): Promise<Page<WorkItem>> {
  return apiFetch<Page<WorkItem>>(`${ADMIN}/workers/failures${buildQuery({ ...params })}`);
}

export function listJobHistory(params: { jobName?: string; limit?: number; offset?: number } = {}): Promise<Page<JobRunHistory>> {
  return apiFetch<Page<JobRunHistory>>(`${ADMIN}/jobs/history${buildQuery(params)}`);
}

export function runJobNow(name: string): Promise<JobRunHistory> {
  return apiFetch<JobRunHistory>(`${ADMIN}/jobs/${encodeURIComponent(name)}/run-now`, {
    method: 'POST',
  });
}

export function pauseJob(name: string): Promise<WorkerPool> {
  return apiFetch<WorkerPool>(`${ADMIN}/jobs/${encodeURIComponent(name)}/pause`, {
    method: 'POST',
  });
}

export function resumeJob(name: string): Promise<WorkerPool> {
  return apiFetch<WorkerPool>(`${ADMIN}/jobs/${encodeURIComponent(name)}/resume`, {
    method: 'POST',
  });
}

export function getSearchStatus(): Promise<SearchStatus> {
  return apiFetch<SearchStatus>(`${ADMIN}/search/status`);
}

/**
 * Durably requests a generational search rebuild (HTTP 202). The search
 * worker claims and executes the job in the background; 409
 * `rebuild_in_progress` while one is unfinished, 409 `search_disabled` when
 * the search worker is off.
 */
export function requestSearchRebuild(): Promise<SearchRebuild> {
  return apiFetch<SearchRebuild>(`${ADMIN}/search/rebuild`, { method: 'POST' });
}

// ---- Ingest work items ----

export interface WorkItemListParams extends ListParams {
  state?: string;
  /**
   * Operator-facing phase filter (ADR-0185). A phase is a state set; use `state`
   * for one exact state. Supplying both is a 400 `invalid_filter`.
   */
  phase?: string;
  /**
   * Case-insensitive substring over the bound Release title, its billed artist,
   * and the work item's current path (ADR-0184). Bounded operator convenience
   * over the active queue, not a catalog search.
   */
  query?: string;
}

export function listWorkItems(params: WorkItemListParams = {}): Promise<Page<WorkItem>> {
  return apiFetch<Page<WorkItem>>(`${ADMIN}/ingest/work-items${buildQuery({ ...params })}`);
}

/** Per-state counts; the endpoint returns only states that have items. */
/**
 * Redacted support bundle (GET /diagnostics/bundle). Only the uptime field is
 * consumed by the Admin UI today: the Workers screen shows real server uptime
 * instead of inventing one (ADR-0184).
 */
export function getDiagnosticsBundle(): Promise<DiagnosticsBundle> {
  return apiFetch<DiagnosticsBundle>(`${ADMIN}/diagnostics/bundle`);
}

export function getProcessingStages(): Promise<ProcessingStageHealth[]> {
  return apiFetch<ProcessingStageHealth[]>(`${ADMIN}/ingest/stages`);
}

export function getWorkItemCounts(): Promise<WorkItemStateCount[]> {
  return apiFetch<WorkItemStateCount[]>(`${ADMIN}/ingest/work-items/counts`);
}

export function getWorkItem(id: string): Promise<WorkItem> {
  return apiFetch<WorkItem>(`${ADMIN}/ingest/work-items/${encodeURIComponent(id)}`);
}

export function getWorkItemEvents(id: string): Promise<WorkItemEvent[]> {
  return apiFetch<WorkItemEvent[]>(
    `${ADMIN}/ingest/work-items/${encodeURIComponent(id)}/events`,
  );
}

/** Active leases; the endpoint returns a bare array (limit only, no paging). */
export function listLeases(params: Pick<ListParams, 'limit'> = {}): Promise<Lease[]> {
  return apiFetch<Lease[]>(`${ADMIN}/ingest/leases${buildQuery({ ...params })}`);
}

// ---- Release review ----

export interface ReviewQueueParams extends ListParams {
  /** Restricted server-side to needs_review | failed | quarantined. */
  state?: string;
}

export function listReviewQueue(params: ReviewQueueParams = {}): Promise<ReviewQueuePage> {
  return apiFetch<ReviewQueuePage>(`${ADMIN}/review/queue${buildQuery({ ...params })}`);
}

export function getReviewDetail(workItemId: string): Promise<ReviewDetail> {
  return apiFetch<ReviewDetail>(`${ADMIN}/review/${encodeURIComponent(workItemId)}`);
}

export interface ReviewDecisionRequest {
  action: 'accept' | 'quarantine' | 'ignore' | 'delete';
  reason?: string;
  /** Explicit destructive acknowledgement; required for `delete`. */
  override?: boolean;
}

export function postReviewDecision(
  workItemId: string,
  request: ReviewDecisionRequest,
): Promise<ReviewDecisionResult> {
  return apiFetch<ReviewDecisionResult>(
    `${ADMIN}/review/${encodeURIComponent(workItemId)}/decision`,
    { method: 'POST', body: request },
  );
}

export function postReviewReprocess(workItemId: string, reason?: string): Promise<WorkItem> {
  return apiFetch<WorkItem>(`${ADMIN}/review/${encodeURIComponent(workItemId)}/reprocess`, {
    method: 'POST',
    body: { reason: reason ?? null },
  });
}

/**
 * Same-origin, cookie-authenticated preview stream URL for the <audio>
 * element (Range-capable server-side). 404 `preview_unavailable` is a normal
 * outcome when no previewable file exists under the configured roots.
 */
export function reviewTrackPreviewUrl(workItemId: string, trackIndex: number): string {
  return `${ADMIN}/review/${encodeURIComponent(workItemId)}/tracks/${trackIndex}/preview`;
}

// ---- Artist identity review ----

export function listUnresolvedObservations(
  params: ListParams = {},
): Promise<Page<UnresolvedObservation>> {
  return apiFetch<Page<UnresolvedObservation>>(
    `${ADMIN}/identity/unresolved${buildQuery({ ...params })}`,
  );
}

export function getObservationDetail(observationId: string): Promise<ObservationDetail> {
  return apiFetch<ObservationDetail>(
    `${ADMIN}/identity/observations/${encodeURIComponent(observationId)}`,
  );
}

export interface ResolveObservationRequest {
  resolution: IdentityResolution;
  /** Artist public id; required for `confirm`, forbidden otherwise. */
  artistId?: string;
  reason: string;
}

export function resolveObservation(
  observationId: string,
  request: ResolveObservationRequest,
): Promise<ResolveObservationResult> {
  return apiFetch<ResolveObservationResult>(
    `${ADMIN}/identity/observations/${encodeURIComponent(observationId)}/resolve`,
    { method: 'POST', body: request },
  );
}

export function neverMatchCandidate(
  observationId: string,
  request: { artistId: string; reason: string },
): Promise<NeverMatchResult> {
  return apiFetch<NeverMatchResult>(
    `${ADMIN}/identity/observations/${encodeURIComponent(observationId)}/never-match`,
    { method: 'POST', body: request },
  );
}

export function listIdentityDecisions(
  params: ListParams = {},
): Promise<Page<IdentityDecisionEntry>> {
  return apiFetch<Page<IdentityDecisionEntry>>(
    `${ADMIN}/identity/decisions${buildQuery({ ...params })}`,
  );
}

export function mergeArtists(request: {
  sourceArtistId: string;
  targetArtistId: string;
  reason: string;
}): Promise<MergeArtistsResult> {
  return apiFetch<MergeArtistsResult>(`${ADMIN}/identity/artists/merge`, {
    method: 'POST',
    body: request,
  });
}

export function previewArtistMerge(request: {
  sourceArtistIds: string[];
  targetArtistId: string;
}): Promise<ArtistMergePreview> {
  return apiFetch<ArtistMergePreview>(`${ADMIN}/identity/artists/merge/preview`, {
    method: 'POST',
    body: request,
  });
}

export function batchMergeArtists(request: {
  sourceArtistIds: string[];
  targetArtistId: string;
  reason: string;
}): Promise<BatchMergeArtistsResult> {
  return apiFetch<BatchMergeArtistsResult>(`${ADMIN}/identity/artists/merge/batch`, {
    method: 'POST',
    body: request,
  });
}

/** Artist deletion impact preview (ADR-0199); read-only. */
export function previewArtistDelete(artistIds: string[]): Promise<ArtistDeletePreview> {
  return apiFetch<ArtistDeletePreview>(`${ADMIN}/catalog/artists/delete/preview`, {
    method: 'POST',
    body: { artistIds },
  });
}

export function deleteArtists(request: {
  artistIds: string[];
  mode: CatalogDeleteMode;
  reason: string;
}): Promise<ArtistDeleteResult> {
  return apiFetch<ArtistDeleteResult>(`${ADMIN}/catalog/artists/delete`, {
    method: 'POST',
    body: request,
  });
}

/** Release deletion impact preview (ADR-0199); read-only. */
export function previewReleaseDelete(releaseIds: string[]): Promise<ReleaseDeletePreview> {
  return apiFetch<ReleaseDeletePreview>(`${ADMIN}/catalog/releases/delete/preview`, {
    method: 'POST',
    body: { releaseIds },
  });
}

export function deleteReleases(request: {
  releaseIds: string[];
  mode: CatalogDeleteMode;
  reason: string;
}): Promise<ReleaseDeleteResult> {
  return apiFetch<ReleaseDeleteResult>(`${ADMIN}/catalog/releases/delete`, {
    method: 'POST',
    body: request,
  });
}

export function splitArtist(
  artistId: string,
  request: {
    reason: string;
    newArtist: NewArtistInput;
    assignments: SplitAssignmentInput[];
  },
): Promise<SplitArtistResult> {
  return apiFetch<SplitArtistResult>(
    `${ADMIN}/identity/artists/${encodeURIComponent(artistId)}/split`,
    { method: 'POST', body: request },
  );
}

// ---- Catalog browsing + detail ----

export interface CatalogListParams extends ListParams {
  /** Substring filter (server-side, escaped/validated by the backend). */
  query?: string;
  /** Releases only: restrict to Releases this Artist public id is billed on. */
  artistId?: string;
  /** Tracks only: restrict to Tracks of this Release public id. */
  releaseId?: string;
}

/** Server-side Artist browsing (envelope; sort whitelist name/createdAt/updatedAt). */
export function listCatalogArtists(
  params: CatalogListParams = {},
  signal?: AbortSignal,
): Promise<Page<CatalogArtistListItem>> {
  return apiFetch<Page<CatalogArtistListItem>>(
    `${ADMIN}/catalog/artists${buildQuery({ ...params })}`,
    { signal },
  );
}

/** Server-side Release browsing (sort whitelist title/releaseDate/createdAt/updatedAt). */
export function listCatalogReleases(
  params: CatalogListParams = {},
  signal?: AbortSignal,
): Promise<Page<CatalogReleaseListItem>> {
  return apiFetch<Page<CatalogReleaseListItem>>(
    `${ADMIN}/catalog/releases${buildQuery({ ...params })}`,
    { signal },
  );
}

/** Multi-source Release merge preview (ADR-0198): bounded, read-only impact. */
export function previewReleaseMerge(request: {
  sourceReleaseIds: string[];
  targetReleaseId: string;
}): Promise<ReleaseMergePreview> {
  return apiFetch<ReleaseMergePreview>(`${ADMIN}/catalog/releases/merge/preview`, {
    method: 'POST',
    body: request,
  });
}

/**
 * One-transaction multi-source Release merge: every selected source folds into
 * the target, or nothing changes. Requires a non-blank audit reason.
 */
export function batchMergeReleases(request: {
  sourceReleaseIds: string[];
  targetReleaseId: string;
  reason: string;
}): Promise<BatchMergeReleasesResult> {
  return apiFetch<BatchMergeReleasesResult>(`${ADMIN}/catalog/releases/merge/batch`, {
    method: 'POST',
    body: request,
  });
}

/** Server-side Track browsing (sort whitelist title/durationMs/createdAt). */
export function listCatalogTracks(
  params: CatalogListParams = {},
  signal?: AbortSignal,
): Promise<Page<CatalogTrackListItem>> {
  return apiFetch<Page<CatalogTrackListItem>>(
    `${ADMIN}/catalog/tracks${buildQuery({ ...params })}`,
    { signal },
  );
}

export function getCatalogArtist(id: string, signal?: AbortSignal): Promise<ArtistDetail> {
  return apiFetch<ArtistDetail>(`${ADMIN}/catalog/artists/${encodeURIComponent(id)}`, { signal });
}

export function getCatalogRelease(id: string, signal?: AbortSignal): Promise<ReleaseDetail> {
  return apiFetch<ReleaseDetail>(`${ADMIN}/catalog/releases/${encodeURIComponent(id)}`, { signal });
}

export function getCatalogTrack(id: string, signal?: AbortSignal): Promise<TrackDetail> {
  return apiFetch<TrackDetail>(`${ADMIN}/catalog/tracks/${encodeURIComponent(id)}`, { signal });
}

// ---- Catalog artwork overrides (ADR-0143) ----

/** Client-side pre-validation bounds mirroring the server pipeline. */
export const CATALOG_ARTWORK_MAX_BYTES = 10 * 1024 * 1024;
const artworkTypes: Record<string, string> = {
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.png': 'image/png',
  '.webp': 'image/webp',
};
const artworkMimeTypes = [...new Set(Object.values(artworkTypes))];
export const CATALOG_ARTWORK_ACCEPT = [...artworkMimeTypes, ...Object.keys(artworkTypes)].join(',');

/** Some file pickers supply an empty MIME type. The server verifies the actual bytes. */
export function catalogArtworkType(file: File): string | undefined {
  if (file.type) {
    return artworkMimeTypes.includes(file.type) ? file.type : undefined;
  }
  return artworkTypes[file.name.slice(file.name.lastIndexOf('.')).toLowerCase()];
}

export interface CatalogImageMutation {
  message: string;
  imageId: string;
  updatedAt?: string | null;
}

/**
 * Uploads raw JPEG, PNG, or WebP artwork for one Artist/Release. The server
 * re-validates magic bytes, size, and dimensions (400 invalid_image).
 */
export function uploadCatalogImage(
  entity: 'artists' | 'releases',
  id: string,
  file: File,
): Promise<CatalogImageMutation> {
  return apiFetch<CatalogImageMutation>(
    `${ADMIN}/catalog/${entity}/${encodeURIComponent(id)}/image`,
    { method: 'PUT', rawBody: { data: file, contentType: catalogArtworkType(file) ?? 'application/octet-stream' } },
  );
}

/**
 * Removes the artwork override (idempotent 204; only the override row is
 * deleted — canonical media is never touched). The operator reason is
 * recorded in the audit event.
 */
export function removeCatalogImage(
  entity: 'artists' | 'releases',
  id: string,
  reason: string,
): Promise<void> {
  return apiFetch<void>(`${ADMIN}/catalog/${entity}/${encodeURIComponent(id)}/image`, {
    method: 'DELETE',
    body: { reason },
  });
}

/**
 * Artwork URL for a catalog `imageId` on the frozen native image endpoint
 * (GET /images/{id}/{size}, same-origin cookie auth). Named sizes per
 * apps/melodee-server/src/native/images.rs. Absent artwork returns an empty
 * 200 body; consumers must treat a failed decode as "no artwork".
 */
export function catalogImageUrl(
  imageId: string,
  size: 'thumbnail' | 'small' | 'medium' | 'large' = 'small',
): string {
  return `/images/${encodeURIComponent(imageId)}/${size}`;
}

// ---- Policy rules ----

/**
 * Rules of the active rule set plus disabled rules known from immutable
 * history, in evaluation order (enabled rules carry zero-based `priority`).
 */
export function listPolicyRules(): Promise<PolicyRulePage> {
  return apiFetch<PolicyRulePage>(`${ADMIN}/policies/rules`);
}

/** Appends a rule at the lowest priority as a new immutable version (201). */
export function createPolicyRule(request: PolicyRuleInput): Promise<PolicyRuleMutation> {
  return apiFetch<PolicyRuleMutation>(`${ADMIN}/policies/rules`, {
    method: 'POST',
    body: request,
  });
}

/** Replaces a rule definition in place within a new version. */
export function updatePolicyRule(
  id: string,
  request: PolicyRuleInput,
): Promise<PolicyRuleMutation> {
  return apiFetch<PolicyRuleMutation>(
    `${ADMIN}/policies/rules/${encodeURIComponent(id)}`,
    { method: 'PUT', body: request },
  );
}

/**
 * Removes a rule in a new version (history retains the definition). Safety
 * rules are protected server-side with 409 `protected_rule`.
 */
export function deletePolicyRule(id: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/policies/rules/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

/** Restores the most recent historical definition at the lowest priority. Idempotent. */
export function enablePolicyRule(id: string): Promise<PolicyRuleMutation> {
  return apiFetch<PolicyRuleMutation>(
    `${ADMIN}/policies/rules/${encodeURIComponent(id)}/enable`,
    { method: 'POST' },
  );
}

/** Removes the rule from a new version; the definition survives in history. Idempotent. */
export function disablePolicyRule(id: string): Promise<PolicyRuleMutation> {
  return apiFetch<PolicyRuleMutation>(
    `${ADMIN}/policies/rules/${encodeURIComponent(id)}/disable`,
    { method: 'POST' },
  );
}

/**
 * Read-only dry-run against a bounded catalog snapshot. Stored-rule runs are
 * persisted to `policy_test_runs`; ad-hoc `rule` runs return
 * `persisted: false` (the audit table requires a durably stored rule set).
 */
export function testPolicyRule(request: PolicyTestRequest): Promise<PolicyTestRun> {
  return apiFetch<PolicyTestRun>(`${ADMIN}/policies/rules/test`, {
    method: 'POST',
    body: request,
  });
}

// ---- Users ----

export interface UserListParams extends ListParams {
  query?: string;
}

export function listUsers(params: UserListParams = {}): Promise<Page<UserSummary>> {
  return apiFetch<Page<UserSummary>>(`${ADMIN}/users${buildQuery({ ...params })}`);
}

export function createUser(request: UserCreateRequest): Promise<UserSummary> {
  return apiFetch<UserSummary>(`${ADMIN}/users`, { method: 'POST', body: request });
}

export function updateUser(id: string, request: UserUpdateRequest): Promise<UserSummary> {
  return apiFetch<UserSummary>(`${ADMIN}/users/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: request,
  });
}

export function uploadUserAvatar(
  id: string,
  file: File,
): Promise<{ message: string; updatedAt?: string }> {
  return apiFetch<{ message: string; updatedAt?: string }>(
    `${ADMIN}/users/${encodeURIComponent(id)}/avatar`,
    { method: 'POST', rawBody: { data: file, contentType: file.type } },
  );
}

export function deleteUserAvatar(id: string): Promise<{ message: string }> {
  return apiFetch<{ message: string }>(`${ADMIN}/users/${encodeURIComponent(id)}/avatar`, {
    method: 'DELETE',
  });
}

export function setUserPassword(id: string, password: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/users/${encodeURIComponent(id)}/password`, {
    method: 'POST',
    body: { password },
  });
}

/**
 * Hard-deletes failed pre-catalog work items, their processing log, and the
 * owned storage trees. The server refuses cataloged items and requires the
 * explicit confirmation flag.
 */
export function purgeWorkItems(workItemIds: string[]): Promise<PurgeWorkItemsResult> {
  return apiFetch<PurgeWorkItemsResult>(`${ADMIN}/ingest/work-items/purge`, {
    method: 'POST',
    body: { workItemIds, confirm: true },
  });
}

// ---- Access groups ----

export function listGroups(): Promise<AccessGroup[]> {
  return apiFetch<AccessGroup[]>(`${ADMIN}/groups`);
}

export function createGroup(name: string): Promise<AccessGroup> {
  return apiFetch<AccessGroup>(`${ADMIN}/groups`, { method: 'POST', body: { name } });
}

export function getGroup(id: string): Promise<AccessGroupDetail> {
  return apiFetch<AccessGroupDetail>(`${ADMIN}/groups/${encodeURIComponent(id)}`);
}

export function updateGroup(id: string, name: string): Promise<AccessGroup> {
  return apiFetch<AccessGroup>(`${ADMIN}/groups/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: { name },
  });
}

export function deleteGroup(id: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/groups/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

export function addGroupMember(id: string, userId: string): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/groups/${encodeURIComponent(id)}/members/${encodeURIComponent(userId)}`,
    { method: 'PUT' },
  );
}

export function removeGroupMember(id: string, userId: string): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/groups/${encodeURIComponent(id)}/members/${encodeURIComponent(userId)}`,
    { method: 'DELETE' },
  );
}

export function bindGroupLibrary(id: string, libraryId: string): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/groups/${encodeURIComponent(id)}/libraries/${encodeURIComponent(libraryId)}`,
    { method: 'PUT' },
  );
}

export function unbindGroupLibrary(id: string, libraryId: string): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/groups/${encodeURIComponent(id)}/libraries/${encodeURIComponent(libraryId)}`,
    { method: 'DELETE' },
  );
}

// ---- Configuration export/import ----

export function listConfigEntries(): Promise<ConfigEntry[]> {
  return apiFetch<ConfigEntry[]>(`${ADMIN}/config/entries`);
}

export function updateConfigEntry(key: string, change: ConfigChange): Promise<void> {
  return apiFetch<void>(`${ADMIN}/config/entries/${encodeURIComponent(key)}`, { method: 'PUT', body: change });
}

export function exportConfig(): Promise<ConfigExport> {
  return apiFetch<ConfigExport>(`${ADMIN}/config/export`);
}

/**
 * Validates the document against the registry and returns the preview diff;
 * `preview` defaults to true server-side — an import never applies silently.
 * `document` is unknown so the server remains the validator of malformed
 * uploads (400 invalid_document / invalid_value / redacted_marker_rejected).
 */
export function importConfig(
  document: unknown,
  options: { preview?: boolean; allowUnknown?: boolean } = {},
): Promise<ConfigImportResult> {
  const query = buildQuery({
    preview: String(options.preview ?? true),
    allowUnknown: String(options.allowUnknown ?? false),
  });
  return apiFetch<ConfigImportResult>(`${ADMIN}/config/import${query}`, {
    method: 'POST',
    body: document,
  });
}

// ---- Onboarding ----

export function getOnboardingStatus(): Promise<OnboardingStatus> {
  return apiFetch<OnboardingStatus>(`${ADMIN}/onboarding/status`);
}

export function completeOnboarding(): Promise<OnboardingStatus> {
  return apiFetch<OnboardingStatus>(`${ADMIN}/onboarding/complete`, { method: 'POST' });
}

// ---- Notifications ----

/** Bounded array (no paging envelope; limit/offset clamped server-side). */
export function listNotificationTemplates(params: ListParams = {}): Promise<NotificationTemplate[]> {
  return apiFetch<NotificationTemplate[]>(
    `${ADMIN}/notifications/templates${buildQuery({ ...params })}`,
  );
}

export function getNotificationTemplate(
  templateKey: string,
  locale: string,
): Promise<NotificationTemplate> {
  return apiFetch<NotificationTemplate>(
    `${ADMIN}/notifications/templates/${encodeURIComponent(templateKey)}/${encodeURIComponent(locale)}`,
  );
}

/**
 * Full-replacement upsert (UpsertTemplateRequest): `null` clears the field
 * on the stored row.
 */
export function putNotificationTemplate(
  templateKey: string,
  locale: string,
  request: { subject: string | null; bodyText: string | null; bodyHtml: string | null },
): Promise<NotificationTemplate> {
  return apiFetch<NotificationTemplate>(
    `${ADMIN}/notifications/templates/${encodeURIComponent(templateKey)}/${encodeURIComponent(locale)}`,
    { method: 'PUT', body: request },
  );
}

export function deleteNotificationTemplate(templateKey: string, locale: string): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/notifications/templates/${encodeURIComponent(templateKey)}/${encodeURIComponent(locale)}`,
    { method: 'DELETE' },
  );
}

/** Bounded array of delivery audit rows (no message content is stored). */
export function listNotificationDeliveries(params: ListParams = {}): Promise<NotificationDelivery[]> {
  return apiFetch<NotificationDelivery[]>(
    `${ADMIN}/notifications/deliveries${buildQuery({ ...params })}`,
  );
}

export function testSendNotification(request: {
  templateKey: string;
  recipient: string;
  locale?: string;
}): Promise<NotificationDelivery> {
  return apiFetch<NotificationDelivery>(`${ADMIN}/notifications/test-send`, {
    method: 'POST',
    body: request,
  });
}

// ---- Scripts ----

/** Registered scripts plus the wired-event registry for this build. */
export function listScripts(): Promise<ScriptListResponse> {
  return apiFetch<ScriptListResponse>(`${ADMIN}/scripts`);
}

export function getScript(event: string, name: string): Promise<AdminScript> {
  return apiFetch<AdminScript>(
    `${ADMIN}/scripts/${encodeURIComponent(event)}/${encodeURIComponent(name)}`,
  );
}

export function getScriptById(id: string): Promise<AdminScript> {
  return apiFetch<AdminScript>(`${ADMIN}/scripts/id/${encodeURIComponent(id)}`);
}

/**
 * Full-replacement upsert keyed by (event, name). `enabled` is never
 * accepted here: a circuit-breaker-disabled script requires the explicit
 * enable route.
 */
export function upsertScript(
  event: string,
  name: string,
  request: { source: string; timeoutMs?: number; memoryBytes?: number },
): Promise<AdminScript> {
  return apiFetch<AdminScript>(
    `${ADMIN}/scripts/${encodeURIComponent(event)}/${encodeURIComponent(name)}`,
    { method: 'PUT', body: request },
  );
}

export function deleteScript(id: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/scripts/id/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

/** Bounded array of execution audit rows, optionally scoped to one script. */
export function listScriptRuns(params: ListParams & { scriptId?: string } = {}): Promise<ScriptRun[]> {
  return apiFetch<ScriptRun[]>(`${ADMIN}/scripts/runs${buildQuery({ ...params })}`);
}

/**
 * Operator test-run: a stored script (`scriptId`) or ad-hoc `source`
 * (exactly one) against an admin-supplied ctx mock, evaluated in the real
 * sandbox and recorded in the run audit.
 */
export function testRunScript(request: {
  event: string;
  scriptId?: string;
  source?: string;
  ctx: unknown;
  timeoutMs?: number;
}): Promise<ScriptTestRunResult> {
  return apiFetch<ScriptTestRunResult>(`${ADMIN}/scripts/test-run`, {
    method: 'POST',
    body: request,
  });
}

export function enableScript(id: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/scripts/id/${encodeURIComponent(id)}/enable`, {
    method: 'POST',
  });
}

export function disableScript(id: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/scripts/id/${encodeURIComponent(id)}/disable`, {
    method: 'POST',
  });
}

// ---- Internet radio stations ----

/** Bounded array (no paging envelope; limit clamped server-side). */
export function listRadioStations(params: ListParams = {}): Promise<RadioStation[]> {
  return apiFetch<RadioStation[]>(`${ADMIN}/radio/stations${buildQuery({ ...params })}`);
}

export function createRadioStation(request: {
  name: string;
  streamUrl: string;
  homePageUrl?: string;
}): Promise<RadioStation> {
  return apiFetch<RadioStation>(`${ADMIN}/radio/stations`, { method: 'POST', body: request });
}

export function updateRadioStation(
  id: string,
  request: { name?: string; streamUrl?: string; homePageUrl?: string | null },
): Promise<RadioStation> {
  return apiFetch<RadioStation>(`${ADMIN}/radio/stations/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: request,
  });
}

export function deleteRadioStation(id: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/radio/stations/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

// ---- Device profiles ----

/** Bounded array (no paging envelope; limit clamped server-side). */
export function listDeviceProfiles(params: ListParams = {}): Promise<DeviceProfile[]> {
  return apiFetch<DeviceProfile[]>(`${ADMIN}/device-profiles${buildQuery({ ...params })}`);
}

export function createDeviceProfile(request: {
  name: string;
  clientMatch?: Record<string, string>;
  profile?: Record<string, unknown>;
  isDefault?: boolean;
}): Promise<DeviceProfile> {
  return apiFetch<DeviceProfile>(`${ADMIN}/device-profiles`, { method: 'POST', body: request });
}

export function updateDeviceProfile(
  id: string,
  request: {
    name?: string;
    clientMatch?: Record<string, string>;
    profile?: Record<string, unknown>;
  },
): Promise<DeviceProfile> {
  return apiFetch<DeviceProfile>(`${ADMIN}/device-profiles/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: request,
  });
}

export function deleteDeviceProfile(id: string): Promise<void> {
  return apiFetch<void>(`${ADMIN}/device-profiles/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  });
}

/** Atomically clears any existing default and marks this profile default. */
export function setDefaultDeviceProfile(id: string): Promise<DeviceProfile> {
  return apiFetch<DeviceProfile>(`${ADMIN}/device-profiles/${encodeURIComponent(id)}/default`, {
    method: 'POST',
  });
}

// ---- Catalog metadata updates (PUT /api/admin/v1/catalog/{entity}/{id}/metadata) ----

export function updateReleaseMetadata(
  releaseId: string,
  patch: ReleaseMetadataPatch,
): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/catalog/releases/${encodeURIComponent(releaseId)}/metadata`,
    { method: 'PUT', body: patch },
  );
}

export function updateArtistMetadata(artistId: string, patch: ArtistMetadataPatch): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/catalog/artists/${encodeURIComponent(artistId)}/metadata`,
    { method: 'PUT', body: patch },
  );
}

export function updateTrackMetadata(trackId: string, patch: TrackMetadataPatch): Promise<void> {
  return apiFetch<void>(
    `${ADMIN}/catalog/tracks/${encodeURIComponent(trackId)}/metadata`,
    { method: 'PUT', body: patch },
  );
}

export function previewReleaseMetadataTransform(
  releaseId: string,
  operation: string,
  params: Record<string, unknown> = {},
): Promise<MetadataTransformPreview> {
  return apiFetch<MetadataTransformPreview>(
    `${ADMIN}/catalog/releases/${encodeURIComponent(releaseId)}/metadata-transforms/preview`,
    { method: 'POST', body: { operation, params } },
  );
}

export function applyReleaseMetadataTransform(
  releaseId: string,
  operation: string,
  previewHash: string,
  params: Record<string, unknown> = {},
): Promise<MetadataTransformResult> {
  return apiFetch<MetadataTransformResult>(
    `${ADMIN}/catalog/releases/${encodeURIComponent(releaseId)}/metadata-transforms/apply`,
    { method: 'POST', body: { operation, params, previewHash } },
  );
}

export function listReleaseMetadataAudits(releaseId: string): Promise<MetadataTransformAudit[]> {
  return apiFetch<MetadataTransformAudit[]>(
    `${ADMIN}/catalog/releases/${encodeURIComponent(releaseId)}/metadata-transforms`,
  );
}

export function revertReleaseMetadataTransform(
  releaseId: string,
  auditId: string,
): Promise<MetadataTransformResult> {
  return apiFetch<MetadataTransformResult>(
    `${ADMIN}/catalog/releases/${encodeURIComponent(releaseId)}/metadata-transforms/${encodeURIComponent(auditId)}/revert`,
    { method: 'POST' },
  );
}

export function reviewSources(id: string) {
  return apiFetch<{ id: string; name: string }[]>(`${ADMIN}/review/${encodeURIComponent(id)}/sources`);
}

export function searchReviewMatches(id: string, provider: string, artist: string, title: string) {
  const query = new URLSearchParams({ provider, artist, title });
  return apiFetch<{ matches: ReviewMatch[]; cacheKey: string | null }>(
    `${ADMIN}/review/${encodeURIComponent(id)}/matches?${query}`,
  );
}

export function saveReviewCorrection(id: string, body: ReviewCorrection) {
  return apiFetch<void>(`${ADMIN}/review/${encodeURIComponent(id)}/metadata`, { method: 'POST', body });
}
