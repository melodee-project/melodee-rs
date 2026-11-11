import type { ReactNode } from 'react';

/**
 * The one card treatment (UI-SPEC §6.1, ADR-0184). Every panel, rail card and
 * table container in the Admin UI is a Surface, so the border, radius, padding
 * and heading scale cannot drift per screen.
 *
 * `flush` removes padding for card-wrapped tables; `inset` uses the muted
 * surface for secondary content; `quiet` drops the border for in-flow groups.
 */
export function Surface({
  title,
  subtitle,
  actions,
  footer,
  children,
  headingId,
  headingLevel = 2,
  variant = 'default',
  className,
  ariaLabel,
}: {
  title?: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  footer?: ReactNode;
  children?: ReactNode;
  headingId?: string;
  headingLevel?: 2 | 3 | 4;
  variant?: 'default' | 'flush' | 'inset' | 'quiet';
  className?: string;
  /** Accessible name when the card has no visible title. */
  ariaLabel?: string;
}) {
  const Heading = `h${headingLevel}` as const;
  const variantClass =
    variant === 'default' ? '' : ` melodee-surface--${variant}`;
  const hasHeader = Boolean(title || subtitle || actions);
  return (
    <section
      className={`melodee-surface${variantClass}${className ? ` ${className}` : ''}`}
      aria-labelledby={title && headingId ? headingId : undefined}
      aria-label={!title ? ariaLabel : undefined}
    >
      {hasHeader ? (
        <div className="melodee-surface__header">
          <div className="melodee-surface__heading">
            {title ? (
              <Heading id={headingId} className="melodee-surface__title">
                {title}
              </Heading>
            ) : null}
            {subtitle ? <p className="melodee-surface__subtitle">{subtitle}</p> : null}
          </div>
          {actions ? <div className="melodee-surface__actions">{actions}</div> : null}
        </div>
      ) : null}
      <div className="melodee-surface__body">{children}</div>
      {footer ? <div className="melodee-surface__footer">{footer}</div> : null}
    </section>
  );
}
