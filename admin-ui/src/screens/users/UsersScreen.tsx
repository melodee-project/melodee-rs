import { Add, Renew } from '@carbon/icons-react';
import {
  ActionableNotification,
  Button,
  Checkbox,
  InlineNotification,
  Loading,
  Modal,
  Search,
  Tab,
  TabList,
  TabPanel,
  TabPanels,
  Tabs,
  TextInput,
} from '@carbon/react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useSearchParams } from 'react-router-dom';

import { createUser, listUsers, setUserPassword, updateUser } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { Page, SortParam, UserCreateRequest, UserSummary } from '../../api/types';
import { CellInteractive } from '../../components/CellInteractive';
import { EntityTable, type EntityColumn, type EntityRow } from '../../components/EntityTable';
import { PageHeader } from '../../components/PageHeader';
import { StatusTag } from '../../components/StatusTag';
import { formatDateTime, formatRelativeTime } from '../../i18n/format';
import { GroupsPanel } from './GroupsPanel';

/**
 * Users & Access (WP-18, PRD-API-ADMIN-002, PRD-WEB-002): server-paged,
 * server-sorted, server-filtered user table with create/edit/set-password
 * flows, plus the library access groups tab. Every mutation surfaces the
 * stable error envelope (409 user_conflict / group_not_empty, 400
 * invalid_password / invalid_request) localized; secrets never render.
 */

/** Sort whitelist shared with the backend (UserSort::parse). */
const SORT_KEYS = new Set(['username', 'createdAt']);
const MIN_PASSWORD_CHARS = 8;
const SEARCH_DEBOUNCE_MS = 300;

interface Notice {
  kind: 'success' | 'error' | 'info' | 'warning';
  text: string;
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

function parseSort(raw: string | null): SortParam | undefined {
  if (!raw) {
    return undefined;
  }
  const [key, direction] = raw.split(':');
  if (!key || (direction !== 'asc' && direction !== 'desc') || !SORT_KEYS.has(key)) {
    return undefined;
  }
  return `${key}:${direction}` as SortParam;
}

function parsePage(raw: string | null): number {
  return Math.max(1, Number.parseInt(raw ?? '1', 10) || 1);
}

/** Parses the comma-separated role editor into a trimmed, deduped list. */
function parseRoles(raw: string): string[] {
  return [...new Set(raw.split(',').map((role) => role.trim()).filter((role) => role.length > 0))];
}

/** Localized text for the stable user-mutation error codes. */
function mutationErrorText(
  error: ApiError,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  if (error.status === 409 && error.code === 'user_conflict') {
    return t('users.error.userConflict');
  }
  if (error.status === 400 && error.code === 'invalid_password') {
    return t('users.error.invalidPassword', { min: MIN_PASSWORD_CHARS });
  }
  return t('users.error.mutation', { message: error.message });
}

export function UsersScreen() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();

  const tab = searchParams.get('tab') === 'groups' ? 'groups' : 'users';
  const queryParam = searchParams.get('query') ?? '';
  const sort = parseSort(searchParams.get('sort'));
  const page = parsePage(searchParams.get('page'));
  const pageSize = Math.max(1, Number.parseInt(searchParams.get('pageSize') ?? '50', 10) || 50);

  const [pageData, setPageData] = useState<Page<UserSummary> | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [notice, setNotice] = useState<Notice | null>(null);

  const [searchText, setSearchText] = useState(queryParam);
  const [prevQueryParam, setPrevQueryParam] = useState(queryParam);
  if (prevQueryParam !== queryParam) {
    setPrevQueryParam(queryParam);
    setSearchText(queryParam);
  }

  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<UserSummary | null>(null);
  const [passwordTarget, setPasswordTarget] = useState<UserSummary | null>(null);

