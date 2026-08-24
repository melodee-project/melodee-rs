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
import { useState } from 'react';
import { Link as RouterLink, useParams } from 'react-router-dom';

import { getCatalogRelease } from '../../api/admin';
import { updateReleaseMetadata } from '../../api/admin';
import type { ReleaseDetail } from '../../api/types';
import { AudioPreview } from '../../components/AudioPreview';
import { AuditTimeline } from '../../components/AuditTimeline';
import { EmptyState } from '../../components/EmptyState';
import { ArtistLink, TrackLink } from '../../components/EntityLink';
import { PageHeader } from '../../components/PageHeader';
import { StatusTag } from '../../components/StatusTag';
import {
  formatBytes,
  formatDate,
  formatDateTime,
  formatDuration,
  formatNumber,
} from '../../i18n/format';
import { catalogStatusKind, toCatalogAuditEvent, trackOrdinalLabel, useCatalogDetail } from './catalog';
import { ArtworkManager } from './ArtworkManager';
import { CatalogArtwork } from './CatalogArtwork';
import { EditPanel } from './EditPanel';

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
          <p>
            <Link as="button" onClick={() => setEditing((prev) => !prev)}>
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
                { name: 'releaseDate', label: t('catalog.release.edit.date'), type: 'string', value: detail.releaseDate ?? '' },
                { name: 'releaseDateText', label: t('catalog.release.edit.dateText'), type: 'string', value: detail.releaseDateText ?? '' },
                { name: 'country', label: t('catalog.release.edit.country'), type: 'string', value: detail.country ?? null },
                { name: 'type', label: t('catalog.release.edit.type'), type: 'string', value: detail.type ?? null },
              ]}
              buildPatch={(state) => ({
                title: state['title'] !== undefined && String(state['title']).trim() !== '' ? String(state['title']) : null,
                sortTitle: state['sortTitle'] !== undefined && String(state['sortTitle']).trim() !== '' ? String(state['sortTitle']) : null,
                displayArtist: state['displayArtist'] !== undefined && String(state['displayArtist']).trim() !== '' ? String(state['displayArtist']) : null,
                releaseDate: state['releaseDate'] !== undefined && String(state['releaseDate']).trim() !== '' ? String(state['releaseDate']) : null,
                releaseDateText: state['releaseDateText'] !== undefined && String(state['releaseDateText']).trim() !== '' ? String(state['releaseDateText']) : null,
                country: state['country'] !== undefined && String(state['country']).trim() !== '' ? String(state['country']) : null,
                type: state['type'] !== undefined && String(state['type']).trim() !== '' ? String(state['type']) : null,
              })}
              onSave={(patch) => updateReleaseMetadata(id, patch)}
              refresh={retry}
            />
          ) : null}
          <div className="melodee-review-card">
            <div className="melodee-review-header">
              <CatalogArtwork imageId={detail.imageId} name={detail.title} size={96} decorative />
              <div className="melodee-review-header__title">
                <Heading style={{ fontSize: '1.25rem' }}>{detail.title}</Heading>
                {detail.displayArtist ? (
                  <p className="melodee-page-header__subtitle">{detail.displayArtist}</p>
                ) : null}
                <div className="melodee-review-header__facts">
                  <div>
                    <div className="melodee-review-fact__label">{t('catalog.column.tracks')}</div>
                    <div>{formatNumber(detail.trackCount)}</div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">{t('catalog.column.duration')}</div>
                    <div>{formatDuration(detail.durationMs)}</div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">
                      {t('catalog.release.fact.mediaSize')}
                    </div>
                    <div>{formatBytes(detail.mediaBytes)}</div>
                  </div>
                  {detail.releaseDate || detail.releaseDateText ? (
                    <div>
                      <div className="melodee-review-fact__label">
                        {t('catalog.column.releaseDate')}
                      </div>
                      <div>
                        {detail.releaseDate
                          ? formatDate(detail.releaseDate)
                          : detail.releaseDateText}
                      </div>
                    </div>
                  ) : null}
                </div>
              </div>
              <StatusTag
                kind={catalogStatusKind(detail.status)}
                label={t(`catalog.status.${detail.status}`, { defaultValue: detail.status })}
              />
            </div>
          </div>

          <Tabs>
            <TabList aria-label={t('catalog.release.tabsLabel')}>
              <Tab>{t('catalog.release.tabs.overview')}</Tab>
              <Tab>{t('catalog.release.tabs.tracks')}</Tab>
              <Tab>{t('catalog.release.tabs.artists')}</Tab>
              <Tab>{t('catalog.release.tabs.contributors')}</Tab>
              <Tab>{t('catalog.release.tabs.files')}</Tab>
              <Tab>{t('catalog.release.tabs.classification')}</Tab>
              <Tab>{t('catalog.release.tabs.provenance')}</Tab>
              <Tab>{t('catalog.release.tabs.images')}</Tab>
              <Tab>{t('catalog.release.tabs.history')}</Tab>
            </TabList>
            <TabPanels>
              <TabPanel>
                <ReleaseFacts detail={detail} />
              </TabPanel>

              <TabPanel>
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
              </TabPanel>

              <TabPanel>
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
              </TabPanel>

              <TabPanel>
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
              </TabPanel>

              <TabPanel>
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
              </TabPanel>

              <TabPanel>
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
              </TabPanel>

              <TabPanel>
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
              </TabPanel>

              <TabPanel>
                <p className="melodee-panel__subtitle">
                  {t('catalog.release.images.description')}
                </p>
                <ArtworkManager
                  entity="releases"
                  entityId={detail.id}
                  imageId={detail.imageId}
                  name={detail.title}
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
