import { InlineNotification, Modal, TextArea, TextInput } from '@carbon/react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { formatBytes, formatNumber } from '../i18n/format';

/**
 * Destructive action gate (UI-SPEC §6, PRD-UI-008). Shows affected counts,
 * storage impact, the matched rule, and (unless `hideAlternativeNote`) the
 * quarantine/ignore alternative; requires an operator reason; delete-class
 * actions additionally require the typed confirmation phrase before the
 * danger button enables.
 */
export interface DestructiveActionDialogProps {
  open: boolean;
  title: string;
  actionLabel: string;
  /** Delete-class actions require typed confirmation. */
  deleteClass?: boolean;
  /** Typed-confirmation phrase override (default: destructive.confirmPhrase). */
  confirmPhrase?: string;
  affectedReleases?: number;
  affectedTracks?: number;
  affectedFiles?: number;
  storageImpactBytes?: number;
  matchedRule?: string;
  /** Hide the quarantine/ignore alternative note when the action is not a
   *  processing-queue disposition (e.g. artwork override removal). */
  hideAlternativeNote?: boolean;
  onClose: () => void;
  onConfirm: (reason: string) => void;
}

export function DestructiveActionDialog({
  open,
  title,
  actionLabel,
  deleteClass = false,
  confirmPhrase,
  affectedReleases,
  affectedTracks,
  affectedFiles,
  storageImpactBytes,
  matchedRule,
  hideAlternativeNote = false,
  onClose,
  onConfirm,
}: DestructiveActionDialogProps) {
  const { t } = useTranslation();
  const [reason, setReason] = useState('');
  const [confirmation, setConfirmation] = useState('');

  const resetAndClose = () => {
    setReason('');
    setConfirmation('');
    onClose();
  };

  const phrase = confirmPhrase ?? t('destructive.confirmPhrase');
  const reasonValid = reason.trim().length > 0;
  const confirmationValid = !deleteClass || confirmation.trim() === phrase;
  const canConfirm = reasonValid && confirmationValid;

  const counts: [string, number | undefined][] = [
    [t('destructive.affectedReleases'), affectedReleases],
    [t('destructive.affectedTracks'), affectedTracks],
    [t('destructive.affectedFiles'), affectedFiles],
  ];

  return (
    <Modal
      open={open}
      danger
      modalHeading={title}
      primaryButtonText={actionLabel}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={!canConfirm}
      onRequestClose={resetAndClose}
      onRequestSubmit={() => {
        const submitted = reason.trim();
        setReason('');
        setConfirmation('');
        onConfirm(submitted);
      }}
    >
      <dl>
        {counts.map(([label, value]) =>
          value !== undefined ? (
            <div key={label} style={{ display: 'flex', gap: '0.5rem' }}>
              <dt>{label}</dt>
              <dd style={{ marginInlineStart: 0 }}>{formatNumber(value)}</dd>
            </div>
          ) : null,
        )}
        {storageImpactBytes !== undefined ? (
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            <dt>{t('destructive.storageImpact')}</dt>
            <dd style={{ marginInlineStart: 0 }}>{formatBytes(storageImpactBytes)}</dd>
          </div>
        ) : null}
        {matchedRule ? (
          <div style={{ display: 'flex', gap: '0.5rem' }}>
            <dt>{t('destructive.matchedRule')}</dt>
            <dd style={{ marginInlineStart: 0 }}>{matchedRule}</dd>
          </div>
        ) : null}
      </dl>
      {hideAlternativeNote ? null : (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title={t('destructive.alternativeNote')}
          style={{ marginBlockStart: '1rem', marginBlockEnd: '1rem' }}
        />
      )}
      <TextArea
        id="destructive-reason"
        labelText={t('destructive.reasonLabel')}
        value={reason}
        invalid={!reasonValid && reason.length > 0}
        invalidText={t('destructive.reasonRequired')}
        onChange={(event) => setReason(event.target.value)}
      />
      {deleteClass ? (
        <TextInput
          id="destructive-confirm"
          labelText={t('destructive.typeToConfirm', { phrase })}
          value={confirmation}
          onChange={(event) => setConfirmation(event.target.value)}
          style={{ marginBlockStart: '1rem' }}
        />
      ) : null}
    </Modal>
  );
}
