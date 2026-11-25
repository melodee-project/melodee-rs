import { Button, Loading } from '@carbon/react';
import { Suspense, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Link, Navigate, Outlet, useLocation } from 'react-router-dom';

import { ErrorState } from '../components/ErrorState';
import { SessionAvatar } from '../shell/SessionAvatar';
import { useAuth } from './AuthContext';

/**
 * Route guard (ADR-0139). UX only — the server enforces authorization on
 * every request. Editors enter the review queue and listeners see access denied.
 */
export function RequireAuth({ children }: { children: ReactNode }) {
  const { session, signOut } = useAuth();
  const location = useLocation();
  const { t } = useTranslation();

  if (session.kind === 'loading') {
    return <Loading description={t('common.loading')} withOverlay={false} />;
  }
  if (session.kind === 'anonymous') {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }
  if (session.kind === 'forbidden') {
    // Distinct from not-found (PRD-WEB-002): signed in, not an administrator.
    return <ErrorState kind="forbidden" />;
  }
  if (session.kind === 'editor') {
    // Editors can review Releases and inspect/delete catalog entities at any
    // time (ADR-0211); every other administration surface stays admin-only.
    if (
      !/^\/processing\/review(?:\/[^/]+)?\/?$/.test(location.pathname) &&
      !/^\/catalog\/(?:artists|releases|tracks)(?:\/[^/]+)?\/?$/.test(location.pathname)
    ) {
      return <Navigate to="/processing/review" replace />;
    }
    return (
      <>
        <header className="melodee-editor-header">
          <Link to="/processing/review">{t('nav.reviewQueue')}</Link>
          <Link to="/catalog/releases">{t('nav.releases')}</Link>
          <div className="melodee-shell__action-with-label">
            <SessionAvatar username={session.username} />
            <Button kind="ghost" onClick={() => void signOut()}>{t('auth.signOut')}</Button>
          </div>
        </header>
        <main className="melodee-editor-content">
          <Suspense fallback={<Loading description={t('common.loading')} withOverlay={false} />}>
            <Outlet context={{ editorOnly: true }} />
          </Suspense>
        </main>
      </>
    );
  }
  return <>{children}</>;
}
