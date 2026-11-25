import {
  Button,
  InlineNotification,
  Modal,
  Select,
  SelectItem,
  TextArea,
  TextInput,
} from '@carbon/react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  batchMergeReleases,
  deleteReleases,
  listCatalogReleases,
  previewReleaseDelete,
  previewReleaseMerge,
} from '../../api/admin';
import type { CatalogReleaseListItem, ReleaseMergePreview } from '../../api/types';
import { useSessionRole } from '../../auth/AuthContext';
import { CatalogDeleteDialog } from '../../components/CatalogDeleteDialog';
import { ReleaseLink } from '../../components/EntityLink';
import type { EntityColumn } from '../../components/EntityTable';
import { StatusTag } from '../../components/StatusTag';
import { formatDate, formatDateTime, formatDuration, formatNumber, formatRelativeTime } from '../../i18n/format';
import { releaseStatusPresentation, toApiError } from './catalog';
import { releaseDeletePreviewView } from './catalogDelete';
import { CatalogListScreen, CellInteractive } from './CatalogListScreen';

/**
 * Catalog Releases browsing (PRD-UI-005/010): server-side paged table
 * over GET /api/admin/v1/catalog/releases with the backend sort whitelist
 * (title/releaseDate/createdAt/updatedAt) and substring `query` filter.
 *
 * The same table drives the multi-source Release merge (ADR-0198): any
 * Releases on the page — regardless of Artist — can be selected, one is chosen
 * as the survivor, and the rest fold into it with their Tracks, credits, and
 * artwork.
 */
const SORT_KEYS = ['title', 'releaseDate', 'createdAt', 'updatedAt'];

interface MergeReleasesModalProps {
  releases: CatalogReleaseListItem[];
  onClose: () => void;
  onCompleted: () => void;
}

function releaseLabel(release: CatalogReleaseListItem): string {
  return release.displayArtist
    ? `${release.displayArtist} — ${release.title}`
    : release.title;
}

