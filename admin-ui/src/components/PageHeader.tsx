import type { ReactNode } from 'react';

/**
 * Canonical page header (UI-SPEC §3, §6.1): breadcrumb line, localized title,
 * optional subtitle, and an action region aligned to the inline end. Every
 * screen renders exactly one page header; entity detail screens follow it with
 * a `HeroCard`.
 */
export function PageHeader({
  title,
  subtitle,
  actions,
  breadcrumb,
}: {
  title: string;
  subtitle?: string;
  actions?: ReactNode;
  /** Navigation context, e.g. "Review Queue › Men at Work". */
  breadcrumb?: ReactNode;
}) {
  return (
    <header className="melodee-page-header">
      {breadcrumb ? <div className="melodee-page-header__crumb">{breadcrumb}</div> : null}
      <div className="melodee-page-header__content">
        <div>
          <h1 className="melodee-page-header__title">{title}</h1>
          {subtitle ? <p className="melodee-page-header__subtitle">{subtitle}</p> : null}
        </div>
        {actions ? <div className="melodee-page-header__actions">{actions}</div> : null}
      </div>
    </header>
  );
}