  const updateParams = useCallback(
    (updates: Record<string, string | null>) => {
      const next = new URLSearchParams(searchParams);
      for (const [key, value] of Object.entries(updates)) {
        if (value === null) {
          next.delete(key);
        } else {
          next.set(key, value);
        }
      }
      setSearchParams(next, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  // Debounced publish of the search text to the URL, which drives the
  // server fetch (same pattern as the catalog list screens).
  useEffect(() => {
    if (searchText === queryParam) {
      return;
    }
    const timer = window.setTimeout(() => {
      updateParams({ query: searchText || null, page: '1' });
    }, SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [searchText, queryParam, updateParams]);

  const fetchPage = useCallback(
    (signal: { cancelled: boolean }) => {
      return listUsers({
        query: queryParam || undefined,
        limit: pageSize,
        offset: (page - 1) * pageSize,
        sort,
      })
        .then((result) => {
          if (!signal.cancelled) {
            setPageData(result);
            setListError(null);
            setLoading(false);
          }
        })
        .catch((cause: unknown) => {
          if (!signal.cancelled) {
            setListError(toApiError(cause));
            setLoading(false);
          }
        });
    },
    [queryParam, page, pageSize, sort],
  );

  useEffect(() => {
    if (tab !== 'users') {
      return;
    }
    const signal = { cancelled: false };
    void fetchPage(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchPage, tab]);

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: 'username', header: t('users.column.username'), sortable: true },
      { key: 'email', header: t('users.column.email') },
      { key: 'roles', header: t('users.column.roles') },
      { key: 'status', header: t('common.status') },
      { key: 'createdAt', header: t('common.created'), sortable: true },
      { key: 'actions', header: t('users.column.actions') },
    ],
    [t],
  );

  const rows: EntityRow[] = useMemo(
    () =>
      (pageData?.items ?? []).map((user) => ({
        id: user.id,
        cells: {
          username: (
            <>
              <span className="melodee-work-item__title">{user.username}</span>
              {user.isAdmin ? (
                <StatusTag kind="info" label={t('users.roleAdministrator')} />
              ) : null}
              {user.isEditor ? <StatusTag kind="info" label={t('users.roleEditor')} /> : null}
            </>
          ),
          email: user.email,
          roles: user.roles.length > 0 ? user.roles.join(', ') : t('common.none'),
          status: (
            <>
              {user.enabled ? (
                <StatusTag kind="success" label={t('status.enabled')} />
              ) : (
                <StatusTag kind="neutral" label={t('status.disabled')} />
              )}
              {user.locked ? <StatusTag kind="error" label={t('status.locked')} /> : null}
            </>
          ),
          createdAt: (
            <span title={formatDateTime(user.createdAt)}>{formatRelativeTime(user.createdAt)}</span>
          ),
          actions: (
            <CellInteractive>
              <Button kind="ghost" size="sm" onClick={() => setEditTarget(user)}>
                {t('actions.edit')}
              </Button>
              <Button kind="ghost" size="sm" onClick={() => setPasswordTarget(user)}>
                {t('users.setPassword')}
              </Button>
            </CellInteractive>
          ),
        },
      })),
    [pageData, t],
  );

  const toolbar = (
    <>
      <Search
        size="sm"
        labelText={t('users.searchLabel')}
        placeholder={t('users.searchLabel')}
        value={searchText}
        onChange={(event) => setSearchText(event.target.value)}
        onClear={() => setSearchText('')}
      />
      <Button
        kind="ghost"
        size="sm"
        renderIcon={Renew}
        aria-label={t('actions.refresh')}
        onClick={() => void fetchPage({ cancelled: false })}
      >
        {t('actions.refresh')}
      </Button>
      <Button
        kind="primary"
        size="sm"
        renderIcon={Add}
        onClick={() => {
          setNotice(null);
          setCreateOpen(true);
        }}
      >
        {t('users.create')}
      </Button>
    </>
  );

  return (
    <>
      <PageHeader title={t('users.accessTitle')} subtitle={t('users.subtitle')} />
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      <Tabs
        selectedIndex={tab === 'groups' ? 1 : 0}
        onChange={({ selectedIndex }) =>
          updateParams({ tab: selectedIndex === 1 ? 'groups' : null })
        }
      >
        <TabList aria-label={t('users.tabsLabel')}>
          <Tab>{t('users.tabs.users')}</Tab>
          <Tab>{t('users.tabs.groups')}</Tab>
        </TabList>
        <TabPanels>
          <TabPanel>
            {listError ? (
              <ActionableNotification
                inline
                kind="error"
                lowContrast
                title={t('users.loadErrorTitle')}
                subtitle={listError.message}
                hideCloseButton
                actionButtonLabel={t('actions.retry')}
                onActionButtonClick={() => void fetchPage({ cancelled: false })}
              />
            ) : null}
            {loading && pageData === null ? (
              <Loading description={t('common.loading')} withOverlay={false} />
            ) : (
              // The accounts table is wider than the sm viewport; it scrolls
              // inside its own container rather than expanding the page
              // (UI-SPEC §8).
              <div className="melodee-table-scroll">
                <EntityTable
                  caption={t('users.tableCaption')}
                  columns={columns}
                  rows={rows}
                  totalRows={pageData?.total ?? 0}
                  toolbar={toolbar}
                  emptyTitle={t('users.emptyTitle')}
                  emptyDescription={t('users.emptyDescription')}
                />
              </div>
            )}
          </TabPanel>
          <TabPanel>
            <GroupsPanel />
          </TabPanel>
        </TabPanels>
      </Tabs>

      <CreateUserModal
        // Remount on each open so the form (including the password) resets.
        key={String(createOpen)}
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        onCreated={() => {
          setCreateOpen(false);
          setNotice({ kind: 'success', text: t('users.createSuccess') });
          void fetchPage({ cancelled: false });
        }}
      />
      <EditUserModal
        key={`edit-${editTarget?.id ?? 'closed'}`}
        user={editTarget}
        onClose={() => setEditTarget(null)}
        onSaved={() => {
          setEditTarget(null);
          setNotice({ kind: 'success', text: t('users.updateSuccess') });
          void fetchPage({ cancelled: false });
        }}
      />
      <SetPasswordModal
        key={`password-${passwordTarget?.id ?? 'closed'}`}
        user={passwordTarget}
        onClose={() => setPasswordTarget(null)}
        onSaved={() => {
          setPasswordTarget(null);
          setNotice({ kind: 'success', text: t('users.passwordSuccess') });
        }}
      />
    </>
  );
}

// ---- Create user ----

function CreateUserModal({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
}) {
  const { t } = useTranslation();
  const [username, setUsername] = useState('');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [isAdmin, setIsAdmin] = useState(false);
  const [isEditor, setIsEditor] = useState(false);
  const [roles, setRoles] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const passwordValid = [...password].length >= MIN_PASSWORD_CHARS;
  const canSubmit = username.trim().length > 0 && email.trim().length > 0 && passwordValid && !busy;

  // Closed modals are not rendered: Carbon keeps closed modals mounted,
  // which would leave stale buttons/dialogs in the accessibility tree.
  if (!open) {
    return null;
  }

  const submit = () => {
    if (!canSubmit) {
      return;
    }
    setBusy(true);
    setError(null);
    const request: UserCreateRequest = {
      username: username.trim(),
      email: email.trim(),
      password,
      isAdmin,
      isEditor,
    };
    const parsedRoles = parseRoles(roles);
    // Omitted grants the minimal streaming role server-side; only send an
    // explicit set when the operator typed one.
    if (parsedRoles.length > 0) {
      request.roles = parsedRoles;
    }
    createUser(request)
      .then(onCreated)
      .catch((cause: unknown) => setError(mutationErrorText(toApiError(cause), t)))
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={open}
      modalHeading={t('users.createTitle')}
      primaryButtonText={t('actions.create')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={!canSubmit}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      {error ? (
        <InlineNotification kind="error" lowContrast title={error} hideCloseButton />
      ) : null}
      <TextInput
        id="create-user-username"
        labelText={t('auth.username')}
        value={username}
        onChange={(event) => setUsername(event.target.value)}
      />
      <TextInput
        id="create-user-email"
        type="email"
        labelText={t('auth.email')}
        value={email}
        onChange={(event) => setEmail(event.target.value)}
      />
      <TextInput
        id="create-user-password"
        type="password"
        labelText={t('auth.password')}
        value={password}
        invalid={password.length > 0 && !passwordValid}
        invalidText={t('validation.passwordTooShort', { min: MIN_PASSWORD_CHARS })}
        helperText={t('users.passwordHelp', { min: MIN_PASSWORD_CHARS })}
        onChange={(event) => setPassword(event.target.value)}
      />
      <TextInput
        id="create-user-roles"
        labelText={t('users.rolesLabel')}
        helperText={t('users.rolesHelp')}
        value={roles}
        onChange={(event) => setRoles(event.target.value)}
      />
      <Checkbox
        id="create-user-admin"
        labelText={t('users.roleAdministrator')}
        checked={isAdmin}
        onChange={(_event, { checked }) => setIsAdmin(checked)}
      />
      <Checkbox
        id="create-user-editor"
        labelText={t('users.roleEditor')}
        checked={isEditor}
        onChange={(_event, { checked }) => setIsEditor(checked)}
      />
    </Modal>
  );
}

// ---- Edit user flags/roles ----

function EditUserModal({
  user,
  onClose,
  onSaved,
}: {
  user: UserSummary | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const [isAdmin, setIsAdmin] = useState(user?.isAdmin ?? false);
  const [isEditor, setIsEditor] = useState(user?.isEditor ?? false);
  const [enabled, setEnabled] = useState(user?.enabled ?? true);
  const [locked, setLocked] = useState(user?.locked ?? false);
  const [roles, setRoles] = useState((user?.roles ?? []).join(', '));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!user) {
    return null;
  }

  const submit = () => {
    if (busy) {
      return;
    }
    setBusy(true);
    setError(null);
    updateUser(user.id, {
      isAdmin,
      isEditor,
      enabled,
      locked,
      roles: parseRoles(roles),
    })
      .then(onSaved)
      .catch((cause: unknown) => setError(mutationErrorText(toApiError(cause), t)))
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={user !== null}
      modalHeading={t('users.editTitle', { username: user?.username ?? '' })}
      primaryButtonText={t('actions.saveChanges')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={busy}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      {error ? (
        <InlineNotification kind="error" lowContrast title={error} hideCloseButton />
      ) : null}
      <Checkbox
        id="edit-user-enabled"
        labelText={t('users.fieldEnabled')}
        checked={enabled}
        onChange={(_event, { checked }) => setEnabled(checked)}
      />
      <Checkbox
        id="edit-user-locked"
        labelText={t('users.fieldLocked')}
        checked={locked}
        onChange={(_event, { checked }) => setLocked(checked)}
      />
      <Checkbox
        id="edit-user-admin"
        labelText={t('users.roleAdministrator')}
        checked={isAdmin}
        onChange={(_event, { checked }) => setIsAdmin(checked)}
      />
      <Checkbox
        id="edit-user-editor"
        labelText={t('users.roleEditor')}
        checked={isEditor}
        onChange={(_event, { checked }) => setIsEditor(checked)}
      />
      <TextInput
        id="edit-user-roles"
        labelText={t('users.rolesLabel')}
        helperText={t('users.rolesHelp')}
        value={roles}
        onChange={(event) => setRoles(event.target.value)}
      />
    </Modal>
  );
}

// ---- Set password ----

function SetPasswordModal({
  user,
  onClose,
  onSaved,
}: {
  user: UserSummary | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useTranslation();
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const passwordValid = [...password].length >= MIN_PASSWORD_CHARS;
  const matches = password === confirm;
  const canSubmit = passwordValid && matches && !busy;

  if (!user) {
    return null;
  }

  const submit = () => {
    if (!canSubmit) {
      return;
    }
    setBusy(true);
    setError(null);
    setUserPassword(user.id, password)
      .then(onSaved)
      .catch((cause: unknown) => setError(mutationErrorText(toApiError(cause), t)))
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={user !== null}
      modalHeading={t('users.passwordTitle', { username: user?.username ?? '' })}
      primaryButtonText={t('users.setPassword')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={!canSubmit}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      {error ? (
        <InlineNotification kind="error" lowContrast title={error} hideCloseButton />
      ) : null}
      <p>{t('users.passwordNote')}</p>
      <TextInput
        id="set-password"
        type="password"
        labelText={t('auth.password')}
        value={password}
        invalid={password.length > 0 && !passwordValid}
        invalidText={t('validation.passwordTooShort', { min: MIN_PASSWORD_CHARS })}
        helperText={t('users.passwordHelp', { min: MIN_PASSWORD_CHARS })}
        onChange={(event) => setPassword(event.target.value)}
      />
      <TextInput
        id="set-password-confirm"
        type="password"
        labelText={t('auth.confirmPassword')}
        value={confirm}
        invalid={confirm.length > 0 && !matches}
        invalidText={t('validation.passwordsDoNotMatch')}
        onChange={(event) => setConfirm(event.target.value)}
      />
    </Modal>
  );
}
