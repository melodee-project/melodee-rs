import { Download, TrashCan, Renew } from '@carbon/icons-react';
import {
  ActionableNotification,
  Button,
  FileUploaderButton,
  Heading,
  InlineNotification,
  Loading,
  Modal,
  RadioButton,
  RadioButtonGroup,
  Tag,
} from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  deleteThemePack,
  importThemePack,
  listThemePacks,
  rescanThemePacks,
  themePackExportUrl,
} from '../../api/themes';
import type { ThemePackInfo } from '../../api/types';
import { toApiError } from '../catalog/catalog';
import { useThemeState } from '../../theme/ThemeProvider';
import { DEFAULT_THEME_PACK } from '../../theme/packs';

/**
 * Theme settings section (PRD-THEME-004, UI-SPEC §1). Lists the theme packs
 * served by the native endpoints and applies one as the theme axis —
 * independent of the system/light/dark appearance axis in the header menu.
 * Applying a pack loads its `--melodee-*` token values onto the token layer
 * (see src/theme/packs.ts for the mapping and its documented limitations).
 * The selection persists in localStorage with the appearance preference.
 *
 * Administrator actions (rescan / import / export / delete) consume the
 * native cookie-protected endpoints as documented; delete is offered only
 * for non-built-in packs, matching the server-side rule.
 */

type Notice = { kind: 'success' | 'error'; text: string };

export function ThemeSection() {
  const { t } = useTranslation();
  const { themePack, themePackError, setThemePack } = useThemeState();
  const [packs, setPacks] = useState<ThemePackInfo[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ThemePackInfo | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback((signal: { cancelled: boolean }) => {
    return listThemePacks()
      .then((result) => {
        if (!signal.cancelled) {
          setPacks(result);
          setLoadError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setLoadError(toApiError(cause).message);
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void load(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [load]);

  const onRescan = () => {
    setBusy(true);
    rescanThemePacks()
      .then((result) => {
        setPacks(result);
        setNotice({ kind: 'success', text: t('theme.rescanned') });
      })
      .catch((cause: unknown) => setNotice({ kind: 'error', text: toApiError(cause).message }))
      .finally(() => setBusy(false));
  };

  const onImportFile = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    // Reset the input so selecting the same file twice re-triggers.
    event.target.value = '';
    if (!file) {
      return;
    }
    setBusy(true);
    importThemePack(file)
      .then(() => {
        setNotice({ kind: 'success', text: t('theme.imported') });
        void load({ cancelled: false });
      })
      .catch((cause: unknown) => {
        setNotice({
          kind: 'error',
          text: `${t('theme.importFailed')} ${toApiError(cause).message}`,
        });
      })
      .finally(() => setBusy(false));
  };

  const onDelete = () => {
    if (!deleteTarget) {
      return;
    }
    setBusy(true);
    deleteThemePack(deleteTarget.id)
      .then(() => {
        if (themePack === deleteTarget.id) {
          setThemePack(DEFAULT_THEME_PACK);
        }
        setNotice({ kind: 'success', text: t('theme.deleted') });
        setDeleteTarget(null);
        void load({ cancelled: false });
      })
      .catch((cause: unknown) => {
        setNotice({
          kind: 'error',
          text: `${t('theme.deleteFailed')} ${toApiError(cause).message}`,
        });
        setDeleteTarget(null);
      })
      .finally(() => setBusy(false));
  };

  const packMeta = (pack: ThemePackInfo): string =>
    [
      pack.author ? t('theme.byAuthor', { author: pack.author }) : null,
      pack.version ? t('common.versionLabel', { version: pack.version }) : null,
    ]
      .filter((part): part is string => part !== null)
      .join(' · ');

  return (
    <section aria-labelledby="theme-section-title">
      <Heading id="theme-section-title" className="melodee-panel__title">
        {t('theme.sectionTitle')}
      </Heading>
      <p className="melodee-panel__subtitle">{t('theme.sectionDescription')}</p>

      {themePackError ? (
        <InlineNotification
          kind="warning"
          lowContrast
          title={t('theme.applyFailedTitle')}
          subtitle={t('theme.applyFailedDescription')}
          hideCloseButton
        />
      ) : null}
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      {loadError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('theme.loadErrorTitle')}
          subtitle={loadError}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => void load({ cancelled: false })}
        />
      ) : null}

      {packs === null && !loadError ? (
        <Loading description={t('common.loading')} withOverlay={false} />
      ) : null}

      {packs !== null ? (
        <RadioButtonGroup
          legendText={t('theme.listLabel')}
          name="theme-pack"
          valueSelected={themePack}
          onChange={(value) => setThemePack(String(value))}
        >
          <RadioButton
            id="theme-pack-default"
            value={DEFAULT_THEME_PACK}
            labelText={
              <>
                {t('theme.defaultPackName')} <Tag type="gray">{t('theme.builtInTag')}</Tag>
                <span className="melodee-theme-pack__meta">{t('theme.defaultPackDescription')}</span>
              </>
            }
          />
          {packs.map((pack) => (
            <RadioButton
              key={pack.id}
              id={`theme-pack-${pack.id}`}
              value={pack.id}
              labelText={
                <>
                  {pack.name} {pack.isBuiltIn ? <Tag type="gray">{t('theme.builtInTag')}</Tag> : null}
                  {pack.hasWarnings ? <Tag type="red">{t('theme.warningsTag')}</Tag> : null}
                  <span className="melodee-theme-pack__meta">
                    {packMeta(pack)}
                    {pack.description ? ` — ${pack.description}` : ''}
                  </span>
                </>
              }
            />
          ))}
        </RadioButtonGroup>
      ) : null}

      <div className="melodee-panel__footer" style={{ marginBlock: '1rem' }}>
        <Button kind="secondary" size="sm" renderIcon={Renew} disabled={busy} onClick={onRescan}>
          {t('theme.rescan')}
        </Button>
        <FileUploaderButton
          labelText={t('theme.import')}
          accept={['.zip', 'application/zip']}
          buttonKind="secondary"
          size="sm"
          disabled={busy}
          onChange={onImportFile}
        />
      </div>

      {packs !== null && packs.some((pack) => !pack.isBuiltIn) ? (
        <ul className="melodee-theme-pack__actions-list">
          {packs
            .filter((pack) => !pack.isBuiltIn)
            .map((pack) => (
              <li key={pack.id} className="melodee-theme-pack__actions-row">
                <span>{pack.name}</span>
                <span>
                  <Button
                    kind="ghost"
                    size="sm"
                    renderIcon={Download}
                    as="a"
                    href={themePackExportUrl(pack.id)}
                  >
                    {t('theme.export')}
                  </Button>
                  <Button
                    kind="danger--ghost"
                    size="sm"
                    renderIcon={TrashCan}
                    disabled={busy}
                    onClick={() => setDeleteTarget(pack)}
                  >
                    {t('theme.delete')}
                  </Button>
                </span>
              </li>
            ))}
        </ul>
      ) : null}

      <Modal
        open={deleteTarget !== null}
        danger
        modalHeading={t('theme.deleteConfirmTitle')}
        primaryButtonText={t('theme.deleteConfirmAction')}
        secondaryButtonText={t('actions.cancel')}
        onRequestClose={() => setDeleteTarget(null)}
        onRequestSubmit={onDelete}
      >
        <p>{t('theme.deleteConfirmBody', { name: deleteTarget?.name ?? '' })}</p>
      </Modal>
    </section>
  );
}
