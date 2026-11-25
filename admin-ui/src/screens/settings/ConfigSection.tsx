import { Download } from '@carbon/icons-react';
import {
  ActionableNotification,
  Button,
  Checkbox,
  FileUploaderButton,
  Heading,
  InlineNotification,
  Loading,
  Modal,
} from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { exportConfig, importConfig } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { ConfigExport, ConfigImportResult } from '../../api/types';
import { SettingsEditor } from './SettingsEditor';
import { formatNumber } from '../../i18n/format';

interface Notice {
  kind: 'success' | 'error' | 'info' | 'warning';
  text: string;
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

/** Localized text for the stable import error codes. */
function importErrorText(
  error: ApiError,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (error.status === 400 && error.code === 'redacted_marker_rejected') {
    return t('settings.import.error.redactedMarker');
  }
  if (error.status === 400 && error.code === 'invalid_document') {
    return t('settings.import.error.invalidDocument', { message: error.message });
  }
  if (error.status === 400 && error.code === 'invalid_value') {
    return t('settings.import.error.invalidValue', { message: error.message });
  }
  if (error.status === 400 && error.code === 'invalid_key') {
    return t('settings.import.error.invalidKey', { message: error.message });
  }
  return t('settings.import.error.generic', { message: error.message });
}

export function ConfigSection() {
  const { t } = useTranslation();
  const [document, setDocument] = useState<ConfigExport | null>(null);
  const [loadError, setLoadError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [revision, setRevision] = useState(0);

  const [pendingDocument, setPendingDocument] = useState<unknown>(null);
  const [pendingName, setPendingName] = useState<string | null>(null);
  const [preview, setPreview] = useState<ConfigImportResult | null>(null);
  const [allowUnknown, setAllowUnknown] = useState(false);
  const [importBusy, setImportBusy] = useState(false);
  const [applyConfirmOpen, setApplyConfirmOpen] = useState(false);

  const fetchExport = useCallback((signal: { cancelled: boolean }) => {
    return exportConfig()
      .then((result) => {
        if (!signal.cancelled) {
          setDocument(result);
          setLoadError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setLoadError(toApiError(cause));
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchExport(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchExport]);

  const downloadExport = () => {
    if (!document) {
      return;
    }
    const blob = new Blob([JSON.stringify(document, null, 2)], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const anchor = window.document.createElement('a');
    anchor.href = url;
    anchor.download = 'melodee-config-export.json';
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const runPreview = (doc: unknown, allowUnknownKeys: boolean) => {
    setImportBusy(true);
    setNotice(null);
    importConfig(doc, { preview: true, allowUnknown: allowUnknownKeys })
      .then((result) => {
        setPreview(result);
      })
      .catch((cause: unknown) => {
        setPreview(null);
        setNotice({ kind: 'error', text: importErrorText(toApiError(cause), t) });
      })
      .finally(() => setImportBusy(false));
  };

  const onFileSelected = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    // Reset the input so selecting the same file twice re-triggers.
    event.target.value = '';
    if (!file) {
      return;
    }
    file
      .text()
      .then((text) => {
        let parsed: unknown;
        try {
          parsed = JSON.parse(text);
        } catch {
          setPendingDocument(null);
          setPreview(null);
          setNotice({ kind: 'error', text: t('settings.import.error.notJson') });
          return;
        }
        setPendingDocument(parsed);
        setPendingName(file.name);
        setAllowUnknown(false);
        runPreview(parsed, false);
      })
      .catch(() => setNotice({ kind: 'error', text: t('settings.import.error.notJson') }));
  };

  const applyImport = () => {
    if (pendingDocument === null) {
      return;
    }
    setImportBusy(true);
    importConfig(pendingDocument, { preview: false, allowUnknown })
      .then((result) => {
        setPreview(result);
        setNotice({ kind: 'success', text: t('settings.import.applied') });
        setRevision((previous) => previous + 1);
        void fetchExport({ cancelled: false });
      })
      .catch((cause: unknown) => {
        setNotice({ kind: 'error', text: importErrorText(toApiError(cause), t) });
      })
      .finally(() => {
        setImportBusy(false);
        setApplyConfirmOpen(false);
      });
  };

  return (
    <div>
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      {document ? (
        <InlineNotification
          kind="warning"
          lowContrast
          title={t('settings.exportWarningTitle')}
          subtitle={document.warning}
          hideCloseButton
        />
      ) : null}
      {loadError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('settings.loadErrorTitle')}
          subtitle={loadError.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => void fetchExport({ cancelled: false })}
        />
      ) : null}

      <div className="melodee-panel__footer" style={{ marginBlock: '1rem' }}>
        <Button
          kind="secondary"
          size="sm"
          renderIcon={Download}
          disabled={!document}
          onClick={downloadExport}
        >
          {t('settings.exportConfiguration')}
        </Button>
        <FileUploaderButton
          labelText={t('settings.import.chooseFile')}
          accept={['application/json', '.json']}
          buttonKind="primary"
          size="sm"
          onChange={onFileSelected}
        />
      </div>

      {pendingDocument !== null ? (
        <section className="melodee-panel" aria-labelledby="config-import-preview-title">
          <Heading id="config-import-preview-title" className="melodee-panel__title">
            {t('settings.import.previewTitle', { file: pendingName ?? '' })}
          </Heading>
          {importBusy && !preview ? (
            <Loading description={t('common.loading')} withOverlay={false} />
          ) : null}
          {preview ? (
            <>
              <dl className="melodee-fact-list">
                <div className="melodee-fact">
                  <dt>{t('settings.import.added')}</dt>
                  <dd>{formatNumber(preview.added.length)}</dd>
                </div>
                <div className="melodee-fact">
                  <dt>{t('settings.import.changed')}</dt>
                  <dd>{formatNumber(preview.changed.length)}</dd>
                </div>
                <div className="melodee-fact">
                  <dt>{t('settings.import.unchanged')}</dt>
                  <dd>{formatNumber(preview.unchanged.length)}</dd>
                </div>
                <div className="melodee-fact">
                  <dt>{t('settings.import.secretsReentered')}</dt>
                  <dd>{formatNumber(preview.secretsReentered.length)}</dd>
                </div>
                <div className="melodee-fact">
                  <dt>{t('settings.import.secretsSkipped')}</dt>
                  <dd>{formatNumber(preview.secretsSkipped.length)}</dd>
                </div>
                <div className="melodee-fact">
                  <dt>{t('settings.import.unknown')}</dt>
                  <dd>{formatNumber(preview.unknown.length)}</dd>
                </div>
              </dl>
              <PlanKeyList title={t('settings.import.added')} keys={preview.added} />
              <PlanKeyList title={t('settings.import.changed')} keys={preview.changed} />
              <PlanKeyList
                title={t('settings.import.secretsSkipped')}
                keys={preview.secretsSkipped}
                note={t('settings.import.secretsSkippedNote')}
              />
              <PlanKeyList title={t('settings.import.unknown')} keys={preview.unknown} />
              {preview.warnings.length > 0 ? (
                <InlineNotification
                  kind="warning"
                  lowContrast
                  title={t('settings.import.warningsTitle')}
                  subtitle={preview.warnings.join('; ')}
                  hideCloseButton
                />
              ) : null}
              {preview.unknown.length > 0 ? (
                <Checkbox
                  id="config-import-allow-unknown"
                  labelText={t('settings.import.allowUnknown')}
                  checked={allowUnknown}
                  onChange={(_event, { checked }) => {
                    setAllowUnknown(checked);
                    runPreview(pendingDocument, checked);
                  }}
                />
              ) : null}
              <div className="melodee-panel__footer">
                <Button
                  kind="primary"
                  size="sm"
                  disabled={importBusy || preview.applied}
                  onClick={() => setApplyConfirmOpen(true)}
                >
                  {t('settings.import.apply')}
                </Button>
                <Button
                  kind="ghost"
                  size="sm"
                  onClick={() => {
                    setPendingDocument(null);
                    setPreview(null);
                    setPendingName(null);
                  }}
                >
                  {t('settings.import.discard')}
                </Button>
              </div>
            </>
          ) : null}
        </section>
      ) : null}

      <SettingsEditor revision={revision} onSaved={() => void fetchExport({ cancelled: false })} />

      {applyConfirmOpen ? (
        <Modal
          open
          modalHeading={t('settings.import.applyConfirmTitle')}
          primaryButtonText={t('settings.import.apply')}
          secondaryButtonText={t('actions.cancel')}
          primaryButtonDisabled={importBusy}
          onRequestClose={() => setApplyConfirmOpen(false)}
          onRequestSubmit={applyImport}
        >
          <p>
            {t('settings.import.applyConfirmBody', {
              added: formatNumber(preview?.added.length ?? 0),
              changed: formatNumber(preview?.changed.length ?? 0),
            })}
          </p>
        </Modal>
      ) : null}
    </div>
  );
}

/** One plan category rendered by key name only (secret values never render). */
function PlanKeyList({
  title,
  keys,
  note,
}: {
  title: string;
  keys: string[];
  note?: string;
}) {
  if (keys.length === 0) {
    return null;
  }
  return (
    <div>
      <Heading className="melodee-panel__subtitle">{title}</Heading>
      {note ? <p className="melodee-panel__subtitle">{note}</p> : null}
      <ul>
        {keys.map((key) => (
          <li key={key}>
            <code>{key}</code>
          </li>
        ))}
      </ul>
    </div>
  );
}
