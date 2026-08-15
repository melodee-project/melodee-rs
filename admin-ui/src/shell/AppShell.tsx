import {
  Asleep,
  BrightnessContrast,
  Dashboard,
  DataBase,
  DocumentTasks,
  Folder,
  History,
  Identification,
  Language,
  List,
  Logout,
  Notification,
  Renew,
  Rule,
  Search as SearchIcon,
  Settings,
  SidePanelClose,
  SidePanelOpen,
  Sun,
  TaskView,
  UserAdmin,
  UserAvatar,
} from '@carbon/icons-react';
import {
  Content,
  Header,
  HeaderGlobalAction,
  HeaderGlobalBar,
  HeaderMenuButton,
  HeaderName,
  HeaderPanel,
  Loading,
  Modal,
  Search,
  SideNav,
  SideNavItems,
  SideNavLink,
  SideNavMenu,
  SideNavMenuItem,
  SkipToContent,
  SwitcherItem,
} from '@carbon/react';
import { Suspense, useCallback, useEffect, useMemo, useRef, useState, type ComponentType } from 'react';
import { useTranslation } from 'react-i18next';
import { Outlet, useLocation, useNavigate } from 'react-router-dom';

import { getSystemInfo } from '../api/admin';
import { useAuth } from '../auth/AuthContext';
import { CULTURES, dirFor } from '../i18n';
import i18n from '../i18n';
import { useThemeState, type Appearance } from '../theme/ThemeProvider';
import { HealthIndicator, type HealthState } from '../components/HealthIndicator';
import { RouteErrorBoundary } from '../components/RouteErrorBoundary';
import { useScrollRegionFocus } from '../components/scrollRegions';

/**
 * The fixed app shell (PRD-UI-001/004, UI-SPEC §3): 48px header with global
 * search (⌘K), server status, language, appearance, and user controls; a
 * persistent collapsible 256px/48px left navigation with the canonical IA;
 * a 24px status footer inside the nav.
 */

interface NavLeaf {
  key: string;
  path: string;
  icon: ComponentType;
}

interface NavGroup {
  key: string;
  labelKey: string;
  items: NavLeaf[];
}

const NAV_TREE: (NavLeaf | NavGroup)[] = [
  { key: 'overview', path: '/', icon: Dashboard },
  {
    key: 'processing',
    labelKey: 'nav.group.processing',
    items: [
      { key: 'processing', path: '/processing', icon: Renew },
      { key: 'reviewQueue', path: '/processing/review', icon: TaskView },
      { key: 'quarantine', path: '/processing/quarantine', icon: DocumentTasks },
      { key: 'history', path: '/processing/history', icon: History },
    ],
  },
  {
    key: 'catalog',
    labelKey: 'nav.group.catalog',
    items: [
      { key: 'artists', path: '/catalog/artists', icon: UserAdmin },
      { key: 'releases', path: '/catalog/releases', icon: List },
      { key: 'tracks', path: '/catalog/tracks', icon: Folder },
      { key: 'identityReview', path: '/catalog/identity', icon: Identification },
    ],
  },
  {
    key: 'policies',
    labelKey: 'nav.group.policies',
    // The PRD-UI-004 "Matching Rules" entry is deliberately absent: the
    // policy engine has exactly one rule model (Release policy rules) and
    // identity matching is evidence-based, so no matching-rule surface
    // exists to route to (Phase 10 decision log).
    items: [{ key: 'releaseRules', path: '/policies/releases', icon: Rule }],
  },
  {
    key: 'system',
    labelKey: 'nav.group.system',
    items: [
      { key: 'storage', path: '/system/storage', icon: DataBase },
      { key: 'workers', path: '/system/workers', icon: Renew },
      { key: 'search', path: '/system/search', icon: SearchIcon },
      { key: 'usersAccess', path: '/system/users', icon: UserAdmin },
      { key: 'settings', path: '/system/settings', icon: Settings },
      { key: 'diagnostics', path: '/system/diagnostics', icon: BrightnessContrast },
    ],
  },
];

const HEALTH_POLL_MS = 30_000;

function allLeaves(): NavLeaf[] {
  return NAV_TREE.flatMap((entry) => ('items' in entry ? entry.items : [entry]));
}

