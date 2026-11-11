import {
  Button,
  Dropdown,
  InlineNotification,
  ListItem,
  Modal,
  TextInput,
  UnorderedList,
} from '@carbon/react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  applyReleaseMetadataTransform,
  listReleaseMetadataAudits,
  previewReleaseMetadataTransform,
  revertReleaseMetadataTransform,
} from '../../api/admin';
import { Surface } from '../../components/Surface';
import { ApiError } from '../../api/client';
import type {
  MetadataTransformAudit,
  MetadataTransformPreview,
} from '../../api/types';

const OPERATIONS = [
  'remove_featuring_artists',
  'replace_song_titles',
  'replace_song_artists',
  'set_artist_on_all_songs',
  'set_year',
  'renumber_songs',
  'remove_all_song_artists',
] as const;

function apiMessage(error: unknown): string {
  return error instanceof ApiError ? error.message : String(error);
}

export function MetadataTransformPanel({
  releaseId,
  refresh,
}: {
  releaseId: string;
  refresh: () => void;
}) {
  const { t } = useTranslation();
  const [operation, setOperation] = useState<string>(OPERATIONS[0]);
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [artist, setArtist] = useState('');
  const [year, setYear] = useState('');
  const [preview, setPreview] = useState<MetadataTransformPreview | null>(null);
  const [audits, setAudits] = useState<MetadataTransformAudit[]>([]);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ kind: 'error' | 'success'; text: string } | null>(null);
  const [confirmApply, setConfirmApply] = useState(false);
  const [revertTarget, setRevertTarget] = useState<MetadataTransformAudit | null>(null);

  const params = useMemo(() => {
    if (operation === 'set_year') return { year };
    if (operation === 'set_artist_on_all_songs') return { artist };
    if (operation === 'replace_song_titles' || operation === 'replace_song_artists') {
      return { from, to };
    }
    return {};
  }, [artist, from, operation, to, year]);

  const loadAudits = () => {
    listReleaseMetadataAudits(releaseId).then(setAudits).catch((error: unknown) => {
      setNotice({ kind: 'error', text: apiMessage(error) });
    });
  };

  useEffect(loadAudits, [releaseId]);

  const previewTransform = async () => {
    setBusy(true);
    setNotice(null);
    try {
      setPreview(await previewReleaseMetadataTransform(releaseId, operation, params));
    } catch (error) {
      setNotice({ kind: 'error', text: apiMessage(error) });
    } finally {
      setBusy(false);
    }
  };

  const applyTransform = async () => {
    if (!preview) return;
    setBusy(true);
    setNotice(null);
    try {
      await applyReleaseMetadataTransform(releaseId, operation, preview.previewHash, params);
      setPreview(null);
      setNotice({ kind: 'success', text: t('catalog.transform.applied') });
      loadAudits();
      refresh();
    } catch (error) {
      setNotice({ kind: 'error', text: apiMessage(error) });
    } finally {
      setBusy(false);
      setConfirmApply(false);
    }
  };

  const revertTransform = async () => {
    if (!revertTarget) return;
    setBusy(true);
    setNotice(null);
    try {
      await revertReleaseMetadataTransform(releaseId, revertTarget.id);
      setNotice({ kind: 'success', text: t('catalog.transform.reverted') });
      loadAudits();
      refresh();
    } catch (error) {
      setNotice({ kind: 'error', text: apiMessage(error) });
    } finally {
      setBusy(false);
      setRevertTarget(null);
    }
  };

  return (
    <Surface
      title={t('catalog.transform.title')}
      subtitle={t('catalog.transform.description')}
      headingLevel={3}
      className="melodee-transform-panel"
    >
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.kind === 'error' ? t('common.error') : t('common.success')}
          subtitle={notice.text}
          onClose={() => setNotice(null)}
        />
      ) : null}
      <Dropdown
        id="metadata-transform-operation"
        titleText={t('catalog.transform.operation')}
        label={t('catalog.transform.chooseOperation')}
        items={[...OPERATIONS]}
        selectedItem={operation}
        onChange={({ selectedItem }) => {
          if (selectedItem) setOperation(selectedItem);
          setPreview(null);
        }}
        itemToString={(item) => (item ? t(`catalog.transform.operation.${item}`) : '')}
      />
      {operation === 'replace_song_titles' || operation === 'replace_song_artists' ? (
        <div style={{ display: 'flex', gap: '1rem', marginBlock: '1rem' }}>
          <TextInput id="transform-from" labelText={t('catalog.transform.from')} value={from} onChange={(event) => setFrom(event.target.value)} />
          <TextInput id="transform-to" labelText={t('catalog.transform.to')} value={to} onChange={(event) => setTo(event.target.value)} />
        </div>
      ) : null}
      {operation === 'set_artist_on_all_songs' ? (
        <TextInput id="transform-artist" labelText={t('catalog.transform.artist')} value={artist} onChange={(event) => setArtist(event.target.value)} />
      ) : null}
      {operation === 'set_year' ? (
        <TextInput id="transform-year" labelText={t('catalog.transform.year')} value={year} onChange={(event) => setYear(event.target.value)} />
      ) : null}
      <div style={{ display: 'flex', gap: '0.75rem', marginBlock: '1rem' }}>
        <Button kind="secondary" onClick={previewTransform} disabled={busy}>
          {t('catalog.transform.preview')}
        </Button>
        <Button kind="primary" onClick={() => setConfirmApply(true)} disabled={busy || preview === null || preview.affectedFiles === 0}>
          {t('catalog.transform.apply')}
        </Button>
      </div>
      {preview ? (
        <div aria-live="polite">
          <p>{t('catalog.transform.previewSummary', { count: preview.affectedFiles })}</p>
          {preview.deltas.length > 0 ? (
            <UnorderedList>
              {preview.deltas.slice(0, 25).map((delta) => (
                <ListItem key={`${delta.file}-${delta.field}`}>
                  {delta.file}: {delta.field} — {delta.old ?? t('common.none')} → {delta.new ?? t('common.none')}
                </ListItem>
              ))}
            </UnorderedList>
          ) : <p>{t('catalog.transform.noChanges')}</p>}
        </div>
      ) : null}
      <h4>{t('catalog.transform.history')}</h4>
      {audits.length === 0 ? <p>{t('catalog.transform.noHistory')}</p> : (
        <UnorderedList>
          {audits.map((audit) => (
            <ListItem key={audit.id}>
              <span>{t(`catalog.transform.operation.${audit.operationName}`, { defaultValue: audit.operationName })}</span> — {audit.status} — {audit.affectedFiles} {t('catalog.transform.files')}
              {audit.status === 'success' ? (
                <Button kind="ghost" size="sm" onClick={() => setRevertTarget(audit)} disabled={busy}>
                  {t('catalog.transform.revert')}
                </Button>
              ) : null}
            </ListItem>
          ))}
        </UnorderedList>
      )}
      <Modal
        open={confirmApply}
        modalHeading={t('catalog.transform.confirmTitle')}
        primaryButtonText={t('catalog.transform.apply')}
        secondaryButtonText={t('actions.cancel')}
        onRequestClose={() => setConfirmApply(false)}
        onRequestSubmit={applyTransform}
      >
        <p>{t('catalog.transform.confirmBody', { count: preview?.affectedFiles ?? 0 })}</p>
      </Modal>
      <Modal
        open={revertTarget !== null}
        modalHeading={t('catalog.transform.revertTitle')}
        primaryButtonText={t('catalog.transform.revert')}
        secondaryButtonText={t('actions.cancel')}
        onRequestClose={() => setRevertTarget(null)}
        onRequestSubmit={revertTransform}
      >
        <p>{t('catalog.transform.revertBody')}</p>
      </Modal>
    </Surface>
  );
}
