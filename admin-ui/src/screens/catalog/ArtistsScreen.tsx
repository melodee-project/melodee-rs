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
  batchMergeArtists,
  listCatalogArtists,
  previewArtistMerge,
} from '../../api/admin';
import type { ArtistMergePreview, CatalogArtistListItem } from '../../api/types';
import { ArtistLink } from '../../components/EntityLink';
import type { EntityColumn } from '../../components/EntityTable';
import { StatusTag } from '../../components/StatusTag';
import { formatDateTime, formatNumber, formatRelativeTime } from '../../i18n/format';
import { catalogStatusKind, toApiError } from './catalog';
import { CatalogListScreen, CellInteractive } from './CatalogListScreen';

/**
 * Catalog Artists browsing (PRD-UI-005/010): server-side paged table
 * over GET /api/admin/v1/catalog/artists with the backend sort whitelist
 * (name/createdAt/updatedAt) and substring `query` filter.
 */
const SORT_KEYS = ['name', 'createdAt', 'updatedAt'];

interface MergeArtistsModalProps {
  artists: CatalogArtistListItem[];
  onClose: () => void;
  onCompleted: () => void;
}

function MergeArtistsModal({ artists, onClose, onCompleted }: MergeArtistsModalProps) {
  const { t } = useTranslation();
  const [targetId, setTargetId] = useState(artists[0]?.id ?? '');
  const [reason, setReason] = useState('');
  const [confirmation, setConfirmation] = useState('');
  const [preview, setPreview] = useState<ArtistMergePreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [loadingPreview, setLoadingPreview] = useState(true);
  const [busy, setBusy] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  const sourceIds = useMemo(
    () => artists.filter((artist) => artist.id !== targetId).map((artist) => artist.id),
    [artists, targetId],
  );

  useEffect(() => {
    let cancelled = false;
    previewArtistMerge({ sourceArtistIds: sourceIds, targetArtistId: targetId })
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
    batchMergeArtists({
      sourceArtistIds: sourceIds,
      targetArtistId: targetId,
      reason: reason.trim(),
    })
      .then(onCompleted)
      .catch((cause: unknown) => setSubmitError(toApiError(cause).message))
      .finally(() => setBusy(false));
  };

  const impact = preview?.impact;
  return (
    <Modal
      open
      size="lg"
      danger
      modalHeading={t('catalog.artists.merge.title')}
      primaryButtonText={t('catalog.artists.merge.action')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={!canSubmit}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      <p>{t('catalog.artists.merge.description', { count: artists.length })}</p>
      <InlineNotification
        kind="warning"
        lowContrast
        hideCloseButton
        title={t('catalog.artists.merge.targetWinsTitle')}
        subtitle={t('catalog.artists.merge.targetWinsDescription')}
      />
      {submitError ? (
        <InlineNotification kind="error" lowContrast hideCloseButton title={submitError} />
      ) : null}
      <Select
        id="artist-merge-target"
        labelText={t('catalog.artists.merge.targetLabel')}
        value={targetId}
        onChange={(event) => {
          setTargetId(event.target.value);
          setLoadingPreview(true);
          setPreview(null);
          setPreviewError(null);
        }}
      >
        {artists.map((artist) => (
          <SelectItem
            key={artist.id}
            value={artist.id}
            text={`${artist.name}${artist.disambiguation ? ` — ${artist.disambiguation}` : ''}`}
          />
        ))}
      </Select>
      <TextArea
        id="artist-merge-reason"
        labelText={t('identity.decision.reasonLabel')}
        value={reason}
        onChange={(event) => setReason(event.target.value)}
        style={{ marginBlockStart: '1rem' }}
      />
      {loadingPreview ? <p role="status">{t('catalog.artists.merge.loadingPreview')}</p> : null}
      {previewError ? (
        <InlineNotification
          kind="error"
          lowContrast
          hideCloseButton
          title={t('catalog.artists.merge.previewError')}
          subtitle={previewError}
        />
      ) : null}
      {impact ? (
        <section aria-label={t('catalog.artists.merge.impactTitle')}>
          <h3>{t('catalog.artists.merge.impactTitle')}</h3>
          <dl className="melodee-definition-list">
            <div>
              <dt>{t('catalog.artists.merge.sources')}</dt>
              <dd>{formatNumber(preview.sources.length)}</dd>
            </div>
            <div>
              <dt>{t('catalog.column.releases')}</dt>
              <dd>{formatNumber(impact.releases)}</dd>
            </div>
            <div>
              <dt>{t('catalog.column.tracks')}</dt>
              <dd>{formatNumber(impact.tracks)}</dd>
            </div>
            <div>
              <dt>{t('catalog.artists.merge.credits')}</dt>
              <dd>
                {formatNumber(
                  impact.releaseCredits + impact.trackCredits + impact.contributions,
                )}
              </dd>
            </div>
            <div>
              <dt>{t('catalog.artists.merge.userStates')}</dt>
              <dd>{formatNumber(impact.userStates)}</dd>
            </div>
            <div>
              <dt>{t('catalog.artists.merge.images')}</dt>
              <dd>{formatNumber(impact.operatorImages + impact.providerImages)}</dd>
            </div>
            <div>
              <dt>{t('catalog.artists.merge.providerData')}</dt>
              <dd>{formatNumber(impact.providerCandidates + impact.providerTasks)}</dd>
            </div>
            <div>
              <dt>{t('catalog.artists.merge.metadataConflicts')}</dt>
              <dd>{formatNumber(preview.metadataConflicts.length)}</dd>
            </div>
          </dl>
          <p>{t('catalog.artists.merge.mediaUnchanged')}</p>
        </section>
      ) : null}
      <TextInput
        id="artist-merge-confirmation"
        labelText={t('destructive.typeToConfirm', { phrase: 'MERGE' })}
        value={confirmation}
        onChange={(event) => setConfirmation(event.target.value)}
        style={{ marginBlockStart: '1rem' }}
      />
    </Modal>
  );
}

export function ArtistsScreen() {
  const { t } = useTranslation();
  const [items, setItems] = useState<CatalogArtistListItem[]>([]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [mergeOpen, setMergeOpen] = useState(false);
  const [mergeCompleted, setMergeCompleted] = useState(false);
  const [refreshToken, setRefreshToken] = useState(0);

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: 'name', header: t('catalog.column.name'), sortable: true },
      { key: 'artistType', header: t('catalog.column.type') },
      { key: 'status', header: t('catalog.column.status') },
      { key: 'releaseCount', header: t('catalog.column.releases') },
      { key: 'trackCount', header: t('catalog.column.tracks') },
      { key: 'updatedAt', header: t('catalog.column.updated'), sortable: true },
    ],
    [t],
  );

  const toRow = useCallback(
    (item: CatalogArtistListItem) => ({
      id: item.id,
      cells: {
        name: (
          <CellInteractive>
            <ArtistLink id={item.id} name={item.name} />
            {item.disambiguation ? (
              <span className="melodee-work-item__id">{item.disambiguation}</span>
            ) : null}
          </CellInteractive>
        ),
        artistType: t(`catalog.artistType.${item.artistType}`, { defaultValue: item.artistType }),
        status:
          item.status === 'active' ? (
            // Active is the normal state of every imported Artist, so a chip on
            // every row is noise. Verified/merged/deprecated are the states an
            // operator needs to spot.
            <span className="melodee-text-muted" title={t('catalog.artists.activeHint')}>
              {t('catalog.status.active')}
            </span>
          ) : (
            <StatusTag
              kind={catalogStatusKind(item.status)}
              label={t(`catalog.status.${item.status}`, { defaultValue: item.status })}
            />
          ),
        releaseCount: formatNumber(item.releaseCount),
        trackCount: formatNumber(item.trackCount),
        updatedAt: (
          <span title={formatDateTime(item.updatedAt)}>{formatRelativeTime(item.updatedAt)}</span>
        ),
      },
    }),
    [t],
  );

  const onItemsChange = useCallback((nextItems: CatalogArtistListItem[]) => {
    setItems(nextItems);
    const visible = new Set(nextItems.map((item) => item.id));
    setSelectedIds((current) => current.filter((id) => visible.has(id)));
  }, []);
  const selectedArtists = items.filter((item) => selectedIds.includes(item.id));

  return (
    <>
      <CatalogListScreen
        titleKey="catalog.artists.title"
        subtitleKey="catalog.artists.subtitle"
        captionKey="catalog.artists.tableCaption"
        searchLabelKey="catalog.artists.searchLabel"
        emptyTitleKey="catalog.artists.emptyTitle"
        emptyDescriptionKey="catalog.artists.emptyDescription"
        sortKeys={SORT_KEYS}
        fetchPage={listCatalogArtists}
        columns={columns}
        toRow={toRow}
        detailPath={(item) => `/catalog/artists/${encodeURIComponent(item.id)}`}
        selection={{ selectedIds, onChange: setSelectedIds }}
        toolbarActions={
          <Button
            kind="secondary"
            size="sm"
            disabled={selectedIds.length < 2}
            onClick={() => {
              setMergeCompleted(false);
              setMergeOpen(true);
            }}
          >
            {t('catalog.artists.merge.selectedAction', { count: selectedIds.length })}
          </Button>
        }
        notice={
          mergeCompleted ? (
            <InlineNotification
              kind="success"
              lowContrast
              hideCloseButton
              title={t('catalog.artists.merge.success')}
            />
          ) : undefined
        }
        refreshToken={refreshToken}
        onItemsChange={onItemsChange}
      />
      {mergeOpen ? (
        <MergeArtistsModal
          artists={selectedArtists}
          onClose={() => setMergeOpen(false)}
          onCompleted={() => {
            setMergeOpen(false);
            setMergeCompleted(true);
            setSelectedIds([]);
            setRefreshToken((current) => current + 1);
          }}
        />
      ) : null}
    </>
  );
}
