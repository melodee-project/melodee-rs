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
  Search,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
} from '@carbon/react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { exportConfig, importConfig } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { ConfigExport, ConfigImportResult } from '../../api/types';
import { StatusTag } from '../../components/StatusTag';
import { formatDateTime, formatNumber } from '../../i18n/format';

/**
 * Configuration browser + export/import (PRD-CONFIG-001/002).
 *
 * Honesty contract: the admin API exposes exactly two configuration
 * mutations — export and the preview-then-apply import flow. There is no
 * per-key mutation endpoint, so individual settings are read-only here and
 * the screen says so instead of faking inline editors. Secret values arrive
 * as the fixed redaction marker, render as a redacted tag, and are never
 * editable; re-entering secrets happens only through an import document the
 * operator supplies.
 */

/** Fixed marker substituted for every stored secret (melodee-persistence). */
const SECRET_REDACTION_MARKER = '__MELODEE_SECRET_REDACTED__';

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

interface SettingRow {
  key: string;
  value: unknown;
  redacted: boolean;
}

/** Groups registry keys by their first dotted segment for browsing. */
function groupSettings(settings: Record<string, unknown>): Map<string, SettingRow[]> {
  const groups = new Map<string, SettingRow[]>();
  for (const key of Object.keys(settings).sort()) {
    const value = settings[key];
    const group = key.split('.')[0] ?? key;
    const row: SettingRow = {
      key,
      value,
      redacted: value === SECRET_REDACTION_MARKER,
    };
    const list = groups.get(group) ?? [];
    list.push(row);
    groups.set(group, list);
  }
  return groups;
}

function renderValue(value: unknown): string {
  if (value === null) {
    return 'null';
  }
  if (typeof value === 'string') {
    return value;
  }
  return JSON.stringify(value);
}

export function ConfigSection() {
  const { t } = useTranslation();
  const [document, setDocument] = useState<ConfigExport | null>(null);
  const [loadError, setLoadError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [searchText, setSearchText] = useState('');

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
        // The browser reflects durable state; refresh after an apply.
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

  const groups = useMemo(() => {
    if (!document) {
      return new Map<string, SettingRow[]>();
    }
    const query = searchText.trim().toLowerCase();
    const filtered: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(document.settings)) {
      if (!query || key.toLowerCase().includes(query)) {
        filtered[key] = value;
      }
    }
    return groupSettings(filtered);
  }, [document, searchText]);

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
      <InlineNotification
        kind="info"
        lowContrast
        title={t('settings.readOnlyTitle')}
        subtitle={t('settings.readOnlyNote')}
        hideCloseButton
      />
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

      {document === null && !loadError ? (
        <Loading description={t('common.loading')} withOverlay={false} />
      ) : null}
      {document ? (
        <>
          <Search
            size="md"
            labelText={t('settings.searchLabel')}
            placeholder={t('settings.searchLabel')}
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
            onClear={() => setSearchText('')}
          />
          <p className="melodee-panel__subtitle">
            {t('settings.exportedAt', { time: formatDateTime(document.exportedAt) })}
          </p>
          {[...groups.entries()].map(([group, rows]) => (
            <section key={group} className="melodee-panel" aria-label={group}>
              <Heading className="melodee-panel__title">{group}</Heading>
              <TableContainer title={group}>
                <Table size="sm" aria-label={group}>
                  <TableHead>
                    <TableRow>
                      <TableHeader>{t('settings.column.key')}</TableHeader>
                      <TableHeader>{t('settings.column.value')}</TableHeader>
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {rows.map((row) => (
                      <TableRow key={row.key}>
                        <TableCell>
                          <code>{row.key}</code>
                        </TableCell>
                        <TableCell>
                          {row.redacted ? (
                            <StatusTag
                              kind="neutral"
                              label={t('settings.redacted')}
                              description={t('settings.redactedDescription')}
                            />
                          ) : (
                            <code>{renderValue(row.value)}</code>
                          )}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </TableContainer>
            </section>
          ))}
          {groups.size === 0 ? (
            <p className="melodee-panel__subtitle">{t('empty.noResults')}</p>
          ) : null}
        </>
      ) : null}

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
