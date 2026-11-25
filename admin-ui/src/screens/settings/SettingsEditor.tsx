import {
  Button,
  InlineNotification,
  Loading,
  Modal,
  Search,
  Select,
  SelectItem,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  TextInput,
} from '@carbon/react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listConfigEntries, updateConfigEntry } from '../../api/admin';
import type { ConfigEntry, ConfigChange } from '../../api/types';

const pageSize = 25;
const throughputLabels: Record<string, string> = {
  MELODEE__INSPECTION__CONCURRENCY: 'settings.processing.inspection',
  MELODEE__NORMALIZATION__CONCURRENCY: 'settings.processing.normalization',
  MELODEE__VALIDATION__CONCURRENCY: 'settings.processing.validation',
  MELODEE__PUBLICATION__CONCURRENCY: 'settings.processing.publication',
  MELODEE__PUBLICATION__NFS_MIB_PER_SECOND: 'settings.processing.nfs',
  MELODEE__CATALOG__CONCURRENCY: 'settings.processing.catalog',
};
const throughput = Object.keys(throughputLabels);
const sources = {
  database: 'settings.source.database',
  environment: 'settings.source.environment',
  'config-file': 'settings.source.file',
  default: 'settings.source.default',
  unset: 'settings.source.unset',
} as const;

function format(value: unknown): string {
  if (value == null) return '';
  return typeof value === 'string' ? value : JSON.stringify(value);
}

