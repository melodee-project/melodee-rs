import {
  ActionableNotification,
  Heading,
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
import { useTranslation } from 'react-i18next';
import { Link as RouterLink, useParams } from 'react-router-dom';
import { useState } from 'react';

import { getCatalogTrack, updateTrackMetadata } from '../../api/admin';
import { AudioPreview } from '../../components/AudioPreview';
import { EmptyState } from '../../components/EmptyState';
import { ArtistLink, ReleaseLink } from '../../components/EntityLink';
import { PageHeader } from '../../components/PageHeader';
import { StatusTag } from '../../components/StatusTag';
import { formatBytes, formatDateTime, formatDuration, formatNumber } from '../../i18n/format';
import { catalogStatusKind, trackOrdinalLabel, useCatalogDetail } from './catalog';
import { CatalogArtwork } from './CatalogArtwork';
import { EditPanel } from './EditPanel';

/**
 * Track Detail (PRD-UI-006/007): identity facts, the Release linkage,
 * billed Artists, contributors, and linked media assets, plus a labeled audio
 * preview when the DTO carries a signed stream URL (playable asset exists and
 * the acting administrator is stream-authorized). Read-only otherwise.
 */
export function TrackDetailScreen() {
  const { t } = useTranslation();
  const { id = '' } = useParams();
  const [editing, setEditing] = useState(false);
  const { detail, error, loading, retry } = useCatalogDetail(getCatalogTrack, id);

  if (loading && detail === null) {
    return (
      <>
        <PageHeader title={t('catalog.track.detailTitle')} subtitle={t('catalog.track.detailSubtitle')} />
        <Loading description={t('common.loading')} withOverlay={false} />
      </>
    );
  }

  return (
    <div>
      <PageHeader title={t('catalog.track.detailTitle')} subtitle={t('catalog.track.detailSubtitle')} />
      <p>
        <Link as={RouterLink} to="/catalog/tracks">
          {t('catalog.track.back')}
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
              title={t('catalog.track.edit.title')}
              fields={[
                { name: 'title', label: t('catalog.track.edit.titleField'), type: 'string', value: detail.title },
                { name: 'sortTitle', label: t('catalog.track.edit.sortTitle'), type: 'string', value: detail.sortTitle },
                { name: 'trackNumber', label: t('catalog.track.edit.number'), type: 'number', value: detail.trackNumber ?? undefined },
                { name: 'durationMs', label: t('catalog.track.edit.duration'), type: 'number', value: detail.durationMs ?? undefined },
              ]}
              buildPatch={(state) => ({
                title: state['title'] !== undefined && String(state['title']).trim() !== '' ? String(state['title']) : null,
                sortTitle: state['sortTitle'] !== undefined && String(state['sortTitle']).trim() !== '' ? String(state['sortTitle']) : null,
                trackNumber: (state['trackNumber'] as number | undefined) ?? undefined,
                durationMs: (state['durationMs'] as number | undefined) ?? undefined,
              })}
              onSave={(patch) => updateTrackMetadata(id, patch)}
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
                    <div className="melodee-review-fact__label">{t('catalog.column.ordinal')}</div>
                    <div>
                      {trackOrdinalLabel(
                        detail.ordinal,
                        detail.discNumber,
                        detail.trackNumber,
                        formatNumber,
                      )}
                    </div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">{t('catalog.column.duration')}</div>
                    <div>
                      {detail.durationMs != null ? formatDuration(detail.durationMs) : t('common.none')}
                    </div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">{t('catalog.column.release')}</div>
                    <div>
                      <ReleaseLink id={detail.releaseId} name={detail.releaseTitle} />
                    </div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">{t('common.created')}</div>
                    <div>{formatDateTime(detail.createdAt)}</div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">{t('common.updated')}</div>
                    <div>{formatDateTime(detail.updatedAt)}</div>
                  </div>
                </div>
                {detail.sourcePath ? (
                  <p className="melodee-review-track__file" style={{ marginBlockStart: '0.5rem' }}>
                    {detail.sourcePath}
                  </p>
                ) : null}
                {detail.streamUrl ? (
                  // Same-origin signed URL on the native stream route; used as-is.
                  <div style={{ marginBlockStart: '0.5rem' }}>
                    <AudioPreview
                      src={detail.streamUrl}
                      label={t('catalog.track.previewLabel', { title: detail.title })}
                    />
                  </div>
                ) : null}
              </div>
            </div>
          </div>

          <div className="melodee-review-card">
            <Heading className="melodee-panel__title">{t('catalog.track.artists.title')}</Heading>
            {detail.artists.length === 0 ? (
              <EmptyState title={t('catalog.track.artists.empty')} compact />
            ) : (
              <div className="melodee-review-table-scroll">
                <TableContainer title={t('catalog.track.artists.caption')}>
                  <Table size="sm" aria-label={t('catalog.track.artists.caption')}>
                    <TableHead>
                      <TableRow>
                        <TableHeader>{t('catalog.column.name')}</TableHeader>
                        <TableHeader>{t('catalog.column.role')}</TableHeader>
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
          </div>

          <div className="melodee-review-card">
            <Heading className="melodee-panel__title">{t('catalog.track.contributors.title')}</Heading>
            {detail.contributors.items.length === 0 ? (
              <EmptyState title={t('catalog.track.contributors.empty')} compact />
            ) : (
              <div className="melodee-review-table-scroll">
                <TableContainer title={t('catalog.track.contributors.caption')}>
                  <Table size="sm" aria-label={t('catalog.track.contributors.caption')}>
                    <TableHead>
                      <TableRow>
                        <TableHeader>{t('catalog.column.name')}</TableHeader>
                        <TableHeader>{t('catalog.column.role')}</TableHeader>
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
                          <TableCell>{contributor.evidenceSource}</TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </TableContainer>
              </div>
            )}
          </div>

          <div className="melodee-review-card">
            <Heading className="melodee-panel__title">{t('catalog.track.files.title')}</Heading>
            {detail.mediaAssets.length === 0 ? (
              <EmptyState title={t('catalog.track.files.empty')} compact />
            ) : (
              <div className="melodee-review-table-scroll">
                <TableContainer title={t('catalog.track.files.caption')}>
                  <Table size="sm" aria-label={t('catalog.track.files.caption')}>
                    <TableHead>
                      <TableRow>
                        <TableHeader>{t('catalog.track.files.column.purpose')}</TableHeader>
                        <TableHeader>{t('catalog.release.files.column.path')}</TableHeader>
                        <TableHeader>{t('catalog.column.format')}</TableHeader>
                        <TableHeader>{t('catalog.column.size')}</TableHeader>
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {detail.mediaAssets.map((asset) => (
                        <TableRow key={`${asset.id}-${asset.purpose}`}>
                          <TableCell>{asset.purpose}</TableCell>
                          <TableCell>{asset.canonicalRelativePath}</TableCell>
                          <TableCell>
                            {[asset.container, asset.codec].filter(Boolean).join(' / ') ||
                              asset.mimeType}
                          </TableCell>
                          <TableCell>{formatBytes(asset.byteSize)}</TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </TableContainer>
              </div>
            )}
          </div>
        </>
      ) : null}
    </div>
  );
}
