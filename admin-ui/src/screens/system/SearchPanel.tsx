import {
  ActionableNotification,
  Button,
  Heading,
  InlineNotification,
  Modal,
} from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { getSearchStatus, requestSearchRebuild } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { SearchStatus } from '../../api/types';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { formatNumber } from '../../i18n/format';

/**
 * Search index status + rebuild trigger panel (SPEC §22), shared by the
 * System → Workers rail and the System → Search screen so the status and
 * rebuild logic exist exactly once.
 *
 * Honesty rules: the rebuild button never fakes progress (the search worker
 * claims the durable job in the background), `409 rebuild_in_progress` /
 * `409 search_disabled` surface as localized notifications, and the trigger
 * is disabled with the reason when the search worker is off.
 *
 * `refreshKey` lets a parent screen's refresh action re-run the status fetch
 * without owning the fetch itself.
 */

interface Notice {
  kind: 'success' | 'error' | 'info' | 'warning';
  text: string;
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

export function SearchPanel({
  refreshKey = 0,
  headingId = 'system-search-title',
}: {
  refreshKey?: number;
  headingId?: string;
}) {
  const { t } = useTranslation();
  const [searchStatus, setSearchStatus] = useState<SearchStatus | null>(null);
  const [searchError, setSearchError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [rebuildConfirmOpen, setRebuildConfirmOpen] = useState(false);
  const [rebuildBusy, setRebuildBusy] = useState(false);

  const fetchSearch = useCallback((signal: { cancelled: boolean }) => {
    return getSearchStatus()
      .then((result) => {
        if (!signal.cancelled) {
          setSearchStatus(result);
          setSearchError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setSearchError(toApiError(cause));
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchSearch(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchSearch, refreshKey]);

  const confirmRebuild = () => {
    setRebuildBusy(true);
    requestSearchRebuild()
      .then((job) => {
        setNotice({
          kind: 'success',
          text: t('system.search.rebuildRequested', { jobId: job.jobId }),
        });
        void fetchSearch({ cancelled: false });
      })
      .catch((cause: unknown) => {
        const error = toApiError(cause);
        if (error.status === 409 && error.code === 'rebuild_in_progress') {
          setNotice({ kind: 'warning', text: t('system.search.rebuildInProgress') });
        } else if (error.status === 409 && error.code === 'search_disabled') {
          setNotice({ kind: 'info', text: t('system.search.disabledNote') });
        } else {
          setNotice({
            kind: 'error',
            text: t('system.search.rebuildError', { message: error.message }),
          });
        }
      })
      .finally(() => {
        setRebuildBusy(false);
        setRebuildConfirmOpen(false);
      });
  };

  return (
    <>
      <section className="melodee-panel" aria-labelledby={headingId}>
        <Heading id={headingId} className="melodee-panel__title">
          {t('system.search.title')}
        </Heading>
        {notice ? (
          <InlineNotification
            kind={notice.kind}
            lowContrast
            title={notice.text}
            hideCloseButton={false}
            onCloseButtonClick={() => setNotice(null)}
          />
        ) : null}
        {searchError ? (
          <ActionableNotification
            inline
            kind="error"
            lowContrast
            title={t('system.loadErrorTitle')}
            subtitle={searchError.message}
            hideCloseButton
            actionButtonLabel={t('actions.retry')}
            onActionButtonClick={() => void fetchSearch({ cancelled: false })}
          />
        ) : null}
        {searchStatus && !searchStatus.enabled ? (
          <InlineNotification
            kind="info"
            lowContrast
            title={t('system.search.disabledTitle')}
            subtitle={t('system.search.disabledNote')}
            hideCloseButton
          />
        ) : null}
        {searchStatus?.enabled ? (
          <dl className="melodee-fact-list">
            <div className="melodee-fact">
              <dt>{t('system.search.pending')}</dt>
              <dd>{formatNumber(searchStatus.projection?.pending ?? 0)}</dd>
            </div>
            <div className="melodee-fact">
              <dt>{t('system.search.failed')}</dt>
              <dd>{formatNumber(searchStatus.projection?.failed ?? 0)}</dd>
            </div>
            <div className="melodee-fact">
              <dt>{t('system.search.leased')}</dt>
              <dd>{formatNumber(searchStatus.projection?.leased ?? 0)}</dd>
            </div>
            <div className="melodee-fact">
              <dt>{t('system.search.generation')}</dt>
              <dd>{formatNumber(searchStatus.generation?.total ?? 0)}</dd>
            </div>
            {searchStatus.projection?.latestRebuildStatus ? (
              <div className="melodee-fact">
                <dt>{t('system.search.lastRebuild')}</dt>
                <dd>
                  <ProcessingStateTag state={searchStatus.projection.latestRebuildStatus} />
                  {searchStatus.projection.latestRebuildError ? (
                    <span className="melodee-fact__detail">
                      {searchStatus.projection.latestRebuildError}
                    </span>
                  ) : null}
                </dd>
              </div>
            ) : null}
          </dl>
        ) : null}
        <div className="melodee-panel__footer">
          <Button
            kind="primary"
            size="sm"
            disabled={!searchStatus?.enabled || rebuildBusy}
            title={!searchStatus?.enabled ? t('system.search.disabledNote') : undefined}
            onClick={() => setRebuildConfirmOpen(true)}
          >
            {t('system.search.rebuild')}
          </Button>
        </div>
      </section>
      <Modal
        open={rebuildConfirmOpen}
        modalHeading={t('system.search.rebuildConfirmTitle')}
        primaryButtonText={t('system.search.rebuild')}
        secondaryButtonText={t('actions.cancel')}
        primaryButtonDisabled={rebuildBusy}
        onRequestClose={() => setRebuildConfirmOpen(false)}
        onRequestSubmit={confirmRebuild}
      >
        <p>{t('system.search.rebuildConfirmBody')}</p>
      </Modal>
    </>
  );
}
