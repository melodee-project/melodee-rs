import type { ReactNode } from 'react';

/**
 * Entity hero (UI-SPEC §6.1, ADR-0184): the reference's top-of-screen summary
 * for a Release, Artist, review subject, or identity observation. One hero per
 * screen — media slot, name, subtitle, status chips, a fact strip, and the
 * primary actions.
 */
export interface HeroFact {
  label: string;
  value: ReactNode;
}

export function HeroCard({
  media,
  title,
  subtitle,
  chips,
  facts,
  actions,
  ariaLabel,
}: {
  media?: ReactNode;
  title: ReactNode;
  subtitle?: ReactNode;
  chips?: ReactNode;
  facts?: HeroFact[];
  actions?: ReactNode;
  ariaLabel?: string;
}) {
  return (
    <section className="melodee-hero" aria-label={ariaLabel}>
      {media ? <div className="melodee-hero__media">{media}</div> : null}
      <div className="melodee-hero__main">
        <h1 className="melodee-hero__title">{title}</h1>
        {subtitle ? <p className="melodee-hero__subtitle">{subtitle}</p> : null}
        {chips ? <div className="melodee-hero__chips">{chips}</div> : null}
        {facts && facts.length > 0 ? (
          <dl className="melodee-hero__facts">
            {facts.map((fact) => (
              <div className="melodee-hero__fact" key={fact.label}>
                <dt className="melodee-hero__fact-label">{fact.label}</dt>
                <dd className="melodee-hero__fact-value" style={{ margin: 0 }}>
                  {fact.value}
                </dd>
              </div>
            ))}
          </dl>
        ) : null}
      </div>
      {actions ? <div className="melodee-hero__actions">{actions}</div> : null}
    </section>
  );
}
