import { Heading } from '@carbon/react';
import type { ReactNode } from 'react';

/**
 * Canonical page header: localized title, optional subtitle, and an action
 * region aligned to the inline end.
 */
export function PageHeader({
  title,
  subtitle,
  actions,
}: {
  title: string;
  subtitle?: string;
  actions?: ReactNode;
}) {
  return (
    <header className="melodee-page-header">
      <div
        style={{
          display: 'flex',
          alignItems: 'flex-start',
          justifyContent: 'space-between',
          gap: '1rem',
        }}
      >
        <div>
          <Heading style={{ fontSize: '1.5rem' }}>{title}</Heading>
          {subtitle ? <p className="melodee-page-header__subtitle">{subtitle}</p> : null}
        </div>
        {actions ? <div>{actions}</div> : null}
      </div>
    </header>
  );
}