export function SettingsEditor({ revision, onSaved }: { revision: number; onSaved: () => void }) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<ConfigEntry[]>();
  const [reload, setReload] = useState(0);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [query, setQuery] = useState('');
  const [page, setPage] = useState(0);
  const [editing, setEditing] = useState<ConfigEntry | null>(null);
  const [input, setInput] = useState('');
  const [confirmReset, setConfirmReset] = useState(false);
  const [saveError, setSaveError] = useState('');
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let current = true;
    listConfigEntries()
      .then((rows) => {
        if (current) {
          // Keep the six processing controls together at the start of the list.
          const rank = (key: string) => {
            const i = throughput.indexOf(key);
            return i < 0 ? throughput.length : i;
          };
          setEntries(
            [...rows].sort((a, b) => rank(a.key) - rank(b.key) || a.key.localeCompare(b.key)),
          );
          setError('');
        }
      })
      .catch((e: unknown) => {
        if (current) setError(String(e));
      });
    return () => {
      current = false;
    };
  }, [revision, reload]);
  const search = query.trim().toLowerCase();
  const rows = (entries ?? []).filter((item) =>
    [
      item.key,
      item.envName,
      throughputLabels[item.key] ? t(throughputLabels[item.key]!) : '',
      item.envName.replaceAll('__', ' ').replaceAll('_', ' '),
    ].some((name) => name.toLowerCase().includes(search)),
  );
  const lastPage = Math.max(0, Math.ceil(rows.length / pageSize) - 1);
  const currentPage = Math.min(page, lastPage);
  function open(row: ConfigEntry) {
    setEditing(row);
    setConfirmReset(false);
    const value = row.savedValue;
    setInput(
      row.secret
        ? ''
        : row.kind === 'boolean'
          ? String(['true', '1', 'yes'].includes(format(value).toLowerCase()))
          : row.kind === 'json'
            ? JSON.stringify(value)
            : format(value),
    );
    setSaveError('');
  }
  async function submit(reset: boolean) {
    if (!editing || busy) return;
    setBusy(true);
    setSaveError('');
    try {
      let change: ConfigChange = { action: 'reset' };
      if (!reset) {
        let value: unknown = input;
        if (editing.kind === 'boolean') value = input === 'true';
        if (editing.kind === 'integer' || editing.kind === 'number') {
          value = Number(input);
          if (
            !input.trim() ||
            !Number.isFinite(value) ||
            (editing.kind === 'integer' && !Number.isSafeInteger(value))
          ) {
            throw new Error(t('settings.editor.invalidNumber'));
          }
        }
        if (editing.kind === 'json') value = JSON.parse(input);
        change = { action: 'set', value };
      }
      await updateConfigEntry(editing.key, change);
      setEditing(null);
      setMessage(
        t(editing.usage === 'startup' ? 'settings.editor.savedStartup' : 'settings.editor.saved', {
          key: editing.key,
        }),
      );
      setReload((n) => n + 1);
      onSaved();
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      {message && (
        <InlineNotification
          kind="success"
          lowContrast
          title={message}
          onCloseButtonClick={() => setMessage('')}
        />
      )}
      {error && (
        <InlineNotification kind="error" title={error} hideCloseButton>
          <Button size="sm" onClick={() => setReload((n) => n + 1)}>
            {t('actions.retry')}
          </Button>
        </InlineNotification>
      )}
      {!entries && !error && <Loading withOverlay={false} description={t('common.loading')} />}
      {entries && (
        <>
          <Search
            labelText={t('settings.searchLabel')}
            placeholder={t('settings.searchLabel')}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setPage(0);
            }}
            onClear={() => {
              setQuery('');
              setPage(0);
            }}
          />
          <Table size="md" aria-label={t('settings.editor.title')}>
            <TableHead>
              <TableRow>
                <TableHeader>{t('settings.column.key')}</TableHeader>
                <TableHeader>{t('settings.column.value')}</TableHeader>
                <TableHeader>{t('settings.editor.source')}</TableHeader>
                <TableHeader>{t('settings.editor.manage')}</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {rows.slice(currentPage * pageSize, (currentPage + 1) * pageSize).map((row) => (
                <TableRow key={row.key}>
                  <TableCell style={{ overflowWrap: 'anywhere' }}>
                    {throughputLabels[row.key] && (
                      <strong style={{ display: 'block' }}>{t(throughputLabels[row.key]!)}</strong>
                    )}
                    <code style={{ overflowWrap: 'anywhere' }}>{row.key}</code>
                    {row.envName !== row.key && (
                      <div className="melodee-panel__subtitle">{row.envName}</div>
                    )}
                    {row.usage === 'legacy' && <div>{t('settings.editor.legacy')}</div>}
                  </TableCell>
                  <TableCell style={{ overflowWrap: 'anywhere' }}>
                    {row.value == null
                      ? t('settings.source.unset')
                      : row.secret
                        ? t('settings.redacted')
                        : format(row.value)}
                    {row.pendingRestart && (
                      <div>
                        <strong>
                          {t('settings.editor.nextStart', {
                            value:
                              row.savedValue == null
                                ? t('settings.source.unset')
                                : format(row.savedValue),
                          })}
                        </strong>
                      </div>
                    )}
                  </TableCell>
                  <TableCell>
                    {t(sources[row.source])}
                    {row.pendingRestart && row.savedSource !== row.source && (
                      <div>{t('settings.editor.nextSource', { source: t(sources[row.savedSource]) })}</div>
                    )}
                  </TableCell>
                  <TableCell>
                    {row.editable ? (
                      <Button
                        kind="ghost"
                        size="sm"
                        aria-label={t('settings.editor.edit', { key: row.key })}
                        onClick={() => open(row)}
                      >
                        {t('actions.edit')}
                      </Button>
                    ) : (
                      t('settings.editor.host')
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {rows.length === 0 && <p>{t('empty.noResults')}</p>}
          {lastPage > 0 && (
            <div className="melodee-panel__footer">
              <Button
                kind="ghost"
                size="sm"
                disabled={currentPage === 0}
                onClick={() => setPage(currentPage - 1)}
              >
                {t('settings.editor.previous')}
              </Button>
              <span>
                {t('settings.editor.page', { page: currentPage + 1, pages: lastPage + 1 })}
              </span>
              <Button
                kind="ghost"
                size="sm"
                disabled={currentPage === lastPage}
                onClick={() => setPage(currentPage + 1)}
              >
                {t('settings.editor.next')}
              </Button>
            </div>
          )}
        </>
      )}
      {editing && (
        <Modal
          open
          modalHeading={t('settings.editor.edit', { key: editing.key })}
          primaryButtonText={t(confirmReset ? 'settings.editor.removeOverride' : 'actions.save')}
          secondaryButtonText={t('actions.cancel')}
          primaryButtonDisabled={busy || (!confirmReset && editing.secret && !input)}
          onRequestClose={() => {
            if (!busy) setEditing(null);
          }}
          onRequestSubmit={() => void submit(confirmReset)}
        >
          <p>
            {t(
              editing.usage === 'startup'
                ? 'settings.editor.restart'
                : editing.usage === 'legacy'
                  ? 'settings.editor.legacy'
                  : 'settings.editor.registry',
            )}
          </p>
          {confirmReset ? (
            <p>{t('settings.editor.confirmReset')}</p>
          ) : editing.kind === 'boolean' || editing.kind === 'choice' ? (
            <Select
              id="setting-edit-value"
              labelText={t('settings.column.value')}
              value={input}
              onChange={(e) => setInput(e.target.value)}
            >
              {(editing.kind === 'boolean' ? ['true', 'false'] : editing.options).map((v) => (
                <SelectItem key={v} value={v} text={v} />
              ))}
            </Select>
          ) : (
            <TextInput
              id="setting-edit-value"
              labelText={t('settings.column.value')}
              type={editing.secret ? 'password' : 'text'}
              autoComplete="off"
              value={input}
              onChange={(e) => setInput(e.target.value)}
            />
          )}
          {editing.canReset && !confirmReset && (
            <Button kind="ghost" size="sm" disabled={busy} onClick={() => setConfirmReset(true)}>
              {t('settings.editor.removeOverride')}
            </Button>
          )}
          {saveError && <InlineNotification kind="error" title={saveError} hideCloseButton />}
        </Modal>
      )}
    </>
  );
}