export function AppShell() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const { session, signOut } = useAuth();
  const { appearance, setAppearance } = useThemeState();

  // UI-SPEC §8: full shell (expanded nav) at lg+; md collapses to the icon
  // rail by default; sm overlays. Collapse when the viewport drops below lg;
  // never force-expand back — that is the operator's choice.
  const [navExpanded, setNavExpanded] = useState(
    () =>
      typeof window.matchMedia !== 'function' ||
      window.matchMedia('(min-width: 1056px)').matches,
  );

  useEffect(() => {
    if (typeof window.matchMedia !== 'function') {
      return;
    }
    const query = window.matchMedia('(min-width: 1056px)');
    const onChange = (event: MediaQueryListEvent) => {
      if (!event.matches) {
        setNavExpanded(false);
      }
    };
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchText, setSearchText] = useState('');
  const [openPanel, setOpenPanel] = useState<'language' | 'appearance' | 'user' | null>(null);
  const [health, setHealth] = useState<HealthState>('checking');

  // WCAG 2.1.1: keep horizontally scrolling regions (tables, chip strips)
  // keyboard-focusable whenever they overflow.
  useScrollRegionFocus();

  const username = session.kind === 'admin' ? session.username : null;

  // Server-status polling; the 401 session-expiry path is owned by the auth
  // provider, so polling suppresses its own auth event.
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      try {
        await getSystemInfo({ suppressAuthEvent: true });
        if (!cancelled) {
          setHealth('online');
        }
      } catch {
        if (!cancelled) {
          setHealth('offline');
        }
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), HEALTH_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  // ⌘K / Ctrl+K opens the command palette (PRD-UI-009); it is never the
  // only path — every destination is also in the side nav.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        setSearchOpen(true);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  const searchInputRef = useRef<HTMLInputElement>(null);

  const togglePanel = useCallback((panel: 'language' | 'appearance' | 'user') => {
    setOpenPanel((current) => (current === panel ? null : panel));
  }, []);

  const changeLanguage = useCallback((code: string) => {
    void i18n.changeLanguage(code);
    setOpenPanel(null);
  }, []);

  const appearanceLabel =
    appearance === 'system'
      ? t('shell.appearanceSystem')
      : appearance === 'light'
        ? t('shell.appearanceLight')
        : t('shell.appearanceDark');

  const filteredLeaves = useMemo(() => {
    const query = searchText.trim().toLowerCase();
    const leaves = allLeaves();
    if (!query) {
      return leaves;
    }
    return leaves.filter((leaf) =>
      t(`nav.${leaf.key}`).toLowerCase().includes(query),
    );
  }, [searchText, t]);

  const renderLeaf = (leaf: NavLeaf) => {
    const Icon = leaf.icon;
    const active =
      leaf.path === '/'
        ? location.pathname === '/'
        : location.pathname === leaf.path || location.pathname.startsWith(`${leaf.path}/`);
    return (
      <SideNavLink
        key={leaf.key}
        renderIcon={Icon}
        isActive={active}
        onClick={() => navigate(leaf.path)}
      >
        {t(`nav.${leaf.key}`)}
      </SideNavLink>
    );
  };

  // Expansion is owned by the shell (desktop starts expanded; the rail
  // collapse is the footer/menu control). The subtree is returned directly:
  // Carbon's HeaderContainer treats its render prop as a component type, so
  // an inline closure would remount the whole routed screen (wiping local
  // state) on every shell re-render.
  const expanded = navExpanded;
  return (
    <>
      <Header aria-label="Melodee Admin">
        <SkipToContent>{t('shell.skipToContent')}</SkipToContent>
        <HeaderMenuButton
          aria-label={expanded ? t('shell.collapseNav') : t('shell.expandNav')}
          onClick={() => setNavExpanded((current) => !current)}
          isActive={expanded}
          isCollapsible
        />
        <HeaderName href="/admin/" prefix="Melodee">
          Admin
        </HeaderName>
        <HeaderGlobalBar>
          <span className="melodee-shell__health">
            <HealthIndicator state={health} />
          </span>
          <HeaderGlobalAction
            aria-label={t('shell.openSearch', { shortcut: '⌘K' })}
            tooltipAlignment="end"
            onClick={() => setSearchOpen(true)}
          >
            <SearchIcon size={20} />
          </HeaderGlobalAction>
          <HeaderGlobalAction
            aria-label={t('shell.notifications')}
            tooltipAlignment="end"
            onClick={() => navigate('/system/diagnostics')}
          >
            <Notification size={20} />
          </HeaderGlobalAction>
          <HeaderGlobalAction
            aria-label={t('shell.language')}
            tooltipAlignment="end"
            isActive={openPanel === 'language'}
            onClick={() => togglePanel('language')}
          >
            <Language size={20} />
          </HeaderGlobalAction>
          <HeaderGlobalAction
            aria-label={`${t('shell.appearance')}: ${appearanceLabel}`}
            tooltipAlignment="end"
            isActive={openPanel === 'appearance'}
            onClick={() => togglePanel('appearance')}
          >
            {appearance === 'dark' ? <Asleep size={20} /> : <Sun size={20} />}
          </HeaderGlobalAction>
          <HeaderGlobalAction
            aria-label={t('shell.userMenu')}
            tooltipAlignment="end"
            isActive={openPanel === 'user'}
            onClick={() => togglePanel('user')}
          >
            <UserAvatar size={20} />
          </HeaderGlobalAction>
        </HeaderGlobalBar>
        <HeaderPanel
          aria-label={t('shell.language')}
          expanded={openPanel === 'language'}
          href="#language"
        >
          {/* SwitcherItem renders an <li>; it must sit inside a list
              (WCAG 1.3.1 — axe listitem rule). */}
          <ul className="melodee-shell__panel-list">
            {CULTURES.map((culture) => (
              <SwitcherItem
                key={culture.code}
                aria-label={culture.nativeName}
                isSelected={i18n.language === culture.code}
                onClick={() => changeLanguage(culture.code)}
              >
                {culture.nativeName}
                {dirFor(culture.code) === 'rtl' ? ' (RTL)' : ''}
              </SwitcherItem>
            ))}
          </ul>
        </HeaderPanel>
        <HeaderPanel
          aria-label={t('shell.appearance')}
          expanded={openPanel === 'appearance'}
          href="#appearance"
        >
          <ul className="melodee-shell__panel-list">
            {(['system', 'light', 'dark'] as Appearance[]).map((mode) => (
              <SwitcherItem
                key={mode}
                aria-label={t(`shell.appearance${mode[0]?.toUpperCase()}${mode.slice(1)}`)}
                isSelected={appearance === mode}
                onClick={() => {
                  setAppearance(mode);
                  setOpenPanel(null);
                }}
              >
                {t(`shell.appearance${mode[0]?.toUpperCase()}${mode.slice(1)}`)}
              </SwitcherItem>
            ))}
          </ul>
        </HeaderPanel>
        <HeaderPanel
          aria-label={t('shell.userMenu')}
          expanded={openPanel === 'user'}
          href="#user"
        >
          <ul className="melodee-shell__panel-list">
            {username ? (
              <SwitcherItem aria-label={t('auth.signedInAs', { name: username })} onClick={() => undefined}>
                {t('auth.signedInAs', { name: username })}
              </SwitcherItem>
            ) : null}
            <SwitcherItem
              aria-label={t('auth.signOut')}
              onClick={() => {
                setOpenPanel(null);
                void signOut();
              }}
            >
              <Logout size={16} aria-hidden /> {t('auth.signOut')}
            </SwitcherItem>
          </ul>
        </HeaderPanel>
      </Header>
      <SideNav
        aria-label={t('shell.mainNavigation')}
        expanded={expanded}
        isRail={!expanded}
        isPersistent
        isChildOfHeader
      >
        <SideNavItems>
          {NAV_TREE.map((entry) =>
            'items' in entry ? (
              <SideNavMenu
                key={entry.key}
                title={t(entry.labelKey)}
                renderIcon={Folder}
                defaultExpanded={entry.items.some(
                  (item) =>
                    location.pathname === item.path ||
                    location.pathname.startsWith(`${item.path}/`),
                )}
              >
                {entry.items.map((leaf) => {
                  const active =
                    location.pathname === leaf.path ||
                    location.pathname.startsWith(`${leaf.path}/`);
                  return (
                    <SideNavMenuItem
                      key={leaf.key}
                      isActive={active}
                      onClick={() => navigate(leaf.path)}
                    >
                      {t(`nav.${leaf.key}`)}
                    </SideNavMenuItem>
                  );
                })}
              </SideNavMenu>
            ) : (
              renderLeaf(entry)
            ),
          )}
        </SideNavItems>
        <div className="melodee-shell__footer">
          <HeaderGlobalAction
            aria-label={expanded ? t('shell.collapseNav') : t('shell.expandNav')}
            onClick={() => setNavExpanded((current) => !current)}
          >
            {expanded ? <SidePanelClose size={16} /> : <SidePanelOpen size={16} />}
          </HeaderGlobalAction>
          {expanded ? (
            <span>
              {t(`health.${health}`)} · {i18n.language} · {appearanceLabel}
            </span>
          ) : null}
        </div>
      </SideNav>
      <Content id="main-content" className="melodee-shell__content">
        {/* Lazy route screens suspend here so a chunk fetch never
            unmounts the shell chrome (Phase 10 W4-5 code splitting). */}
        {/* A screen that throws during render degrades to the localized
            unexpected-error state; navigation (pathname change) resets it. */}
        <RouteErrorBoundary resetKey={location.pathname}>
          <Suspense fallback={<Loading description={t('common.loading')} withOverlay={false} />}>
            <Outlet />
          </Suspense>
        </RouteErrorBoundary>
      </Content>
      <Modal
        open={searchOpen}
        modalHeading={t('shell.commandPaletteTitle')}
        passiveModal
        onRequestClose={() => {
          setSearchOpen(false);
          setSearchText('');
        }}
      >
        <Search
          ref={searchInputRef}
          labelText={t('shell.search')}
          placeholder={t('shell.searchPlaceholder')}
          value={searchText}
          onChange={(event) => setSearchText(event.target.value)}
          autoFocus
        />
        {filteredLeaves.length === 0 ? (
          <p>{t('shell.commandPaletteEmpty')}</p>
        ) : (
          <ul style={{ listStyle: 'none', paddingInlineStart: 0 }}>
            {filteredLeaves.map((leaf) => (
              <li key={leaf.key}>
                <SwitcherItem
                  aria-label={t(`nav.${leaf.key}`)}
                  onClick={() => {
                    setSearchOpen(false);
                    setSearchText('');
                    navigate(leaf.path);
                  }}
                >
                  {t(`nav.${leaf.key}`)}
                </SwitcherItem>
              </li>
            ))}
          </ul>
        )}
      </Modal>
    </>
  );
}
