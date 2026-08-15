/**
 * The §40 PRD shared component set (UI-SPEC §6). Screens must render these
 * concepts through this barrel, never through one-off implementations.
 */
export { AppShell } from '../shell/AppShell';
export { PageHeader } from './PageHeader';
export { MetricCard } from './MetricCard';
export { StatusTag, type StatusKind } from './StatusTag';
export { EntityAvatar } from './EntityAvatar';
export { ArtistLink, ReleaseLink, TrackLink } from './EntityLink';
export { ProcessingStateTag } from './ProcessingStateTag';
export { EvidenceItem, type Evidence } from './EvidenceItem';
export { EvidenceSummary } from './EvidenceSummary';
export { DecisionPanel, type DecisionChoice } from './DecisionPanel';
export { InspectorPanel } from './InspectorPanel';
export { EntityTable, type EntityColumn, type EntityRow } from './EntityTable';
export { EmptyState } from './EmptyState';
export { HealthIndicator, type HealthState } from './HealthIndicator';
export { SystemHealthPanel } from './SystemHealthPanel';
export { AuditTimeline, type AuditEvent } from './AuditTimeline';
export { AudioPreview } from './AudioPreview';
export { DestructiveActionDialog } from './DestructiveActionDialog';
