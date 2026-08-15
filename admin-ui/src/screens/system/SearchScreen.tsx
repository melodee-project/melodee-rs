import { Renew } from '@carbon/icons-react';
import { Button, Heading } from '@carbon/react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { PageHeader } from '../../components/PageHeader';
import { SearchPanel } from './SearchPanel';

/**
 * System → Search (SPEC §22): the Tantivy search index status and the
 * durable, confirmation-gated rebuild trigger — the same SearchPanel the
 * Workers screen embeds, so status/rebuild logic exists exactly once.
 *
 * Honesty rule: the admin API exposes search status and rebuild only; MQL
 * query/autocomplete/parse/explain tooling remains the CLI (`melodee search
 * …`) and the native search API. There is deliberately no query box here.
 */
export function SearchScreen() {
  const { t } = useTranslation();
  const [refreshKey, setRefreshKey] = useState(0);

  return (
    <>
      <PageHeader
        title={t('system.searchTitle')}
        subtitle={t('system.search.screenSubtitle')}
        actions={
          <Button
            kind="ghost"
            size="sm"
            renderIcon={Renew}
            onClick={() => setRefreshKey((key) => key + 1)}
          >
            {t('actions.refresh')}
          </Button>
        }
      />
      <SearchPanel refreshKey={refreshKey} />
      <section className="melodee-panel" aria-labelledby="system-search-query-title">
        <Heading id="system-search-query-title" className="melodee-panel__title">
          {t('system.search.queryNoteTitle')}
        </Heading>
        <p className="melodee-panel__subtitle">{t('system.search.queryNote')}</p>
      </section>
    </>
  );
}
