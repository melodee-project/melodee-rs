import {
  ActionableNotification,
  Heading,
  Link,
  Loading,
  Tab,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
  TabList,
  TabPanel,
  TabPanels,
  Tabs,
  Tag,
} from '@carbon/react';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink, useParams } from 'react-router-dom';

import { getCatalogArtist } from '../../api/admin';
import type { ArtistContribution, ArtistDetail } from '../../api/types';
import { AuditTimeline } from '../../components/AuditTimeline';
import { EmptyState } from '../../components/EmptyState';
import { ArtistLink, ReleaseLink, TrackLink } from '../../components/EntityLink';
import { MetricCard } from '../../components/MetricCard';
import { PageHeader } from '../../components/PageHeader';
import { StatusTag } from '../../components/StatusTag';
import { formatDate, formatDateTime, formatNumber } from '../../i18n/format';
import { catalogStatusKind, toCatalogAuditEvent, useCatalogDetail } from './catalog';
import { ArtworkManager } from './ArtworkManager';
import { CatalogArtwork } from './CatalogArtwork';

/**
 * Artist Detail workspace (PRD-UI-006): identity facts and catalog
 * stats first, then tabbed sections over the bounded detail payload —
 * Overview (typed aliases), Releases, Appearances (performance credits),
 * Contributions (non-performance credits), Relationships (incl. group
 * memberships), Identity (merge target, external ids), Images (durable
 * artwork wiring), History (audit). Every tab renders an honest EmptyState
 * when its section is empty (UI-SPEC §7).
 */

/** Basis-points confidence as a locale percentage string. */
function confidencePercent(confidence: number): string {
  return `${formatNumber(Math.round(confidence / 100))}%`;
}

/** Localized date range with open ends ("1978 –", "– 2012"). */
function dateRangeLabel(start: string | null, end: string | null): string {
  if (!start && !end) {
    return '';
  }
  const from = start ? formatDate(start) : '';
  const to = end ? formatDate(end) : '';
  return `${from} – ${to}`.trim();
}

/** "Showing N of M" honesty note for bounded sections. */
function BoundedNote({ shown, total }: { shown: number; total: number }) {
  const { t } = useTranslation();
  if (total <= shown) {
    return null;
  }
  return (
    <p className="melodee-review-fact__label" style={{ marginBlockStart: '0.5rem' }}>
      {t('catalog.section.bounded', { shown: formatNumber(shown), total: formatNumber(total) })}
    </p>
  );
}

