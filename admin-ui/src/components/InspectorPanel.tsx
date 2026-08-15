import { Close } from '@carbon/icons-react';
import { Heading, IconButton } from '@carbon/react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * Dismissible 400px right rail (UI-SPEC §3). Selection opens it without
 * losing queue position; content reflows. At ≤ md it becomes full-width per
 * tokens.css.
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
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBlockEnd: '0.5rem',
        }}
      >
        <Heading style={{ fontSize: '1rem' }}>{title}</Heading>
        <IconButton kind="ghost" label={t('inspector.close')} onClick={onClose}>
          <Close />
        </IconButton>
      </div>
      {children}
    </aside>
  );
}
