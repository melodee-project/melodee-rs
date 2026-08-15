import { useTranslation } from 'react-i18next';

import { EmptyState } from '../components/EmptyState';
import { PageHeader } from '../components/PageHeader';

/**
 * Placeholder for IA entries whose real screens land in later Phase 10
 * waves. The shell, navigation, theming, and localization are real; the
 * screen body is an honest placeholder, not mockup text.
 */
export function PlaceholderScreen({ titleKey, subtitleKey }: { titleKey: string; subtitleKey?: string }) {
  const { t } = useTranslation();
  return (
    <>
      <PageHeader title={t(titleKey)} subtitle={subtitleKey ? t(subtitleKey) : undefined} />
      <EmptyState
        title={t('empty.placeholderTitle')}
        description={t('empty.placeholderDescription')}
      />
    </>
  );
}