/** Appearances/Contributions share one table shape (ArtistContributionDto). */
function ContributionTable({
  caption,
  items,
  emptyKey,
}: {
  caption: string;
  items: ArtistContribution[];
  emptyKey: string;
}) {
  const { t } = useTranslation();
  if (items.length === 0) {
    return <EmptyState title={t(emptyKey)} compact />;
  }
  return (
    <div className="melodee-review-table-scroll">
      <TableContainer title={caption}>
        <Table size="sm" aria-label={caption}>
          <TableHead>
            <TableRow>
              <TableHeader>{t('catalog.column.role')}</TableHeader>
              <TableHeader>{t('catalog.column.release')}</TableHeader>
              <TableHeader>{t('catalog.column.track')}</TableHeader>
              <TableHeader>{t('catalog.column.evidence')}</TableHeader>
            </TableRow>
          </TableHead>
          <TableBody>
            {items.map((item, index) => (
              <TableRow key={`${item.releaseId}-${item.trackId ?? 'release'}-${index}`}>
                <TableCell>
                  {item.role}
                  {item.roleDetail ? (
                    <span className="melodee-review-track__file">{item.roleDetail}</span>
                  ) : null}
                </TableCell>
                <TableCell>
                  <ReleaseLink id={item.releaseId} name={item.releaseTitle} />
                </TableCell>
                <TableCell>
                  {item.trackId && item.trackTitle ? (
                    <TrackLink id={item.trackId} name={item.trackTitle} />
                  ) : (
                    t('catalog.contribution.releaseLevel')
                  )}
                </TableCell>
                <TableCell>{item.evidenceSource}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </TableContainer>
    </div>
  );
}

function ArtistHeaderFacts({ detail }: { detail: ArtistDetail }) {
  const { t } = useTranslation();
  const facts: { label: string; value: string }[] = [
    {
      label: t('catalog.artist.fact.type'),
      value: t(`catalog.artistType.${detail.artistType}`, { defaultValue: detail.artistType }),
    },
    ...(detail.beginDate
      ? [{ label: t('catalog.artist.fact.beginDate'), value: formatDate(detail.beginDate) }]
      : []),
    ...(detail.endDate
      ? [{ label: t('catalog.artist.fact.endDate'), value: formatDate(detail.endDate) }]
      : []),
  ];
  return (
    <div className="melodee-review-header__facts">
      {facts.map((fact) => (
        <div key={fact.label}>
          <div className="melodee-review-fact__label">{fact.label}</div>
          <div>{fact.value}</div>
        </div>
      ))}
    </div>
  );
}

export function ArtistDetailScreen() {
  const { t } = useTranslation();
  const { id = '' } = useParams();
  const { detail, error, loading, retry } = useCatalogDetail(getCatalogArtist, id);

  if (loading && detail === null) {
    return (
      <>
        <PageHeader title={t('catalog.artist.detailTitle')} subtitle={t('catalog.artist.detailSubtitle')} />
        <Loading description={t('common.loading')} withOverlay={false} />
      </>
    );
  }

  return (
    <div>
      <PageHeader title={t('catalog.artist.detailTitle')} subtitle={t('catalog.artist.detailSubtitle')} />
      <p>
        <Link as={RouterLink} to="/catalog/artists">
          {t('catalog.artist.back')}
        </Link>
      </p>
      {error ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('catalog.detail.loadErrorTitle')}
          subtitle={error.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={retry}
        />
      ) : null}
      {detail ? (
        <>
          <div className="melodee-review-card">
            <div className="melodee-review-header">
              <CatalogArtwork imageId={detail.imageId} name={detail.name} size={96} decorative />
              <div className="melodee-review-header__title">
                <Heading style={{ fontSize: '1.25rem' }}>{detail.name}</Heading>
                {detail.disambiguation ? (
                  <p className="melodee-page-header__subtitle">{detail.disambiguation}</p>
                ) : null}
                <ArtistHeaderFacts detail={detail} />
                {detail.externalIds.items.length > 0 ? (
                  <p style={{ marginBlockStart: '0.5rem' }}>
                    {detail.externalIds.items.map((externalId) => (
                      <Tag
                        key={`${externalId.provider}-${externalId.externalId}`}
                        type="outline"
                        title={externalId.externalId}
                        style={{ marginInlineEnd: '0.25rem' }}
                      >
                        {externalId.provider}
                      </Tag>
                    ))}
                  </p>
                ) : null}
              </div>
              <StatusTag
                kind={catalogStatusKind(detail.status)}
                label={t(`catalog.status.${detail.status}`, { defaultValue: detail.status })}
              />
            </div>
          </div>

          <div className="melodee-metric-grid">
            <MetricCard
              label={t('catalog.artist.stats.releases')}
              value={formatNumber(detail.stats.releaseCount)}
            />
            <MetricCard
              label={t('catalog.artist.stats.billedTracks')}
              value={formatNumber(detail.stats.billedTrackCount)}
            />
            <MetricCard
              label={t('catalog.artist.stats.performanceTracks')}
              value={formatNumber(detail.stats.performanceTrackCount)}
            />
            <MetricCard
              label={t('catalog.artist.stats.aliases')}
              value={formatNumber(detail.aliases.total)}
            />
          </div>

          <Tabs>
            <TabList aria-label={t('catalog.artist.tabsLabel')}>
              <Tab>{t('catalog.artist.tabs.overview')}</Tab>
              <Tab>{t('catalog.artist.tabs.releases')}</Tab>
              <Tab>{t('catalog.artist.tabs.appearances')}</Tab>
              <Tab>{t('catalog.artist.tabs.contributions')}</Tab>
              <Tab>{t('catalog.artist.tabs.relationships')}</Tab>
              <Tab>{t('catalog.artist.tabs.identity')}</Tab>
              <Tab>{t('catalog.artist.tabs.images')}</Tab>
              <Tab>{t('catalog.artist.tabs.history')}</Tab>
            </TabList>
            <TabPanels>
              <TabPanel>
                <Heading className="melodee-panel__title">
                  {t('catalog.artist.aliases.title')}
                </Heading>
                {detail.aliases.items.length === 0 ? (
                  <EmptyState title={t('catalog.artist.aliases.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.artist.aliases.caption')}>
                      <Table size="sm" aria-label={t('catalog.artist.aliases.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.artist.aliases.column.alias')}</TableHeader>
                            <TableHeader>{t('catalog.column.type')}</TableHeader>
                            <TableHeader>{t('catalog.artist.aliases.column.locale')}</TableHeader>
                            <TableHeader>{t('catalog.column.evidence')}</TableHeader>
                            <TableHeader>{t('catalog.column.confidence')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.aliases.items.map((alias) => (
                            <TableRow key={`${alias.alias}-${alias.createdAt}`}>
                              <TableCell>{alias.alias}</TableCell>
                              <TableCell>
                                {t(`catalog.aliasType.${alias.aliasType}`, {
                                  defaultValue: alias.aliasType,
                                })}
                              </TableCell>
                              <TableCell>{alias.locale ?? t('common.none')}</TableCell>
                              <TableCell>{alias.evidenceSource}</TableCell>
                              <TableCell>{confidencePercent(alias.confidence)}</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote shown={detail.aliases.items.length} total={detail.aliases.total} />
              </TabPanel>

              <TabPanel>
                {detail.releases.items.length === 0 ? (
                  <EmptyState title={t('catalog.artist.releases.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.artist.releases.caption')}>
                      <Table size="sm" aria-label={t('catalog.artist.releases.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.column.title')}</TableHeader>
                            <TableHeader>{t('catalog.column.role')}</TableHeader>
                            <TableHeader>{t('catalog.column.releaseDate')}</TableHeader>
                            <TableHeader>{t('catalog.column.tracks')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.releases.items.map((release) => (
                            <TableRow key={`${release.releaseId}-${release.role}`}>
                              <TableCell>
                                <ReleaseLink id={release.releaseId} name={release.title} />
                              </TableCell>
                              <TableCell>{release.role}</TableCell>
                              <TableCell>
                                {release.releaseDate
                                  ? formatDate(release.releaseDate)
                                  : t('common.none')}
                              </TableCell>
                              <TableCell>{formatNumber(release.trackCount)}</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote shown={detail.releases.items.length} total={detail.releases.total} />
              </TabPanel>

              <TabPanel>
                <ContributionTable
                  caption={t('catalog.artist.appearances.caption')}
                  items={detail.contributions.items.filter((item) => item.isPerformance)}
                  emptyKey="catalog.artist.appearances.empty"
                />
                <BoundedNote
                  shown={detail.contributions.items.length}
                  total={detail.contributions.total}
                />
              </TabPanel>

              <TabPanel>
                <ContributionTable
                  caption={t('catalog.artist.contributions.caption')}
                  items={detail.contributions.items.filter((item) => !item.isPerformance)}
                  emptyKey="catalog.artist.contributions.empty"
                />
                <BoundedNote
                  shown={detail.contributions.items.length}
                  total={detail.contributions.total}
                />
              </TabPanel>

              <TabPanel>
                <Heading className="melodee-panel__title">
                  {t('catalog.artist.relationships.title')}
                </Heading>
                {detail.relationships.items.length === 0 ? (
                  <EmptyState title={t('catalog.artist.relationships.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.artist.relationships.caption')}>
                      <Table size="sm" aria-label={t('catalog.artist.relationships.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.artist.relationships.column.artist')}</TableHeader>
                            <TableHeader>{t('catalog.column.type')}</TableHeader>
                            <TableHeader>{t('catalog.column.role')}</TableHeader>
                            <TableHeader>{t('catalog.column.dateRange')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.relationships.items.map((relationship) => {
                            const other =
                              relationship.sourceArtistId === detail.id
                                ? {
                                    id: relationship.targetArtistId,
                                    name: relationship.targetName,
                                  }
                                : {
                                    id: relationship.sourceArtistId,
                                    name: relationship.sourceName,
                                  };
                            return (
                              <TableRow
                                key={`${relationship.sourceArtistId}-${relationship.targetArtistId}-${relationship.relationshipType}`}
                              >
                                <TableCell>
                                  <ArtistLink id={other.id} name={other.name} />
                                </TableCell>
                                <TableCell>{relationship.relationshipType}</TableCell>
                                <TableCell>{relationship.role ?? t('common.none')}</TableCell>
                                <TableCell>
                                  {dateRangeLabel(relationship.startDate, relationship.endDate) ||
                                    t('common.none')}
                                </TableCell>
                              </TableRow>
                            );
                          })}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote
                  shown={detail.relationships.items.length}
                  total={detail.relationships.total}
                />
                <Heading className="melodee-panel__title" style={{ marginBlockStart: '1rem' }}>
                  {t('catalog.artist.memberships.title')}
                </Heading>
                {detail.memberships.items.length === 0 ? (
                  <EmptyState title={t('catalog.artist.memberships.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.artist.memberships.caption')}>
                      <Table size="sm" aria-label={t('catalog.artist.memberships.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.artist.memberships.column.artist')}</TableHeader>
                            <TableHeader>{t('catalog.column.role')}</TableHeader>
                            <TableHeader>{t('catalog.column.dateRange')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.memberships.items.map((membership) => {
                            const isGroup = membership.groupArtistId === detail.id;
                            const other = isGroup
                              ? { id: membership.memberArtistId, name: membership.memberName }
                              : { id: membership.groupArtistId, name: membership.groupName };
                            return (
                              <TableRow key={`${membership.memberArtistId}-${membership.groupArtistId}`}>
                                <TableCell>
                                  <ArtistLink id={other.id} name={other.name} />
                                  <span className="melodee-review-track__file">
                                    {isGroup
                                      ? t('catalog.artist.memberships.memberOfThis')
                                      : t('catalog.artist.memberships.thisIsMember')}
                                  </span>
                                </TableCell>
                                <TableCell>{membership.role}</TableCell>
                                <TableCell>
                                  {dateRangeLabel(membership.startDate, membership.endDate) ||
                                    t('common.none')}
                                </TableCell>
                              </TableRow>
                            );
                          })}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote
                  shown={detail.memberships.items.length}
                  total={detail.memberships.total}
                />
              </TabPanel>

              <TabPanel>
                <dl className="melodee-fact-list">
                  <div className="melodee-fact">
                    <dt>{t('catalog.artist.fact.sortName')}</dt>
                    <dd>{detail.sortName}</dd>
                  </div>
                  <div className="melodee-fact">
                    <dt>{t('catalog.artist.fact.type')}</dt>
                    <dd>
                      {t(`catalog.artistType.${detail.artistType}`, {
                        defaultValue: detail.artistType,
                      })}
                    </dd>
                  </div>
                  <div className="melodee-fact">
                    <dt>{t('common.status')}</dt>
                    <dd>
                      <StatusTag
                        kind={catalogStatusKind(detail.status)}
                        label={t(`catalog.status.${detail.status}`, {
                          defaultValue: detail.status,
                        })}
                      />
                    </dd>
                  </div>
                  <div className="melodee-fact">
                    <dt>{t('catalog.artist.fact.beginDate')}</dt>
                    <dd>{detail.beginDate ? formatDate(detail.beginDate) : t('common.none')}</dd>
                  </div>
                  <div className="melodee-fact">
                    <dt>{t('catalog.artist.fact.endDate')}</dt>
                    <dd>{detail.endDate ? formatDate(detail.endDate) : t('common.none')}</dd>
                  </div>
                  <div className="melodee-fact">
                    <dt>{t('catalog.artist.fact.revision')}</dt>
                    <dd>{formatNumber(detail.revision)}</dd>
                  </div>
                  <div className="melodee-fact">
                    <dt>{t('common.created')}</dt>
                    <dd>{formatDateTime(detail.createdAt)}</dd>
                  </div>
                  <div className="melodee-fact">
                    <dt>{t('common.updated')}</dt>
                    <dd>{formatDateTime(detail.updatedAt)}</dd>
                  </div>
                </dl>
                {detail.mergedIntoArtistId ? (
                  <p style={{ marginBlockStart: '0.5rem' }}>
                    {t('catalog.artist.mergedInto')}{' '}
                    <ArtistLink
                      id={detail.mergedIntoArtistId}
                      name={detail.mergedIntoArtistId}
                    />
                  </p>
                ) : null}
                <Heading className="melodee-panel__title" style={{ marginBlockStart: '1rem' }}>
                  {t('catalog.artist.externalIds.title')}
                </Heading>
                {detail.externalIds.items.length === 0 ? (
                  <EmptyState title={t('catalog.artist.externalIds.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.artist.externalIds.caption')}>
                      <Table size="sm" aria-label={t('catalog.artist.externalIds.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.artist.externalIds.column.provider')}</TableHeader>
                            <TableHeader>{t('catalog.artist.externalIds.column.externalId')}</TableHeader>
                            <TableHeader>{t('catalog.column.confidence')}</TableHeader>
                            <TableHeader>{t('catalog.column.evidence')}</TableHeader>
                            <TableHeader>{t('catalog.artist.externalIds.column.verifiedAt')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.externalIds.items.map((externalId) => (
                            <TableRow key={`${externalId.provider}-${externalId.externalId}`}>
                              <TableCell>{externalId.provider}</TableCell>
                              <TableCell>{externalId.externalId}</TableCell>
                              <TableCell>{confidencePercent(externalId.confidence)}</TableCell>
                              <TableCell>{externalId.evidenceSource}</TableCell>
                              <TableCell>
                                {externalId.verifiedAt
                                  ? formatDateTime(externalId.verifiedAt)
                                  : t('common.none')}
                              </TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote
                  shown={detail.externalIds.items.length}
                  total={detail.externalIds.total}
                />
              </TabPanel>

              <TabPanel>
                <p className="melodee-panel__subtitle">{t('catalog.artist.images.description')}</p>
                <ArtworkManager
                  entity="artists"
                  entityId={detail.id}
                  imageId={detail.imageId}
                  name={detail.name}
                  onChanged={retry}
                />
              </TabPanel>

              <TabPanel>
                <AuditTimeline events={detail.history.items.map(toCatalogAuditEvent)} />
                <BoundedNote shown={detail.history.items.length} total={detail.history.total} />
              </TabPanel>
            </TabPanels>
          </Tabs>
        </>
      ) : null}
    </div>
  );
}
