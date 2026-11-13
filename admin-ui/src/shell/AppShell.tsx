import {
  ChartLine,
  Activity,
  Archive,
  Asleep,
  Catalog,
  Dashboard,
  DataBase,
  Globe,
  History,
  Identification,
  Logout,
  Music,
  Notification,
  Renew,
  Rule,
  Search as SearchIcon,
  Settings,
  SidePanelClose,
  SidePanelOpen,
  Sun,
  TaskView,
  User,
  UserMultiple,
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
 * The fixed app shell (PRD-UI-001/004, UI-SPEC §3, ADR-0184): a dark brand rail
 * with a monogram tile, wordmark, and `ADMIN` micro-label; a 48px header with
 * one inset global search entry (⌘K), server status, notifications, language,
 * appearance, and user controls; a persistent collapsible 256px/48px left
 * navigation with the canonical PRD-UI-004 IA and icon vocabulary; and a
 * status strip at the foot of the rail.
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
      { key: 'quarantine', path: '/processing/quarantine', icon: Archive },
      { key: 'history', path: '/processing/history', icon: History },
    ],
  },
  {
    key: 'catalog',
    labelKey: 'nav.group.catalog',
    items: [
      { key: 'artists', path: '/catalog/artists', icon: User },
      { key: 'releases', path: '/catalog/releases', icon: Catalog },
      { key: 'tracks', path: '/catalog/tracks', icon: Music },
      { key: 'identityReview', path: '/catalog/identity', icon: Identification },
    ],
  },
  {
    key: 'policies',
    labelKey: 'nav.group.policies',
    // The PRD-UI-004 "Matching Rules" entry is deliberately absent: the
    // policy engine has exactly one rule model (Release policy rules) and
    // identity matching is evidence-based, so no matching-rule surface
    // exists to route to (ADR-0184 decision log).
    items: [{ key: 'releaseRules', path: '/policies/releases', icon: Rule }],
  },
  {
    key: 'system',
    labelKey: 'nav.group.system',
    items: [
      { key: 'stats', path: '/system/stats', icon: ChartLine },
      { key: 'storage', path: '/system/storage', icon: DataBase },
      { key: 'workers', path: '/system/workers', icon: Renew },
      { key: 'search', path: '/system/search', icon: SearchIcon },
      { key: 'usersAccess', path: '/system/users', icon: UserMultiple },
      { key: 'settings', path: '/system/settings', icon: Settings },
      { key: 'diagnostics', path: '/system/diagnostics', icon: Activity },
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
  const currentCulture = CULTURES.find((culture) => culture.code === i18n.language);

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
        className="melodee-shell__nav-link"
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
  const shellStateClass = expanded ? 'melodee-shell--expanded' : 'melodee-shell--rail';
  return (
    <>
      <div className={`melodee-shell__brand ${shellStateClass}`}>
        <button
          type="button"
          className="melodee-shell__brand-link"
          tabIndex={-1}
          onClick={() => navigate('/')}
        >
          <img
            className="melodee-shell__brand-mark"
            src="/admin/brand/melodee-circular-badge.svg"
            alt=""
          />
          <span className="melodee-shell__brand-wordmark">
            <span className="melodee-shell__brand-name">melodee</span>
            <span className="melodee-shell__brand-subtitle">Admin</span>
          </span>
          {/* The wordmark is hidden in the collapsed rail, so the accessible
              name must not depend on it. */}
          <span className="cds--visually-hidden">{t('shell.brandName')}</span>
        </button>
      </div>
      <Header className={`melodee-shell__header ${shellStateClass}`} aria-label="Melodee Admin">
        <SkipToContent>{t('shell.skipToContent')}</SkipToContent>
        <HeaderMenuButton
          className="melodee-shell__header-menu"
          aria-label={expanded ? t('shell.collapseNav') : t('shell.expandNav')}
          onClick={() => setNavExpanded((current) => !current)}
          isActive={expanded}
          isCollapsible
        />
        <HeaderName className="melodee-shell__header-name" href="/admin/" prefix="Melodee">
          Admin
        </HeaderName>
        <button
          type="button"
          className="melodee-shell__command-entry"
          onClick={() => setSearchOpen(true)}
        >
          <SearchIcon size={16} aria-hidden />
          <span>{t('shell.searchPlaceholder')}</span>
          <kbd>⌘K</kbd>
        </button>
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
          {/* Reference composition: locale name, not a bare icon, so the active
              language is readable without opening the panel. Stays a
              HeaderGlobalAction so Carbon's 48px hit area and popover wrapper
              are preserved. */}
          <HeaderGlobalAction
            aria-label={`${t('shell.language')}: ${
              currentCulture?.nativeName ?? i18n.language
            }`}
            tooltipAlignment="end"
            isActive={openPanel === 'language'}
            onClick={() => togglePanel('language')}
          >
            <span className="melodee-shell__action-with-label">
              <Globe size={20} aria-hidden />
              <span className="melodee-shell__locale-name">
                {currentCulture?.nativeName ?? i18n.language}
              </span>
            </span>
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
            aria-label={
              username ? `${t('shell.userMenu')}: ${username}` : t('shell.userMenu')
            }
            tooltipAlignment="end"
            isActive={openPanel === 'user'}
            onClick={() => togglePanel('user')}
          >
            <span className="melodee-shell__action-with-label">
              {username ? (
                <span className="melodee-shell__avatar" aria-hidden>
                  {username.slice(0, 1).toUpperCase()}
                </span>
              ) : (
                <UserAvatar size={20} aria-hidden />
              )}
              {username ? <span className="melodee-shell__user-name">{username}</span> : null}
            </span>
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
        className={`melodee-shell__nav ${shellStateClass}`}
        aria-label={t('shell.mainNavigation')}
        expanded={expanded}
        isRail={!expanded}
        isPersistent
        isChildOfHeader
      >
        <SideNavItems>
          {NAV_TREE.flatMap((entry) =>
            'items' in entry
              ? [
                  <li className="melodee-shell__nav-group" key={`${entry.key}-label`}>
                    <span className="melodee-shell__nav-group-label">
                      {t(entry.labelKey)}
                    </span>
                  </li>,
                  ...entry.items.map(renderLeaf),
                ]
              : [renderLeaf(entry)],
          )}
        </SideNavItems>
        <div className="melodee-shell__footer">
          {/* Server status lives once, in the header. The footer carries only the
              locale, appearance, and the collapse control. */}
          <span className="melodee-shell__footer-controls">
            {expanded ? (
              <span className="melodee-shell__footer-meta">
                <span>{currentCulture?.nativeName ?? i18n.language}</span>
                <span>{appearanceLabel}</span>
              </span>
            ) : null}
            <HeaderGlobalAction
              aria-label={expanded ? t('shell.collapseNav') : t('shell.expandNav')}
              onClick={() => setNavExpanded((current) => !current)}
            >
              {expanded ? <SidePanelClose size={16} /> : <SidePanelOpen size={16} />}
            </HeaderGlobalAction>
          </span>
        </div>
      </SideNav>
      <Content id="main-content" className={`melodee-shell__content ${shellStateClass}`}>
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
      {/* Modals mount only while open: Carbon keeps a closed Modal in the DOM,
          where its fixed container still contributes to layout width (it
          widened the mobile layout viewport past the screen), and its labelled
          content would duplicate dialog roles for assistive tech. */}
      {searchOpen ? (
        <Modal
          open
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
            <ul className="melodee-shell__panel-list">
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
      ) : null}
    </>
  );
}
