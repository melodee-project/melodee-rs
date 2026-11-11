import {
  ActionableNotification,
  Link,
  Loading,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
} from '@carbon/react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink, useParams } from 'react-router-dom';
import { useState } from 'react';

import { getCatalogArtist, updateArtistMetadata } from '../../api/admin';
import type { ArtistContribution, ArtistDetail } from '../../api/types';
import { AuditTimeline } from '../../components/AuditTimeline';
import { EmptyState } from '../../components/EmptyState';
import { EntityAvatar } from '../../components/EntityAvatar';
import { ArtistLink, ReleaseLink, TrackLink } from '../../components/EntityLink';
import { HeroCard } from '../../components/HeroCard';
import { PageHeader } from '../../components/PageHeader';
import { StatStrip } from '../../components/StatStrip';
import { StatusChip, StatusTag } from '../../components/StatusTag';
import { Surface } from '../../components/Surface';
import { TabNav } from '../../components/TabNav';
import { formatDate, formatDateTime, formatNumber } from '../../i18n/format';
import { catalogStatusKind, toCatalogAuditEvent, useCatalogDetail } from './catalog';
import { ArtworkManager } from './ArtworkManager';
import { CatalogArtwork } from './CatalogArtwork';
import { EditPanel } from './EditPanel';

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

/** Hero fact strip (UI-SPEC §6.1): identity facts the reference shows at the
 *  top of the Artist page. Genres and a biography are intentionally absent —
 *  the catalog payload has no genre or biography field, so rendering them would
 *  be invention. */
function artistHeroFacts(detail: ArtistDetail, t: TFunction) {
  return [
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
    { label: t('catalog.artist.fact.sortName'), value: detail.sortName },
  ];
}

