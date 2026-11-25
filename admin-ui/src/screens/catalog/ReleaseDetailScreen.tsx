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
  Tag,
} from '@carbon/react';
import { useTranslation } from 'react-i18next';
import { useState } from 'react';
import { Link as RouterLink, useParams } from 'react-router-dom';

import { getCatalogRelease } from '../../api/admin';
import { updateReleaseMetadata } from '../../api/admin';
import type { ReleaseDetail } from '../../api/types';
import { AudioPreview } from '../../components/AudioPreview';
import { AuditTimeline } from '../../components/AuditTimeline';
import { EmptyState } from '../../components/EmptyState';
import { ArtistLink, TrackLink } from '../../components/EntityLink';
import { HeroCard } from '../../components/HeroCard';
import { PageHeader } from '../../components/PageHeader';
import { StatusChip, StatusTag } from '../../components/StatusTag';
import { Surface } from '../../components/Surface';
import { TabNav } from '../../components/TabNav';
import {
  formatBytes,
  formatDate,
  formatDateTime,
  formatDuration,
  formatNumber,
} from '../../i18n/format';
import {
  catalogStatusKind,
  releaseStatusPresentation,
  toCatalogAuditEvent,
  trackOrdinalLabel,
  useCatalogDetail,
} from './catalog';
import { ArtworkManager } from './ArtworkManager';
import { CatalogArtwork } from './CatalogArtwork';
import { EditPanel } from './EditPanel';
import { MetadataTransformPanel } from './MetadataTransformPanel';

/**
 * Release Detail workspace (PRD-UI-006): header artwork/title/status
 * plus the track-count/duration/media-size stats strip, then tabbed sections
 * — Overview (facts that exist only), Tracks (ordered, with format facts and
 * a labeled signed audio preview per playable Track, PRD-UI-007),
 * Artists (billed, with join phrases), Contributors, Files (media assets),
 * Classification (traits or an honest not-yet-classified state),
 * Provenance (import manifest evidence), History (audit). Release
 * type/label/country/genre/barcode are not modeled in the schema, so
 * they are never rendered (mockup text is not data, PRD-UI-REF-001).
 */

function confidencePercent(confidence: number): string {
  return `${formatNumber(Math.round(confidence / 100))}%`;
}

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

/** Uniform track format across the Release ("FLAC / flac"), or empty when the
 *  Tracks disagree — a Release itself has no format field. */
function releaseFormatLabel(detail: ReleaseDetail): string {
  const formats = new Set(
    detail.tracks.items
      .map((track) => formatLabel(track.container, track.codec, ''))
      .filter((value) => value !== ''),
  );
  return formats.size === 1 ? ([...formats][0] ?? '') : '';
}

/** Container/codec pair, or an honest absence. */
function formatLabel(container: string | null, codec: string | null, none: string): string {
  return [container, codec].filter(Boolean).join(' / ') || none;
}

function ReleaseFacts({ detail }: { detail: ReleaseDetail }) {
  const { t } = useTranslation();
  // Only facts that exist in the DTO are rendered; nulls are skipped.
  const facts: { label: string; value: string }[] = [
    ...(detail.displayArtist
      ? [{ label: t('catalog.column.artist'), value: detail.displayArtist }]
      : []),
    ...(detail.releaseDate
      ? [{ label: t('catalog.column.releaseDate'), value: formatDate(detail.releaseDate) }]
      : detail.releaseDateText
        ? [{ label: t('catalog.column.releaseDate'), value: detail.releaseDateText }]
        : []),
    { label: t('catalog.release.fact.sortTitle'), value: detail.sortTitle },
    { label: t('catalog.release.fact.revision'), value: formatNumber(detail.revision) },
    { label: t('common.created'), value: formatDateTime(detail.createdAt) },
    { label: t('common.updated'), value: formatDateTime(detail.updatedAt) },
  ];
  return (
    <dl className="melodee-fact-list">
      {facts.map((fact) => (
        <div className="melodee-fact" key={fact.label}>
          <dt>{fact.label}</dt>
          <dd>{fact.value}</dd>
        </div>
      ))}
    </dl>
  );
}