function MergeReleasesModal({ releases, onClose, onCompleted }: MergeReleasesModalProps) {
  const { t } = useTranslation();
  const [targetId, setTargetId] = useState(releases[0]?.id ?? '');
  const [reason, setReason] = useState('');
  const [confirmation, setConfirmation] = useState('');
  const [preview, setPreview] = useState<ReleaseMergePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [loadingPreview, setLoadingPreview] = useState(true);
  const [busy, setBusy] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const sourceIds = useMemo(
    () => releases.filter((release) => release.id !== targetId).map((release) => release.id),
    [releases, targetId],
  );

  useEffect(() => {
    let cancelled = false;
    previewReleaseMerge({ sourceReleaseIds: sourceIds, targetReleaseId: targetId })
      .then((result) => {
        if (!cancelled) {
          setPreview(result);
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setPreviewError(toApiError(cause).message);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoadingPreview(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [sourceIds, targetId]);

  const canSubmit =
    preview !== null &&
    !loadingPreview &&
    !busy &&
    reason.trim().length > 0 &&
    confirmation.trim() === 'MERGE';
  const submit = () => {
    setBusy(true);
    setSubmitError(null);
    batchMergeReleases({
      sourceReleaseIds: sourceIds,
      targetReleaseId: targetId,
      reason: reason.trim(),
    })
      .then(onCompleted)
      .catch((cause: unknown) => setSubmitError(toApiError(cause).message))
      .finally(() => setBusy(false));
  };

  const impact = preview?.impact;
  const artwork = preview?.artwork;
  return (
    <Modal
      open
      size="lg"
      danger
      modalHeading={t('catalog.releases.merge.title')}
      primaryButtonText={t('catalog.releases.merge.action')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={!canSubmit}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      <p>{t('catalog.releases.merge.description', { count: releases.length })}</p>
      <InlineNotification
        kind="warning"
        lowContrast
        hideCloseButton
        title={t('catalog.releases.merge.targetWinsTitle')}
        subtitle={t('catalog.releases.merge.targetWinsDescription')}
      />
      {submitError ? (
        <InlineNotification kind="error" lowContrast hideCloseButton title={submitError} />
      ) : null}
      <Select
        id="release-merge-target"
        labelText={t('catalog.releases.merge.targetLabel')}
        value={targetId}
        onChange={(event) => {
          setTargetId(event.target.value);
          setLoadingPreview(true);
          setPreview(null);
          setPreviewError(null);
        }}
      >
        {releases.map((release) => (
          <SelectItem key={release.id} value={release.id} text={releaseLabel(release)} />
        ))}
      </Select>
      <TextArea
        id="release-merge-reason"
        labelText={t('identity.decision.reasonLabel')}
        value={reason}
        onChange={(event) => setReason(event.target.value)}
        style={{ marginBlockStart: '1rem' }}
      />
      {loadingPreview ? (
        <p role="status">{t('catalog.releases.merge.loadingPreview')}</p>
      ) : null}
      {previewError ? (
        <InlineNotification
          kind="error"
          lowContrast
          hideCloseButton
          title={t('catalog.releases.merge.previewError')}
          subtitle={previewError}
        />
      ) : null}
      {impact && artwork ? (
        <section aria-label={t('catalog.releases.merge.impactTitle')}>
          <h3>{t('catalog.releases.merge.impactTitle')}</h3>
          <dl className="melodee-definition-list">
            <div>
              <dt>{t('catalog.releases.merge.sources')}</dt>
              <dd>{formatNumber(preview.sources.length)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.tracks')}</dt>
              <dd>{formatNumber(impact.movedTracks)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.trackCountAfter')}</dt>
              <dd>{formatNumber(impact.targetTrackCountAfter)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.credits')}</dt>
              <dd>{formatNumber(impact.movedObservations + impact.movedContributions)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.sharedRecordings')}</dt>
              <dd>{formatNumber(impact.sharedAudioIdentities)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.userStates')}</dt>
              <dd>{formatNumber(impact.movedUserStates)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.images')}</dt>
              <dd>{formatNumber(artwork.operatorImagesAdopted)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.providerImages')}</dt>
              <dd>{formatNumber(artwork.providerImagesAdopted)}</dd>
            </div>
            <div>
              <dt>{t('catalog.releases.merge.metadataConflicts')}</dt>
              <dd>{formatNumber(preview.metadataConflicts.length)}</dd>
            </div>
          </dl>
          <p>{t('catalog.releases.merge.mediaUnchanged')}</p>
          {preview.warnings.length > 0 ? (
            <ul>
              {preview.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          ) : null}
        </section>
      ) : null}
      <TextInput
        id="release-merge-confirmation"
        labelText={t('destructive.typeToConfirm', { phrase: 'MERGE' })}
        value={confirmation}
        onChange={(event) => setConfirmation(event.target.value)}
        style={{ marginBlockStart: '1rem' }}
      />
    </Modal>
  );
}

export function ReleasesScreen() {
  const { t } = useTranslation();
  const role = useSessionRole();
  const isAdmin = role === 'admin';
  const [items, setItems] = useState<CatalogReleaseListItem[]>([]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [mergeOpen, setMergeOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [notice, setNotice] = useState<'merge' | 'delete' | null>(null);
  const [noticeDeleteCount, setNoticeDeleteCount] = useState(0);
  const [refreshToken, setRefreshToken] = useState(0);

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: 'title', header: t('catalog.column.title'), sortable: true },
      { key: 'displayArtist', header: t('catalog.column.artist') },
      { key: 'releaseDate', header: t('catalog.column.releaseDate'), sortable: true },
      { key: 'status', header: t('catalog.column.status') },
      { key: 'trackCount', header: t('catalog.column.tracks') },
      { key: 'durationMs', header: t('catalog.column.duration') },
      { key: 'updatedAt', header: t('catalog.column.updated'), sortable: true },
    ],
    [t],
  );

  const toRow = useCallback(
    (item: CatalogReleaseListItem) => {
      const status = releaseStatusPresentation(item);
      return {
        id: item.id,
        cells: {
        title: (
          <CellInteractive>
            <ReleaseLink id={item.id} name={item.title} />
          </CellInteractive>
        ),
        displayArtist: item.displayArtist ?? t('common.none'),
        releaseDate: item.releaseDate
          ? formatDate(item.releaseDate)
          : (item.releaseDateText ?? t('common.none')),
        status: (
          <StatusTag
            kind={status.kind}
            label={t(status.labelKey, { defaultValue: item.status })}
            description={status.description}
          />
        ),
        trackCount: formatNumber(item.trackCount),
        durationMs: formatDuration(item.durationMs),
        updatedAt: (
          <span title={formatDateTime(item.updatedAt)}>{formatRelativeTime(item.updatedAt)}</span>
        ),
        },
      };
    },
    [t],
  );

  const onItemsChange = useCallback((nextItems: CatalogReleaseListItem[]) => {
    setItems(nextItems);
    const visible = new Set(nextItems.map((item) => item.id));
    setSelectedIds((current) => current.filter((id) => visible.has(id)));
  }, []);
  const selectedReleases = items.filter((item) => selectedIds.includes(item.id));

  return (
    <>
      <CatalogListScreen
        titleKey="catalog.releases.title"
        subtitleKey="catalog.releases.subtitle"
        captionKey="catalog.releases.tableCaption"
        searchLabelKey="catalog.releases.searchLabel"
        emptyTitleKey="catalog.releases.emptyTitle"
        emptyDescriptionKey="catalog.releases.emptyDescription"
        sortKeys={SORT_KEYS}
        fetchPage={listCatalogReleases}
        columns={columns}
        toRow={toRow}
        detailPath={(item) => `/catalog/releases/${encodeURIComponent(item.id)}`}
        selection={{ selectedIds, onChange: setSelectedIds }}
        toolbarActions={
          <>
            {isAdmin ? <Button
              kind="secondary"
              size="sm"
              disabled={selectedIds.length < 2}
              onClick={() => {
                setNotice(null);
                setMergeOpen(true);
              }}
            >
              {t('catalog.releases.merge.selectedAction', { count: selectedIds.length })}
            </Button> : null}
            <Button
              kind="danger--tertiary"
              size="sm"
              disabled={selectedIds.length < 1}
              onClick={() => {
                setNotice(null);
                setDeleteOpen(true);
              }}
            >
              {t('catalog.releases.delete.selectedAction', { count: selectedIds.length })}
            </Button>
          </>
        }
        notice={
          notice === 'merge' ? (
            <InlineNotification
              kind="success"
              lowContrast
              hideCloseButton
              title={t('catalog.releases.merge.success')}
            />
          ) : notice === 'delete' ? (
            <InlineNotification
              kind="success"
              lowContrast
              hideCloseButton
              title={t('catalog.releases.delete.success', { count: noticeDeleteCount })}
            />
          ) : undefined
        }
        refreshToken={refreshToken}
        onItemsChange={onItemsChange}
      />
      {mergeOpen ? (
        <MergeReleasesModal
          releases={selectedReleases}
          onClose={() => setMergeOpen(false)}
          onCompleted={() => {
            setMergeOpen(false);
            setNotice('merge');
            setSelectedIds([]);
            setRefreshToken((current) => current + 1);
          }}
        />
      ) : null}
      {deleteOpen ? (
        <CatalogDeleteDialog
          kind="release"
          count={selectedIds.length}
          preview={async () => releaseDeletePreviewView(await previewReleaseDelete(selectedIds))}
          onSubmit={async () => {
            const result = await deleteReleases({ releaseIds: selectedIds });
            setDeleteOpen(false);
            setNoticeDeleteCount(result.deletedReleaseIds.length);
            setNotice('delete');
            setSelectedIds([]);
            setRefreshToken((current) => current + 1);
          }}
          onClose={() => setDeleteOpen(false)}
        />
      ) : null}
    </>
  );
}
