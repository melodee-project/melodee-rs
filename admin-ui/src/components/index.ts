/**
 * The §40 PRD shared component set (UI-SPEC §6) plus the §6.1 Melodee surface
 * and primitive layer (ADR-0184). Screens must render these concepts through
 * this barrel, never through one-off implementations.
 */
export { AppShell } from '../shell/AppShell';
export { PageHeader } from './PageHeader';
export { Surface } from './Surface';
export { HeroCard, type HeroFact } from './HeroCard';
export { MetricCard, type MetricDeltaTone } from './MetricCard';
export { StatStrip, type StatStripItem } from './StatStrip';
export { StatusChip, StatusTag, type StatusKind, type StatusChipProps } from './StatusTag';
export { EntityAvatar } from './EntityAvatar';
export { ArtistLink, ReleaseLink, TrackLink } from './EntityLink';
export { ProcessingStateTag } from './ProcessingStateTag';
export { EvidenceItem, type Evidence } from './EvidenceItem';
export { EvidenceSummary } from './EvidenceSummary';
export { EvidenceTileGrid, type EvidenceTile, type EvidenceTileTone } from './EvidenceTileGrid';
export { TimelineList, type TimelineEvent, type TimelineTone } from './TimelineList';
export { DecisionActionList, type DecisionAction } from './DecisionActionList';
export { DecisionPanel, type DecisionChoice } from './DecisionPanel';
export { TabNav, type TabNavItem } from './TabNav';
export { ProgressBar } from './ProgressBar';
export { FilterPills, type FilterPillItem } from './FilterPills';
export { KeyValueList, type KeyValueItem } from './KeyValueList';
export { InspectorPanel } from './InspectorPanel';
export { EntityTable, type EntityColumn, type EntityRow } from './EntityTable';
export { StackedCell, AbsentCell } from './cells';
export { EmptyState } from './EmptyState';
export { HealthIndicator, HealthDot, type HealthState, type HealthTone } from './HealthIndicator';
export { SystemHealthPanel } from './SystemHealthPanel';
export { AuditTimeline, type AuditEvent } from './AuditTimeline';
export { AudioPreview } from './AudioPreview';
export { DestructiveActionDialog } from './DestructiveActionDialog';
