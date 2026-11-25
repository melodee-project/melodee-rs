import { InlineNotification, Loading, Modal } from '@carbon/react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatBytes, formatNumber } from '../i18n/format';
import type { CatalogDeletePreview } from '../screens/catalog/catalogDelete';

export interface CatalogDeleteDialogProps {
  kind: 'artist' | 'release' | 'track';
  count: number;
  preview: () => Promise<CatalogDeletePreview>;
  onSubmit: () => Promise<void>;
  onClose: () => void;
}

/**
 * Confirmation-only hard-deletion dialog (ADR-0211). The operator sees the
 * exact impact and confirms; there is no reason field, scope choice, typed
 * phrase, workflow stage, or second dialog. Every selected catalog row and
 * every file it owns is removed.
 */
export function CatalogDeleteDialog({
  kind,
  count,
  preview,
  onSubmit,
  onClose,
}: CatalogDeleteDialogProps) {
  const { t } = useTranslation();
  const [data, setData] = useState<CatalogDeletePreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    preview()
      .then((result) => {
        if (!cancelled) {
          setData(result);
        }
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setError(cause instanceof Error ? cause.message : String(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [preview]);

  const submit = async () => {
    setBusy(true);
    setSubmitError(null);
    try {
      await onSubmit();
    } catch (cause) {
      setSubmitError(cause instanceof Error ? cause.message : String(cause));
      setBusy(false);
    }
  };

  const title = t(`catalog.${kind}.delete.title`);
  const mediaBlocked =
    data !== null &&
    data.impact.mediaFiles > 0 &&
    !data.storageConfigured;

  return (
    <Modal
      open
      danger
      modalHeading={title}
      primaryButtonText={busy ? t('common.deleting') : t('catalog.delete.confirmAction')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={busy || data === null || mediaBlocked || error !== null}
      onRequestClose={onClose}
      onRequestSubmit={() => void submit()}
    >
      <p>{t('catalog.delete.confirmBody', { count, kind: t(`catalog.delete.kind.${kind}`) })}</p>
      {error ? (
        <InlineNotification
          kind="error"
          lowContrast
          hideCloseButton
          title={t('catalog.delete.previewError')}
          subtitle={error}
        />
      ) : null}
      {!data && !error ? (
        <Loading description={t('catalog.delete.loadingImpact')} withOverlay={false} />
      ) : null}
      {data ? (
        <section aria-label={t('catalog.delete.impactTitle')}>
          <h3>{t('catalog.delete.impactTitle')}</h3>
          <dl className="melodee-definition-list">
            <div>
              <dt>{t('catalog.delete.affectedReleases')}</dt>
              <dd>{formatNumber(data.impact.releases)}</dd>
            </div>
            <div>
              <dt>{t('catalog.delete.affectedTracks')}</dt>
              <dd>{formatNumber(data.impact.tracks)}</dd>
            </div>
            <div>
              <dt>{t('catalog.delete.affectedFiles')}</dt>
              <dd>{formatNumber(data.impact.mediaFiles)}</dd>
            </div>
            <div>
              <dt>{t('catalog.delete.storageImpact')}</dt>
              <dd>{formatBytes(data.impact.mediaBytes)}</dd>
            </div>
            {data.impact.images > 0 ? (
              <div>
                <dt>{t('catalog.delete.affectedImages')}</dt>
                <dd>{formatNumber(data.impact.images)}</dd>
              </div>
            ) : null}
          </dl>
          {mediaBlocked ? (
            <InlineNotification
              kind="error"
              lowContrast
              hideCloseButton
              title={t('catalog.delete.storageUnavailable')}
            />
          ) : null}
          {data.impact.retainedSharedReleases > 0 ? (
            <InlineNotification
              kind="info"
              lowContrast
              hideCloseButton
              title={t('catalog.delete.retainedShared', {
                count: data.impact.retainedSharedReleases,
              })}
            />
          ) : null}
          {data.warnings.length > 0 ? (
            <ul>
              {data.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          ) : null}
        </section>
      ) : null}
      {submitError ? (
        <InlineNotification kind="error" lowContrast hideCloseButton title={submitError} />
      ) : null}
    </Modal>
  );
}