export function ArtistDetailScreen() {
  const { t } = useTranslation();
  const { id = '' } = useParams();
  const [editing, setEditing] = useState(false);
  const [tab, setTab] = useState('overview');
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
          <p>
            <Link className="melodee-detail-edit-link" as="button" onClick={() => setEditing((prev) => !prev)}>
              {editing ? t('catalog.edit.cancel') : t('catalog.edit')}
            </Link>
          </p>
          {editing ? (
            <EditPanel
              title={t('catalog.artist.edit.title')}
              fields={[
                { name: 'canonicalName', label: t('catalog.artist.edit.name'), type: 'string', value: detail.name },
                { name: 'sortName', label: t('catalog.artist.edit.sortName'), type: 'string', value: detail.sortName },
                { name: 'artistType', label: t('catalog.artist.edit.type'), type: 'string', value: detail.artistType },
              ]}
              buildPatch={(state) => ({
                canonicalName: state['canonicalName'] !== undefined && String(state['canonicalName']).trim() !== '' ? String(state['canonicalName']) : null,
                sortName: state['sortName'] !== undefined && String(state['sortName']).trim() !== '' ? String(state['sortName']) : null,
                artistType: state['artistType'] !== undefined && String(state['artistType']).trim() !== '' ? String(state['artistType']) : null,
              })}
              onSave={(patch) => updateArtistMetadata(id, patch)}
              refresh={retry}
            />
          ) : null}
          <HeroCard
            media={<CatalogArtwork imageId={detail.imageId} name={detail.name} size={112} decorative />}
            title={detail.name}
            subtitle={detail.disambiguation ?? undefined}
            chips={
              <>
                <StatusChip
                  kind={catalogStatusKind(detail.status)}
                  label={t(`catalog.status.${detail.status}`, { defaultValue: detail.status })}
                />
                {detail.externalIds.items.map((externalId) => (
                  <StatusChip
                    key={`${externalId.provider}-${externalId.externalId}`}
                    kind="neutral"
                    label={externalId.provider}
                    description={externalId.externalId}
                    hideIcon
                  />
                ))}
              </>
            }
            facts={artistHeroFacts(detail, t)}
          />

          <StatStrip
            ariaLabel={t('catalog.artist.stats.label')}
            items={[
              {
                label: t('catalog.artist.stats.releases'),
                value: formatNumber(detail.stats.releaseCount),
              },
              {
                label: t('catalog.artist.stats.billedTracks'),
                value: formatNumber(detail.stats.billedTrackCount),
              },
              {
                label: t('catalog.artist.stats.members'),
                value: formatNumber(detail.memberships.total),
              },
              {
                label: t('catalog.artist.stats.appearances'),
                value: formatNumber(detail.stats.performanceTrackCount),
              },
              {
                label: t('catalog.artist.stats.aliases'),
                value: formatNumber(detail.aliases.total),
              },
            ]}
          />

          <TabNav
            ariaLabel={t('catalog.artist.tabsLabel')}
            value={tab}
            onChange={setTab}
            tabs={[
              { key: 'overview', label: t('catalog.artist.tabs.overview') },
              { key: 'releases', label: t('catalog.artist.tabs.releases') },
              { key: 'appearances', label: t('catalog.artist.tabs.appearances') },
              { key: 'contributions', label: t('catalog.artist.tabs.contributions') },
              { key: 'relationships', label: t('catalog.artist.tabs.relationships') },
              { key: 'identity', label: t('catalog.artist.tabs.identity') },
              { key: 'images', label: t('catalog.artist.tabs.images') },
              { key: 'history', label: t('catalog.artist.tabs.history') },
            ]}
          />
          {tab === 'overview' ? (
                <div className="melodee-overview-columns melodee-overview-columns--tertiary">
                  <Surface title={t('catalog.artist.aliases.title')}>
                    {detail.aliases.items.length === 0 ? (
                      <EmptyState title={t('catalog.artist.aliases.empty')} compact />
                    ) : (
                      <ul className="melodee-alias-list">
                        {detail.aliases.items.map((alias) => (
                          <li className="melodee-alias-list__row" key={`${alias.alias}-${alias.createdAt}`}>
                            <StatusChip kind="info" label={alias.alias} hideIcon />
                            <span className="melodee-alias-list__type">
                              {t(`catalog.aliasType.${alias.aliasType}`, {
                                defaultValue: alias.aliasType,
                              })}
                            </span>
                          </li>
                        ))}
                      </ul>
                    )}
                    <BoundedNote shown={detail.aliases.items.length} total={detail.aliases.total} />
                  </Surface>
                  <Surface title={t('catalog.artist.overview.topReleases')}>
                    {detail.releases.items.length === 0 ? (
                      <EmptyState title={t('catalog.artist.releases.empty')} compact />
                    ) : (
                      <ul className="melodee-link-list">
                        {detail.releases.items.slice(0, 6).map((release) => (
                          <li className="melodee-link-list__row" key={`${release.releaseId}-${release.role}`}>
                            <ReleaseLink id={release.releaseId} name={release.title} />
                            <span className="melodee-link-list__meta">
                              {release.releaseDate ? formatDate(release.releaseDate) : t('common.none')}
                              {' · '}
                              {t('catalog.artist.overview.trackCount', {
                                count: formatNumber(release.trackCount),
                              })}
                            </span>
                          </li>
                        ))}
                      </ul>
                    )}
                  </Surface>
                  <Surface title={t('catalog.artist.overview.relationships')}>
                    {detail.memberships.items.length === 0 ? (
                      <EmptyState title={t('catalog.artist.memberships.empty')} compact />
                    ) : (
                      <ul className="melodee-link-list">
                        {detail.memberships.items.slice(0, 6).map((membership) => {
                          const isGroup = membership.groupArtistId === detail.id;
                          const other = isGroup
                            ? { id: membership.memberArtistId, name: membership.memberName }
                            : { id: membership.groupArtistId, name: membership.groupName };
                          return (
                            <li
                              className="melodee-link-list__row melodee-link-list__row--with-avatar"
                              key={`${membership.memberArtistId}-${membership.groupArtistId}`}
                            >
                              <EntityAvatar name={other.name} size={32} decorative />
                              <span>
                                <ArtistLink id={other.id} name={other.name} />
                                <span className="melodee-link-list__meta">
                                  {[membership.role, dateRangeLabel(membership.startDate, membership.endDate)]
                                    .filter(Boolean)
                                    .join(' · ')}
                                </span>
                              </span>
                            </li>
                          );
                        })}
                      </ul>
                    )}
                  </Surface>
                </div>
          ) : null}
          {tab === 'releases' ? (
                <Surface title={t('catalog.artist.releases.title')} variant="flush">
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
                </Surface>
          ) : null}
          {tab === 'appearances' ? (
                <Surface title={t('catalog.artist.appearances.title')} variant="flush">
                <ContributionTable
                  caption={t('catalog.artist.appearances.caption')}
                  items={detail.contributions.items.filter((item) => item.isPerformance)}
                  emptyKey="catalog.artist.appearances.empty"
                />
                <BoundedNote
                  shown={detail.contributions.items.length}
                  total={detail.contributions.total}
                />
                </Surface>
          ) : null}
          {tab === 'contributions' ? (
                <Surface title={t('catalog.artist.contributions.title')} variant="flush">
                <ContributionTable
                  caption={t('catalog.artist.contributions.caption')}
                  items={detail.contributions.items.filter((item) => !item.isPerformance)}
                  emptyKey="catalog.artist.contributions.empty"
                />
                <BoundedNote
                  shown={detail.contributions.items.length}
                  total={detail.contributions.total}
                />
                </Surface>
          ) : null}
          {tab === 'relationships' ? (
                <Surface title={t('catalog.artist.relationships.title')} variant="flush">
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
                </Surface>
          ) : null}
          {tab === 'identity' ? (
                <Surface title={t('catalog.artist.identity.title')} variant="flush">
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
                <p className="melodee-surface__subtitle">{t('catalog.artist.externalIds.title')}</p>
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
                </Surface>
          ) : null}
          {tab === 'images' ? (
                <Surface title={t('catalog.artist.images.title')} variant="flush">
                <p className="melodee-panel__subtitle">{t('catalog.artist.images.description')}</p>
                <ArtworkManager
                  entity="artists"
                  entityId={detail.id}
                  imageId={detail.imageId}
                  name={detail.name}
                  onChanged={retry}
                />
                </Surface>
          ) : null}
          {tab === 'history' ? (
                <Surface title={t('catalog.artist.history.title')} variant="flush">
                <AuditTimeline events={detail.history.items.map(toCatalogAuditEvent)} />
                <BoundedNote shown={detail.history.items.length} total={detail.history.total} />
                </Surface>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