export function ReleaseDetailScreen() {
  const { t } = useTranslation();
  const { id = '' } = useParams();
  const [editing, setEditing] = useState(false);
  const [tab, setTab] = useState('overview');
  const { detail, error, loading, retry } = useCatalogDetail(getCatalogRelease, id);

  if (loading && detail === null) {
    return (
      <>
        <PageHeader title={t('catalog.release.detailTitle')} subtitle={t('catalog.release.detailSubtitle')} />
        <Loading description={t('common.loading')} withOverlay={false} />
      </>
    );
  }

  return (
    <div>
      <PageHeader title={t('catalog.release.detailTitle')} subtitle={t('catalog.release.detailSubtitle')} />
      <p>
        <Link as={RouterLink} to="/catalog/releases">
          {t('catalog.release.back')}
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
          {(() => {
            const status = releaseStatusPresentation(detail);
            return detail.workflowState === 'needs_review' ? (
              <ActionableNotification
                inline
                kind="warning"
                lowContrast
                title={t(status.labelKey)}
                subtitle={status.description ?? t('queue.nextAction.needs_review')}
                hideCloseButton
              />
            ) : null;
          })()}
          <p>
            <Link className="melodee-detail-edit-link" as="button" onClick={() => setEditing((prev) => !prev)}>
              {editing ? t('catalog.edit.cancel') : t('catalog.edit')}
            </Link>
          </p>
          {editing ? (
            <EditPanel
              title={t('catalog.release.edit.title')}
              fields={[
                { name: 'title', label: t('catalog.release.edit.title'), type: 'string', value: detail.title },
                { name: 'sortTitle', label: t('catalog.release.edit.sortTitle'), type: 'string', value: detail.sortTitle },
                { name: 'displayArtist', label: t('catalog.column.artist'), type: 'string', value: detail.displayArtist ?? '' },
                {
                  // ISO date, validated inline: the server parses `YYYY-MM-DD`
                  // and rejects anything else with 400.
                  name: 'releaseDate',
                  label: t('catalog.release.edit.date'),
                  type: 'date',
                  placeholder: 'YYYY-MM-DD',
                  invalidText: t('catalog.release.edit.dateInvalid'),
                  value: detail.releaseDate ?? '',
                },
                { name: 'releaseDateText', label: t('catalog.release.edit.dateText'), type: 'string', value: detail.releaseDateText ?? '' },
              ]}
              buildPatch={(state) => ({
                title: state['title'] !== undefined && String(state['title']).trim() !== '' ? String(state['title']) : null,
                sortTitle: state['sortTitle'] !== undefined && String(state['sortTitle']).trim() !== '' ? String(state['sortTitle']) : null,
                displayArtist: state['displayArtist'] !== undefined && String(state['displayArtist']).trim() !== '' ? String(state['displayArtist']) : null,
                releaseDate: state['releaseDate'] !== undefined && String(state['releaseDate']).trim() !== '' ? String(state['releaseDate']) : null,
                releaseDateText: state['releaseDateText'] !== undefined && String(state['releaseDateText']).trim() !== '' ? String(state['releaseDateText']) : null,
                // `country` and `releaseType` are not part of the metadata
                // contract (the columns do not exist), so the form no longer
                // sends values the server would silently discard.
              })}
              onSave={(patch) => updateReleaseMetadata(id, patch)}
              refresh={retry}
            />
          ) : null}
          <HeroCard
            media={
              <CatalogArtwork cacheKey={detail.imageUpdatedAt} imageId={detail.imageId} name={detail.title} size={176} decorative />
            }
            title={detail.title}
            subtitle={[
              detail.displayArtist,
              detail.releaseDate ? formatDate(detail.releaseDate) : detail.releaseDateText,
              releaseFormatLabel(detail),
            ]
              .filter(Boolean)
              .join(' · ')}
            chips={(() => {
              const status = releaseStatusPresentation(detail);
              return (
                <StatusChip
                  kind={status.kind}
                  label={t(status.labelKey, { defaultValue: detail.status })}
                  description={status.description}
                />
              );
            })()}
            facts={[
              { label: t('catalog.column.tracks'), value: formatNumber(detail.trackCount) },
              { label: t('catalog.column.duration'), value: formatDuration(detail.durationMs) },
              { label: t('catalog.release.fact.mediaSize'), value: formatBytes(detail.mediaBytes) },
              {
                label: t('catalog.column.releaseDate'),
                value: detail.releaseDate
                  ? formatDate(detail.releaseDate)
                  : (detail.releaseDateText ?? t('common.none')),
              },
            ]}
          />

          <TabNav
            ariaLabel={t('catalog.release.tabsLabel')}
            value={tab}
            onChange={setTab}
            tabs={[
              { key: 'overview', label: t('catalog.release.tabs.overview') },
              { key: 'tracks', label: t('catalog.release.tabs.tracks') },
              { key: 'artists', label: t('catalog.release.tabs.artists') },
              { key: 'contributors', label: t('catalog.release.tabs.contributors') },
              { key: 'files', label: t('catalog.release.tabs.files') },
              { key: 'classification', label: t('catalog.release.tabs.classification') },
              { key: 'provenance', label: t('catalog.release.tabs.provenance') },
              { key: 'images', label: t('catalog.release.tabs.images') },
              { key: 'history', label: t('catalog.release.tabs.history') },
            ]}
          />
          {tab === 'overview' ? (
            <div className="melodee-release-overview">
                <div className="melodee-review-split">
                  <Surface title={t('catalog.release.overview.information')}>
                    <ReleaseFacts detail={detail} />
                  </Surface>
                  <Surface title={t('catalog.release.overview.mediaFiles')}>
                    {detail.mediaAssets.items.length === 0 ? (
                      <EmptyState title={t('catalog.release.files.empty')} compact />
                    ) : (
                      <ul className="melodee-link-list">
                        {detail.mediaAssets.items.map((asset) => (
                          <li className="melodee-link-list__row" key={asset.id}>
                            <span className="melodee-cell-stack__primary">
                              {asset.canonicalRelativePath.split('/').pop() ??
                                asset.canonicalRelativePath}
                            </span>
                            <span className="melodee-link-list__meta">
                              {formatBytes(asset.byteSize)}
                              {' · '}
                              <StatusChip
                                kind="neutral"
                                hideIcon
                                label={(asset.container ?? asset.mimeType)
                                  .split('/')
                                  .pop()
                                  ?.toUpperCase() ?? asset.mimeType}
                              />
                            </span>
                          </li>
                        ))}
                      </ul>
                    )}
                    <BoundedNote
                      shown={detail.mediaAssets.items.length}
                      total={detail.mediaAssets.total}
                    />
                  </Surface>
                </div>
                <MetadataTransformPanel releaseId={id} refresh={retry} />
            </div>
          ) : null}
          {tab === 'tracks' ? (
                <Surface title={t('catalog.release.tracks.title')} variant="flush">
                {detail.tracks.items.length === 0 ? (
                  <EmptyState title={t('catalog.release.tracks.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.release.tracks.caption')}>
                      <Table size="sm" aria-label={t('catalog.release.tracks.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.column.ordinal')}</TableHeader>
                            <TableHeader>{t('catalog.column.title')}</TableHeader>
                            <TableHeader>{t('catalog.column.duration')}</TableHeader>
                            <TableHeader>{t('catalog.column.format')}</TableHeader>
                            <TableHeader>{t('catalog.column.size')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.tracks.items.map((track) => (
                            <TableRow key={track.id}>
                              <TableCell>
                                {trackOrdinalLabel(
                                  track.ordinal,
                                  track.discNumber,
                                  track.trackNumber,
                                  formatNumber,
                                )}
                              </TableCell>
                              <TableCell>
                                <TrackLink id={track.id} name={track.title} />
                                {track.displayArtist ? (
                                  <span className="melodee-review-track__file">
                                    {track.displayArtist}
                                  </span>
                                ) : null}
                                {track.streamUrl ? (
                                  // Signed same-origin preview (PRD-UI-007); null
                                  // streamUrl renders no affordance at all.
                                  <AudioPreview
                                    src={track.streamUrl}
                                    label={t('catalog.track.previewLabel', { title: track.title })}
                                  />
                                ) : null}
                              </TableCell>
                              <TableCell>
                                {track.durationMs != null
                                  ? formatDuration(track.durationMs)
                                  : t('common.none')}
                              </TableCell>
                              <TableCell>
                                {formatLabel(track.container, track.codec, t('common.none'))}
                              </TableCell>
                              <TableCell>
                                {track.byteSize != null
                                  ? formatBytes(track.byteSize)
                                  : t('common.none')}
                              </TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote shown={detail.tracks.items.length} total={detail.tracks.total} />
                </Surface>
          ) : null}
          {tab === 'artists' ? (
                <Surface title={t('catalog.release.artists.title')} variant="flush">
                {detail.artists.length === 0 ? (
                  <EmptyState title={t('catalog.release.artists.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.release.artists.caption')}>
                      <Table size="sm" aria-label={t('catalog.release.artists.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.column.name')}</TableHeader>
                            <TableHeader>{t('catalog.column.role')}</TableHeader>
                            <TableHeader>{t('catalog.release.artists.column.displayValue')}</TableHeader>
                            <TableHeader>{t('catalog.release.artists.column.joinPhrase')}</TableHeader>
                            <TableHeader>{t('catalog.column.status')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.artists.map((artist) => (
                            <TableRow key={`${artist.artistId}-${artist.role}`}>
                              <TableCell>
                                <ArtistLink id={artist.artistId} name={artist.name} />
                              </TableCell>
                              <TableCell>{artist.role}</TableCell>
                              <TableCell>{artist.displayValue}</TableCell>
                              <TableCell>{artist.joinPhrase || t('common.none')}</TableCell>
                              <TableCell>
                                <StatusTag
                                  kind={catalogStatusKind(artist.status)}
                                  label={t(`catalog.status.${artist.status}`, {
                                    defaultValue: artist.status,
                                  })}
                                />
                              </TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                </Surface>
          ) : null}
          {tab === 'contributors' ? (
                <Surface title={t('catalog.release.contributors.title')} variant="flush">
                {detail.contributors.items.length === 0 ? (
                  <EmptyState title={t('catalog.release.contributors.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.release.contributors.caption')}>
                      <Table size="sm" aria-label={t('catalog.release.contributors.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.column.name')}</TableHeader>
                            <TableHeader>{t('catalog.column.role')}</TableHeader>
                            <TableHeader>{t('catalog.column.track')}</TableHeader>
                            <TableHeader>{t('catalog.column.evidence')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.contributors.items.map((contributor, index) => (
                            <TableRow key={`${contributor.artistId}-${contributor.role}-${index}`}>
                              <TableCell>
                                <ArtistLink id={contributor.artistId} name={contributor.artistName} />
                              </TableCell>
                              <TableCell>
                                {contributor.role}
                                {contributor.roleDetail ? (
                                  <span className="melodee-review-track__file">
                                    {contributor.roleDetail}
                                  </span>
                                ) : null}
                              </TableCell>
                              <TableCell>
                                {contributor.trackId && contributor.trackTitle ? (
                                  <TrackLink id={contributor.trackId} name={contributor.trackTitle} />
                                ) : (
                                  t('catalog.contribution.releaseLevel')
                                )}
                              </TableCell>
                              <TableCell>{contributor.evidenceSource}</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote
                  shown={detail.contributors.items.length}
                  total={detail.contributors.total}
                />
                </Surface>
          ) : null}
          {tab === 'files' ? (
                <Surface title={t('catalog.release.files.title')} variant="flush">
                {detail.mediaAssets.items.length === 0 ? (
                  <EmptyState title={t('catalog.release.files.empty')} compact />
                ) : (
                  <div className="melodee-review-table-scroll">
                    <TableContainer title={t('catalog.release.files.caption')}>
                      <Table size="sm" aria-label={t('catalog.release.files.caption')}>
                        <TableHead>
                          <TableRow>
                            <TableHeader>{t('catalog.release.files.column.path')}</TableHeader>
                            <TableHeader>{t('catalog.release.files.column.mimeType')}</TableHeader>
                            <TableHeader>{t('catalog.column.format')}</TableHeader>
                            <TableHeader>{t('catalog.column.size')}</TableHeader>
                          </TableRow>
                        </TableHead>
                        <TableBody>
                          {detail.mediaAssets.items.map((asset) => (
                            <TableRow key={asset.id}>
                              <TableCell>{asset.canonicalRelativePath}</TableCell>
                              <TableCell>{asset.mimeType}</TableCell>
                              <TableCell>
                                {formatLabel(asset.container, asset.codec, t('common.none'))}
                              </TableCell>
                              <TableCell>{formatBytes(asset.byteSize)}</TableCell>
                            </TableRow>
                          ))}
                        </TableBody>
                      </Table>
                    </TableContainer>
                  </div>
                )}
                <BoundedNote
                  shown={detail.mediaAssets.items.length}
                  total={detail.mediaAssets.total}
                />
                </Surface>
          ) : null}
          {tab === 'classification' ? (
                <Surface title={t('catalog.release.classification.title')} variant="flush">
                {detail.classification ? (
                  <>
                    {detail.classification.traits.length > 0 ? (
                      <p>
                        {detail.classification.traits.map((trait) => (
                          <Tag key={trait} type="outline" style={{ marginInlineEnd: '0.25rem' }}>
                            {trait}
                          </Tag>
                        ))}
                      </p>
                    ) : null}
                    <dl className="melodee-fact-list">
                      <div className="melodee-fact">
                        <dt>{t('catalog.release.classification.confidence')}</dt>
                        <dd>{confidencePercent(detail.classification.confidence)}</dd>
                      </div>
                      <div className="melodee-fact">
                        <dt>{t('catalog.release.classification.classifiedAt')}</dt>
                        <dd>{formatDateTime(detail.classification.classifiedAt)}</dd>
                      </div>
                      <div className="melodee-fact">
                        <dt>{t('catalog.release.classification.version')}</dt>
                        <dd>{detail.classification.classifierVersion}</dd>
                      </div>
                    </dl>
                  </>
                ) : (
                  <EmptyState
                    title={t('catalog.release.classification.empty')}
                    description={t('catalog.release.classification.emptyDescription')}
                    compact
                  />
                )}
                </Surface>
          ) : null}
          {tab === 'provenance' ? (
                <Surface title={t('catalog.release.provenance.title')} variant="flush">
                {detail.provenance ? (
                  <dl className="melodee-fact-list">
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.workItem')}</dt>
                      <dd>{detail.provenance.workItemId}</dd>
                    </div>
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.manifest')}</dt>
                      <dd>{detail.provenance.manifestId}</dd>
                    </div>
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.schemaVersion')}</dt>
                      <dd>{formatNumber(detail.provenance.manifestSchemaVersion)}</dd>
                    </div>
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.semanticManifestHash')}</dt>
                      <dd>
                        <code>{detail.provenance.semanticManifestHash}</code>
                      </dd>
                    </div>
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.publicationPlanHash')}</dt>
                      <dd>
                        <code>{detail.provenance.publicationPlanHash}</code>
                      </dd>
                    </div>
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.sealedManifestHash')}</dt>
                      <dd>
                        <code>{detail.provenance.sealedManifestHash}</code>
                      </dd>
                    </div>
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.resolverVersion')}</dt>
                      <dd>{detail.provenance.resolverVersion}</dd>
                    </div>
                    <div className="melodee-fact">
                      <dt>{t('catalog.release.provenance.importedAt')}</dt>
                      <dd>{formatDateTime(detail.provenance.importedAt)}</dd>
                    </div>
                  </dl>
                ) : (
                  <EmptyState
                    title={t('catalog.release.provenance.empty')}
                    description={t('catalog.release.provenance.emptyDescription')}
                    compact
                  />
                )}
                </Surface>
          ) : null}
          {tab === 'images' ? (
                <Surface title={t('catalog.release.images.title')} variant="flush">
                <p className="melodee-panel__subtitle">
                  {t('catalog.release.images.description')}
                </p>
                <ArtworkManager
                  entity="releases"
                  entityId={detail.id}
                  imageId={detail.imageId}
                  imageUpdatedAt={detail.imageUpdatedAt}
                  name={detail.title}
                  onChanged={retry}
                />
                </Surface>
          ) : null}
          {tab === 'history' ? (
                <Surface title={t('catalog.release.history.title')} variant="flush">
                <AuditTimeline events={detail.history.items.map(toCatalogAuditEvent)} />
                <BoundedNote shown={detail.history.items.length} total={detail.history.total} />
                </Surface>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
