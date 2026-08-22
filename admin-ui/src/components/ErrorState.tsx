import { Button } from '@carbon/react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router-dom';

import { EmptyState } from './EmptyState';
import { PageHeader } from './PageHeader';

/**
 * Localized, themed error states (PRD-WEB-002 — the legacy `/Error`
 * replacement). Renders inside the app shell/theme, never as a raw browser
 * page. Three distinct kinds share the vocabulary:
 *
 *  - `notFound`: the catch-all route for unknown in-app paths;
 *  - `forbidden`: signed in but not an administrator (no back link — every
 *    route is guarded, so "back to overview" would loop);
 *  - `unexpected`: render-error fallback from RouteErrorBoundary.
 */
export type ErrorKind = 'notFound' | 'forbidden' | 'unexpected';

const TITLE_KEYS: Record<ErrorKind, string> = {
  notFound: 'errors.notFoundTitle',
  forbidden: 'errors.forbiddenTitle',
  unexpected: 'errors.unexpectedTitle',
};

const DESCRIPTION_KEYS: Record<ErrorKind, string> = {
  notFound: 'errors.notFoundDescription',
  forbidden: 'errors.forbiddenDescription',
  unexpected: 'errors.unexpectedDescription',
};

export function ErrorState({ kind }: { kind: ErrorKind }) {
  const { t } = useTranslation();
  const title = t(TITLE_KEYS[kind]);
  const description = t(DESCRIPTION_KEYS[kind]);
  return (
    <>
      <PageHeader title={title} subtitle={description} />
      <EmptyState
        title={title}
        description={description}
        tone={kind === 'notFound' ? 'default' : 'error'}
        action={
          kind === 'forbidden' ? undefined : (
            <Button as={Link} to="/" kind="primary">
              {t('errors.backToOverview')}
            </Button>
          )
        }
      />
    </>
  );
}
