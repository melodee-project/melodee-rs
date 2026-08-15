import { useRef, useState } from 'react';

import { Button, InlineNotification } from '@carbon/react';
import { useTranslation } from 'react-i18next';

import {
  CATALOG_ARTWORK_MAX_BYTES,
  CATALOG_ARTWORK_TYPES,
  removeCatalogImage,
  uploadCatalogImage,
} from '../../api/admin';
import { ApiError } from '../../api/client';
import { DestructiveActionDialog } from '../../components/DestructiveActionDialog';
import { formatBytes } from '../../i18n/format';
import { CatalogArtwork } from './CatalogArtwork';

/**
 * Catalog artwork override management (ADR-0143, parity matrix artwork row):
 * shows the current artwork served by the frozen native image endpoint,
 * uploads a jpeg/png replacement, and removes the override behind a
 * DestructiveActionDialog-consistent confirm. Removal only deletes the
 * override row — the plan/placeholder fallback returns and canonical media
 * is never touched. Client-side type/size checks pre-validate against the
 * documented server bounds; server 400/404 surface as localized errors.
 */
export function ArtworkManager({
  entity,
  entityId,
  imageId,
  name,
  onChanged,
}: {
  entity: 'artists' | 'releases';
  entityId: string;
  /** Frozen-endpoint image id (`artist_…` / `album_…`) from the detail DTO. */
  imageId: string;
  name: string;
  /** Re-fetches the parent detail after a committed mutation. */
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  const [file, setFile] = useState<File | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);

  const describeServerError = (cause: unknown): string => {
    if (cause instanceof ApiError) {
      if (cause.status === 400) {
        return t('catalog.artwork.error.invalid');
      }
      if (cause.status === 404) {
        return t('catalog.artwork.error.notFound');
      }
    }
    return t('catalog.artwork.error.generic');
  };

  const onFileChosen = (chosen: File | null) => {
    setError(null);
    if (!chosen) {
      setFile(null);
      return;
    }
    if (!CATALOG_ARTWORK_TYPES.includes(chosen.type)) {
      setFile(null);
      setError(t('catalog.artwork.error.type'));
      return;
    }
    if (chosen.size === 0 || chosen.size > CATALOG_ARTWORK_MAX_BYTES) {
      setFile(null);
      setError(t('catalog.artwork.error.size', { max: formatBytes(CATALOG_ARTWORK_MAX_BYTES) }));
      return;
    }
    setFile(chosen);
  };

  const upload = async () => {
    if (!file || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await uploadCatalogImage(entity, entityId, file);
      setFile(null);
      if (inputRef.current) {
        inputRef.current.value = '';
      }
      setRefreshKey((key) => key + 1);
      onChanged();
    } catch (cause) {
      setError(describeServerError(cause));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (reason: string) => {
    setConfirmOpen(false);
    setBusy(true);
    setError(null);
    try {
      await removeCatalogImage(entity, entityId, reason);
      setRefreshKey((key) => key + 1);
      onChanged();
    } catch (cause) {
      setError(describeServerError(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <CatalogArtwork
        imageId={imageId}
        name={name}
        size={192}
        imageSize="medium"
        cacheKey={refreshKey}
      />
      <p className="melodee-panel__subtitle" style={{ marginBlockStart: '0.5rem' }}>
        {t('catalog.artwork.hint', { max: formatBytes(CATALOG_ARTWORK_MAX_BYTES) })}
      </p>
      {error ? (
        <InlineNotification
          kind="error"
          lowContrast
          hideCloseButton
          title={t('catalog.artwork.error.title')}
          subtitle={error}
        />
      ) : null}
      <div
        style={{
          display: 'flex',
          flexWrap: 'wrap',
          gap: '0.75rem',
          alignItems: 'center',
          marginBlockStart: '0.75rem',
        }}
      >
        <input
          ref={inputRef}
          type="file"
          accept="image/jpeg,image/png"
          aria-label={t('catalog.artwork.chooseFile')}
          onChange={(event) => onFileChosen(event.target.files?.[0] ?? null)}
        />
        <Button size="sm" disabled={!file || busy} onClick={() => void upload()}>
          {t('catalog.artwork.upload')}
        </Button>
        <Button
          size="sm"
          kind="danger--tertiary"
          disabled={busy}
          onClick={() => setConfirmOpen(true)}
        >
          {t('catalog.artwork.remove')}
        </Button>
      </div>
      {file ? (
        <p className="melodee-review-fact__label" style={{ marginBlockStart: '0.5rem' }}>
          {t('catalog.artwork.selected', { name: file.name, size: formatBytes(file.size) })}
        </p>
      ) : null}
      <DestructiveActionDialog
        open={confirmOpen}
        title={t('catalog.artwork.removeTitle')}
        actionLabel={t('catalog.artwork.remove')}
        hideAlternativeNote
        onClose={() => setConfirmOpen(false)}
        onConfirm={(reason) => void remove(reason)}
      />
    </div>
  );
}
