import { Add } from '@carbon/icons-react';
import {
  ActionableNotification,
  Button,
  Heading,
  InlineNotification,
  Loading,
  Modal,
  NumberInput,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
  TextArea,
  TextInput,
} from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  deleteScript,
  disableScript,
  enableScript,
  listScriptRuns,
  listScripts,
  testRunScript,
  upsertScript,
} from '../../api/admin';
import { ApiError } from '../../api/client';
import type {
  AdminScript,
  ScriptListResponse,
  ScriptRun,
  ScriptTestRunResult,
} from '../../api/types';
import { EmptyState } from '../../components/EmptyState';
import { StatusTag } from '../../components/StatusTag';
import { formatDateTime, formatNumber, formatRelativeTime } from '../../i18n/format';

/**
 * Scripts (WP-19, PRD-SCRIPT-001..005, ADR-0135): registered scripts by
 * event with enable/disable (explicit routes — circuit-breaker-disabled
 * scripts cannot be re-enabled by an upsert), source editing via a plain
 * textarea (no code editor is claimed), test-run against an operator ctx
 * mock with the real sandbox evaluation result, and the execution audit.
 * The wired-event registry renders so operators can tell live hooks from
 * inert registrations; the effective on-deny action is shown verbatim.
 */

interface Notice {
  kind: 'success' | 'error' | 'info' | 'warning';
  text: string;
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

function outcomeKind(outcome: string): 'success' | 'error' | 'warning' | 'neutral' {
  const normalized = outcome.toLowerCase();
  if (normalized === 'allow' || normalized === 'ok' || normalized === 'success') {
    return 'success';
  }
  if (normalized === 'error' || normalized === 'failed') {
    return 'error';
  }
  if (normalized === 'deny' || normalized === 'skip') {
    return 'warning';
  }
  return 'neutral';
}

export function ScriptsSection() {
  const { t } = useTranslation();
  const [listing, setListing] = useState<ScriptListResponse | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [runs, setRuns] = useState<ScriptRun[] | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const [registerOpen, setRegisterOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<AdminScript | null>(null);
  const [testTarget, setTestTarget] = useState<AdminScript | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<AdminScript | null>(null);

  const fetchScripts = useCallback((signal: { cancelled: boolean }) => {
    return listScripts()
      .then((result) => {
        if (!signal.cancelled) {
          setListing(result);
          setListError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setListError(toApiError(cause));
        }
      });
  }, []);

  const fetchRuns = useCallback((signal: { cancelled: boolean }) => {
    return listScriptRuns({ limit: 100 })
      .then((result) => {
        if (!signal.cancelled) {
          setRuns(result);
        }
      })
      .catch(() => {
        // The run audit is supplementary; a failure leaves the empty state.
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchScripts(signal);
    void fetchRuns(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchScripts, fetchRuns]);

  const mutationError = (cause: unknown) =>
    setNotice({
      kind: 'error',
      text: t('settings.scripts.error', { message: toApiError(cause).message }),
    });

  const toggleEnabled = (script: AdminScript) => {
    setBusyId(script.id);
    const request = script.enabled ? disableScript(script.id) : enableScript(script.id);
    request
      .then(() => {
        setNotice({
          kind: 'success',
          text: t(script.enabled ? 'settings.scripts.disableSuccess' : 'settings.scripts.enableSuccess'),
        });
        void fetchScripts({ cancelled: false });
      })
      .catch(mutationError)
      .finally(() => setBusyId(null));
  };

  return (
    <div>
      <InlineNotification
        kind="info"
        lowContrast
        title={t('settings.scripts.wiredTitle')}
        subtitle={
          listing && listing.wiredEvents.length > 0
            ? t('settings.scripts.wiredNote', { events: listing.wiredEvents.join(', ') })
            : t('settings.scripts.wiredNone')
        }
        hideCloseButton
      />
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      {listError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('settings.scripts.loadErrorTitle')}
          subtitle={listError.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => void fetchScripts({ cancelled: false })}
        />
      ) : null}
      {listing === null && !listError ? (
        <Loading description={t('common.loading')} withOverlay={false} />
      ) : null}
      {listing !== null ? (
        <TableContainer title={t('settings.scripts.tableCaption')}>
          <Table aria-label={t('settings.scripts.tableCaption')}>
            <TableHead>
              <TableRow>
                <TableHeader>{t('settings.scripts.column.event')}</TableHeader>
                <TableHeader>{t('common.name')}</TableHeader>
                <TableHeader>{t('settings.scripts.column.hook')}</TableHeader>
                <TableHeader>{t('common.status')}</TableHeader>
                <TableHeader>{t('settings.scripts.column.version')}</TableHeader>
                <TableHeader>{t('common.updated')}</TableHeader>
                <TableHeader>{t('users.column.actions')}</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {listing.scripts.map((script) => (
                <TableRow key={script.id}>
                  <TableCell>
                    <code>{script.event}</code>
                  </TableCell>
                  <TableCell>{script.name}</TableCell>
                  <TableCell>
                    {script.wired ? (
                      <StatusTag kind="info" label={t('settings.scripts.hookWired')} />
                    ) : (
                      <StatusTag
                        kind="neutral"
                        label={t('settings.scripts.hookInert')}
                        description={t('settings.scripts.hookInertDescription')}
                      />
                    )}
                  </TableCell>
                  <TableCell>
                    {script.enabled ? (
                      <StatusTag kind="success" label={t('status.enabled')} />
                    ) : (
                      <StatusTag kind="neutral" label={t('status.disabled')} />
                    )}
                  </TableCell>
                  <TableCell>{formatNumber(script.version)}</TableCell>
                  <TableCell>
                    <span title={formatDateTime(script.updatedAt)}>
                      {formatRelativeTime(script.updatedAt)}
                    </span>
                  </TableCell>
                  <TableCell>
                    <Button kind="ghost" size="sm" onClick={() => setEditTarget(script)}>
                      {t('actions.edit')}
                    </Button>
                    <Button
                      kind="ghost"
                      size="sm"
                      disabled={busyId === script.id}
                      onClick={() => toggleEnabled(script)}
                    >
                      {script.enabled ? t('actions.disable') : t('actions.enable')}
                    </Button>
                    <Button kind="ghost" size="sm" onClick={() => setTestTarget(script)}>
                      {t('settings.scripts.testRun')}
                    </Button>
                    <Button
                      kind="danger--ghost"
                      size="sm"
                      onClick={() => setDeleteTarget(script)}
                    >
                      {t('actions.delete')}
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {listing.scripts.length === 0 ? (
            <EmptyState
              compact
              title={t('settings.scripts.emptyTitle')}
              description={t('settings.scripts.emptyDescription')}
            />
          ) : null}
        </TableContainer>
      ) : null}
      <div style={{ marginBlockStart: '1rem' }}>
        <Button kind="primary" size="sm" renderIcon={Add} onClick={() => setRegisterOpen(true)}>
          {t('settings.scripts.register')}
        </Button>
      </div>

      <section className="melodee-panel" aria-labelledby="script-runs-title">
        <Heading id="script-runs-title" className="melodee-panel__title">
          {t('settings.scripts.runsTitle')}
        </Heading>
        {runs === null || runs.length === 0 ? (
          <EmptyState compact title={t('settings.scripts.runsEmpty')} />
        ) : (
          <TableContainer title={t('settings.scripts.runsCaption')}>
            <Table size="sm" aria-label={t('settings.scripts.runsCaption')}>
              <TableHead>
                <TableRow>
                  <TableHeader>{t('settings.scripts.column.event')}</TableHeader>
                  <TableHeader>{t('settings.scripts.column.outcome')}</TableHeader>
                  <TableHeader>{t('settings.scripts.column.duration')}</TableHeader>
                  <TableHeader>{t('settings.scripts.column.error')}</TableHeader>
                  <TableHeader>{t('common.created')}</TableHeader>
                </TableRow>
              </TableHead>
              <TableBody>
                {runs.map((run) => (
                  <TableRow key={run.id}>
                    <TableCell>
                      <code>{run.event}</code>
                    </TableCell>
                    <TableCell>
                      <StatusTag kind={outcomeKind(run.outcome)} label={run.outcome} />
                    </TableCell>
                    <TableCell>{t('settings.scripts.durationMs', { ms: formatNumber(run.durationMs) })}</TableCell>
                    <TableCell>{run.error ?? t('common.none')}</TableCell>
                    <TableCell>
                      <span title={formatDateTime(run.createdAt)}>
                        {formatRelativeTime(run.createdAt)}
                      </span>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        )}
      </section>

      <ScriptEditorModal
        open={registerOpen}
        script={null}
        onClose={() => setRegisterOpen(false)}
        onSaved={() => {
          setRegisterOpen(false);
          setNotice({ kind: 'success', text: t('settings.scripts.saveSuccess') });
          void fetchScripts({ cancelled: false });
        }}
        onError={mutationError}
      />
      <ScriptEditorModal
        key={`edit-${editTarget?.id ?? 'closed'}`}
        open={editTarget !== null}
        script={editTarget}
        onClose={() => setEditTarget(null)}
        onSaved={() => {
          setEditTarget(null);
          setNotice({ kind: 'success', text: t('settings.scripts.saveSuccess') });
          void fetchScripts({ cancelled: false });
        }}
        onError={mutationError}
      />
      <TestRunModal
        key={`test-${testTarget?.id ?? 'closed'}`}
        script={testTarget}
        onClose={() => setTestTarget(null)}
        onRan={() => void fetchRuns({ cancelled: false })}
      />
      {deleteTarget !== null ? (
        <Modal
          danger
          open
          modalHeading={t('settings.scripts.deleteTitle', { name: deleteTarget.name })}
          primaryButtonText={t('actions.delete')}
          secondaryButtonText={t('actions.cancel')}
          onRequestClose={() => setDeleteTarget(null)}
          onRequestSubmit={() => {
            const target = deleteTarget;
            setDeleteTarget(null);
            deleteScript(target.id)
              .then(() => {
                setNotice({ kind: 'success', text: t('settings.scripts.deleteSuccess') });
                void fetchScripts({ cancelled: false });
              })
              .catch(mutationError);
          }}
        >
          <p>{t('settings.scripts.deleteBody', { event: deleteTarget.event })}</p>
        </Modal>
      ) : null}
    </div>
  );
}

/**
 * Register/edit modal: upsert keyed by (event, name); event and name are
 * the identity, so they are editable only when registering. Source editing
 * is a plain textarea — no code editor is claimed.
 */
function ScriptEditorModal({
  open,
  script,
  onClose,
  onSaved,
  onError,
}: {
  open: boolean;
  script: AdminScript | null;
  onClose: () => void;
  onSaved: () => void;
  onError: (cause: unknown) => void;
}) {
  const { t } = useTranslation();
  const [event, setEvent] = useState(script?.event ?? '');
  const [name, setName] = useState(script?.name ?? '');
  const [source, setSource] = useState(script?.source ?? '');
  const [timeoutMs, setTimeoutMs] = useState(script?.timeoutMs ?? 100);
  const [memoryBytes, setMemoryBytes] = useState(script?.memoryBytes ?? 16 * 1024 * 1024);
  const [busy, setBusy] = useState(false);

  const valid =
    event.trim().length > 0 && name.trim().length > 0 && source.trim().length > 0 && !busy;

  // Closed modals are not rendered (Carbon keeps them mounted otherwise).
  if (!open) {
    return null;
  }

  const submit = () => {
    if (!valid) {
      return;
    }
    setBusy(true);
    upsertScript(event.trim(), name.trim(), {
      source,
      timeoutMs,
      memoryBytes,
    })
      .then(onSaved)
      .catch(onError)
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={open}
      size="lg"
      modalHeading={
        script
          ? t('settings.scripts.editTitle', { name: script.name })
          : t('settings.scripts.registerTitle')
      }
      primaryButtonText={t('actions.save')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={!valid}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      {!script ? (
        <>
          <TextInput
            id="script-event"
            labelText={t('settings.scripts.column.event')}
            helperText={t('settings.scripts.eventHelp')}
            value={event}
            onChange={(change) => setEvent(change.target.value)}
          />
          <TextInput
            id="script-name"
            labelText={t('common.name')}
            value={name}
            onChange={(change) => setName(change.target.value)}
          />
        </>
      ) : null}
      <TextArea
        id="script-source"
        labelText={t('settings.scripts.sourceLabel')}
        helperText={t('settings.scripts.sourceHelp')}
        rows={10}
        value={source}
        onChange={(change) => setSource(change.target.value)}
      />
      <NumberInput
        id="script-timeout"
        label={t('settings.scripts.timeoutLabel')}
        min={1}
        value={timeoutMs}
        onChange={(_change, { value }) => {
          if (typeof value === 'number' && Number.isFinite(value)) {
            setTimeoutMs(value);
          }
        }}
      />
      <NumberInput
        id="script-memory"
        label={t('settings.scripts.memoryLabel')}
        min={1}
        value={memoryBytes}
        onChange={(_change, { value }) => {
          if (typeof value === 'number' && Number.isFinite(value)) {
            setMemoryBytes(value);
          }
        }}
      />
    </Modal>
  );
}

/** Test-run: stored script body against an operator ctx JSON mock. */
function TestRunModal({
  script,
  onClose,
  onRan,
}: {
  script: AdminScript | null;
  onClose: () => void;
  onRan: () => void;
}) {
  const { t } = useTranslation();
  const [ctxText, setCtxText] = useState('{}');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<ScriptTestRunResult | null>(null);

  if (!script) {
    return null;
  }

  const submit = () => {
    if (busy) {
      return;
    }
    let ctx: unknown;
    try {
      ctx = JSON.parse(ctxText);
    } catch {
      setError(t('settings.scripts.ctxInvalid'));
      return;
    }
    setBusy(true);
    setError(null);
    setResult(null);
    testRunScript({ event: script.event, scriptId: script.id, ctx })
      .then((testResult) => {
        setResult(testResult);
        onRan();
      })
      .catch((cause: unknown) =>
        setError(t('settings.scripts.testError', { message: toApiError(cause).message })),
      )
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={script !== null}
      modalHeading={t('settings.scripts.testTitle', { name: script?.name ?? '' })}
      primaryButtonText={t('settings.scripts.testRun')}
      secondaryButtonText={t('actions.close')}
      primaryButtonDisabled={busy}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      {error ? (
        <InlineNotification kind="error" lowContrast title={error} hideCloseButton />
      ) : null}
      {result ? (
        <div>
          <p>
            <StatusTag
              kind={outcomeKind(result.evaluation.outcome)}
              label={result.evaluation.outcome}
            />{' '}
            {t('settings.scripts.durationMs', { ms: formatNumber(result.evaluation.durationMs) })}
          </p>
          {result.evaluation.message ? <p>{result.evaluation.message}</p> : null}
          {result.evaluation.error ? (
            <InlineNotification
              kind="error"
              lowContrast
              title={result.evaluation.error}
              hideCloseButton
            />
          ) : null}
          <p className="melodee-panel__subtitle">
            {t('settings.scripts.effectiveOnDeny', { action: result.effectiveOnDeny })}
          </p>
        </div>
      ) : null}
      <TextArea
        id="script-test-ctx"
        labelText={t('settings.scripts.ctxLabel')}
        helperText={t('settings.scripts.ctxHelp')}
        value={ctxText}
        onChange={(change) => setCtxText(change.target.value)}
      />
    </Modal>
  );
}
