import { InlineNotification, Loading, Modal, RadioButton, RadioButtonGroup } from '@carbon/react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type {
  CatalogDeleteMode,
  CatalogDeletePreview,
} from '../screens/catalog/catalogDelete';
import { DestructiveActionDialog } from './DestructiveActionDialog';

export interface CatalogDeleteDialogProps {
  /** Entity noun used for the mode help and warning wording. */
  kind: 'artist' | 'release';
  count: number;
  preview: () => Promise<CatalogDeletePreview>;
  onSubmit: (mode: CatalogDeleteMode, reason: string) => Promise<void>;
  onClose: () => void;
}

/**
 * Two-step destructive catalog deletion (ADR-0199) shared by the Artist and
 * Release list bulk actions. The impact preview is scope-aware: the
 * catalog-only scope never deletes releases, tracks, or files, so those counts
 * are shown as zero until the operator selects the file-removing scope.
 */
export function CatalogDeleteDialog({
  kind,
  count,
  preview,
  onSubmit,
  onClose,
}: CatalogDeleteDialogProps) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<CatalogDeleteMode>('catalogOnly');
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

  if (error) {
    return (
      <Modal
        open
        danger
        modalHeading={t(`catalog.${kind}.delete.title`)}
        primaryButtonText={t('actions.close')}
        onRequestClose={onClose}
        onRequestSubmit={onClose}
      >
        <InlineNotification
          kind="error"
          lowContrast
          hideCloseButton
          title={t(`catalog.${kind}.delete.previewError`)}
          subtitle={error}
        />
      </Modal>
    );
  }

  if (!data) {
    return (
      <Modal
        open
        passiveModal
        modalHeading={t(`catalog.${kind}.delete.title`)}
        onRequestClose={onClose}
      >
        <Loading description={t(`catalog.${kind}.delete.loadingPreview`)} withOverlay={false} />
      </Modal>
    );
  }

  const removesContent = mode === 'releasesAndFiles';
  const submit = async (reason: string) => {
    setBusy(true);
    setSubmitError(null);
    try {
      await onSubmit(mode, reason);
    } catch (cause) {
      setSubmitError(cause instanceof Error ? cause.message : String(cause));
      setBusy(false);
    }
  };

  return (
    <DestructiveActionDialog
      open
      title={t(`catalog.${kind}.delete.title`)}
      actionLabel={busy ? t('common.deleting') : t(`catalog.${kind}.delete.action`)}
      deleteClass
      affectedReleases={removesContent ? data.impact.releases : 0}
      affectedTracks={removesContent ? data.impact.tracks : 0}
      affectedFiles={removesContent ? data.impact.files : 0}
      storageImpactBytes={removesContent ? data.impact.bytes : 0}
      hideAlternativeNote
      onClose={onClose}
      onConfirm={(reason) => void submit(reason)}
    >
      <p>{t(`catalog.${kind}.delete.description`, { count })}</p>
      <RadioButtonGroup
        legendText={t(`catalog.${kind}.delete.modeLabel`)}
        name={`catalog-${kind}-delete-mode`}
        orientation="vertical"
        valueSelected={mode}
        onChange={(value) => setMode(value as CatalogDeleteMode)}
      >
        <RadioButton
          id={`catalog-${kind}-delete-catalog`}
          value="catalogOnly"
          labelText={t(`catalog.${kind}.delete.modeCatalogOnly`)}
        />
        <RadioButton
          id={`catalog-${kind}-delete-content`}
          value="releasesAndFiles"
          labelText={t(`catalog.${kind}.delete.modeReleasesAndFiles`)}
          disabled={!data.storageConfigured}
        />
      </RadioButtonGroup>
      <p style={{ marginBlockEnd: '0.5rem', color: 'var(--cds-text-secondary)' }}>
        {mode === 'catalogOnly'
          ? t(`catalog.${kind}.delete.modeCatalogOnlyHelp`)
          : data.storageConfigured
            ? t(`catalog.${kind}.delete.modeReleasesAndFilesHelp`, { files: data.impact.files })
            : t(`catalog.${kind}.delete.modeReleasesAndFilesUnavailable`)}
      </p>
      {removesContent && data.storageReadyReleases !== data.impact.releases ? (
        <InlineNotification
          kind="warning"
          lowContrast
          hideCloseButton
          title={t(`catalog.${kind}.delete.storageEvidenceMissing`, {
            ready: data.storageReadyReleases,
            total: data.impact.releases,
          })}
        />
      ) : null}
      {data.impact.retainedSharedReleases ? (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title={t(`catalog.${kind}.delete.retainedShared`, {
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
      {submitError ? (
        <InlineNotification kind="error" lowContrast hideCloseButton title={submitError} />
      ) : null}
    </DestructiveActionDialog>
  );
}
