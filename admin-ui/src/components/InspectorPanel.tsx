import { Close } from '@carbon/icons-react';
import { IconButton } from '@carbon/react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * Dismissible 400px right rail (UI-SPEC §3, §6.1). Selection opens it without
 * losing queue position; content reflows. Rendered as a `Surface` card so the
 * inspector reads as a peer of the queue card rather than a page divider. At
 * ≤ md it becomes full-width per screens.css.
 */
export function InspectorPanel({
  title,
  open,
  onClose,
  children,
}: {
  title: string;
  open: boolean;
  onClose: () => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  if (!open) {
    return null;
  }
  return (
    <aside className="melodee-inspector" aria-label={t('inspector.title')}>
      <div className="melodee-inspector__header">
        <h2 className="melodee-inspector__title">{title}</h2>
        <IconButton kind="ghost" label={t('inspector.close')} onClick={onClose}>
          <Close />
        </IconButton>
      </div>
      {children}
    </aside>
  );
}
