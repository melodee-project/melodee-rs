import { Renew } from '@carbon/icons-react';
import { ActionableNotification, Button, Loading } from '@carbon/react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { getStats } from '../../api/admin';
import { ApiError } from '../../api/client';
import type { Stats } from '../../api/types';
import { MetricCard } from '../../components/MetricCard';
import { PageHeader } from '../../components/PageHeader';
import { Surface } from '../../components/Surface';
import { formatBytes, formatDuration, formatNumber, formatRelativeTime } from '../../i18n/format';
import { BarList } from './BarList';
import { DecadeBars } from './DecadeBars';
import { PlaysChart } from './PlaysChart';

/**
 * Statistics workspace (PRD-UI-001, ADR-0184).
 *
 * Answers "what is in this library and how is it used?" with three panels —
 * media library, listening activity, and the database — all computed by the
 * server in bounded aggregate queries (`GET /api/admin/v1/stats`). Nothing is
 * re-aggregated client-side, so the numbers match what the CLI and SQL report.
 *
 * Charts follow the ADR-0184 rule for this surface: no chart library, each plot
 * is a token-styled SVG with `role="img"`, a localized label, and the same data
 * as a visually-hidden table for assistive technology.
 */

const REFRESH_MS = 60_000;

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null);
}

export function StatsScreen() {
  const { t } = useTranslation();
  const [stats, setStats] = useState<Stats | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);

  // `loading` is only raised from the refresh handler: setting it synchronously in
  // the effect would trip the repo's no-set-state-in-effect rule, and the first
  // render already shows the loading state via `stats === null`.
  const load = useCallback((signal: { cancelled: boolean }) => {
    return getStats()
      .then((payload) => {
        if (!signal.cancelled) {
          setStats(payload);
          setError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setError(toApiError(cause));
        }
      })
      .finally(() => {
        if (!signal.cancelled) {
          setLoading(false);
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void load(signal);
    const timer = window.setInterval(() => {
      void load(signal);
    }, REFRESH_MS);
    return () => {
      signal.cancelled = true;
      window.clearInterval(timer);
    };
  }, [load]);

  if (error && !stats) {
    return (
      <ActionableNotification
        inline
        kind="error"
        lowContrast
        title={t('stats.loadErrorTitle')}
        subtitle={error.message}
        hideCloseButton
        actionButtonLabel={t('actions.retry')}
        onActionButtonClick={() => void load({ cancelled: false })}
      />
    );
  }
  if (!stats) {
    return <Loading description={t('common.loading')} withOverlay={false} />;
  }

  const { library, listening, database } = stats;
  const losslessShare =
    library.tracks > 0 ? Math.round((library.losslessTracks / library.tracks) * 100) : 0;
  const totalHours = Math.floor(library.totalDurationMs / 3_600_000);
  const playtimeHours = Math.floor(listening.totalPlaytimeSeconds / 3600);

  return (
    <div className="melodee-screen">
      <PageHeader
        title={t('stats.title')}
        subtitle={t('stats.subtitle')}
        actions={
          <>
            <span className="melodee-page-header__meta">
              {t('stats.updated', { when: formatRelativeTime(stats.generatedAt) })}
            </span>
            <Button
              kind="ghost"
              size="sm"
              renderIcon={Renew}
              onClick={() => {
                setLoading(true);
                void load({ cancelled: false });
              }}
              disabled={loading}
            >
              {t('actions.refresh')}
            </Button>
          </>
        }
      />

      <Surface
        title={t('stats.library.title')}
        subtitle={t('stats.library.subtitle')}
        headingId="stats-library"
      >
        <div className="melodee-metric-grid">
          <MetricCard
            label={t('stats.library.releases')}
            value={formatNumber(library.releases)}
            hint={t('stats.library.artistsHint', { count: formatNumber(library.artists) })}
          />
          <MetricCard
            label={t('stats.library.tracks')}
            value={formatNumber(library.tracks)}
            hint={t('stats.library.assetsHint', { count: formatNumber(library.mediaAssets) })}
          />
          <MetricCard
            label={t('stats.library.size')}
            value={formatBytes(library.totalBytes)}
            hint={t('stats.library.averageSize', {
              size: formatBytes(library.tracks > 0 ? library.totalBytes / library.tracks : 0),
            })}
          />
          <MetricCard
            label={t('stats.library.duration')}
            value={t('stats.hoursShort', { hours: formatNumber(totalHours) })}
            hint={t('stats.library.averageTrack', {
              duration: formatDuration(library.averageTrackDurationMs),
            })}
          />
          <MetricCard
            label={t('stats.library.lossless')}
            value={`${losslessShare}%`}
            hint={t('stats.library.losslessHint', {
              count: formatNumber(library.losslessTracks),
            })}
          />
          <MetricCard
            label={t('stats.library.formats')}
            value={formatNumber(library.formats.length)}
            hint={t('stats.library.formatsHint')}
          />
        </div>

        <div className="melodee-stats-panels">
          <div className="melodee-stats-panel">
            <h3 className="melodee-stats-panel__title">{t('stats.library.byFormat')}</h3>
            <BarList
              ariaLabel={t('stats.library.byFormat')}
              items={library.formats.map((format) => ({
                key: `${format.container}-${format.codec}`,
                label: `${format.container.toUpperCase()} · ${format.codec}`,
                value: format.bytes,
                valueText: formatBytes(format.bytes),
                detail: t('stats.library.trackCount', { count: formatNumber(format.tracks) }),
              }))}
            />
            {library.formats.length === 0 ? (
              <p className="melodee-stats-empty">{t('stats.library.noMedia')}</p>
            ) : null}
          </div>
          <div className="melodee-stats-panel">
            <h3 className="melodee-stats-panel__title">{t('stats.library.byDecade')}</h3>
            <DecadeBars
              ariaLabel={t('stats.library.byDecade')}
              series={library.decades}
              labelFor={(decade) => `${decade}s`}
            />
          </div>
        </div>
      </Surface>

      <Surface
        title={t('stats.listening.title')}
        subtitle={t('stats.listening.subtitle')}
        headingId="stats-listening"
      >
        <div className="melodee-metric-grid">
          <MetricCard
            label={t('stats.listening.plays')}
            value={formatNumber(listening.totalPlays)}
            hint={t('stats.listening.playsHint')}
          />
          <MetricCard
            label={t('stats.listening.playtime')}
            value={t('stats.hoursShort', { hours: formatNumber(playtimeHours) })}
            hint={t('stats.listening.playtimeHint', {
              minutes: formatNumber(Math.floor(listening.totalPlaytimeSeconds / 60)),
            })}
          />
          <MetricCard
            label={t('stats.listening.uniqueTracks')}
            value={formatNumber(listening.uniqueTracks)}
            hint={t('stats.listening.uniqueTracksHint', {
              percent: formatNumber(
                library.tracks > 0
                  ? Math.round((listening.uniqueTracks / library.tracks) * 100)
                  : 0,
              ),
            })}
          />
          <MetricCard
            label={t('stats.listening.listeners')}
            value={formatNumber(listening.uniqueListeners)}
            hint={t('stats.listening.listenersHint')}
          />
        </div>
        <PlaysChart
          series={listening.playsPerDay}
          title={t('stats.listening.perDay')}
          playLabel={t('stats.listening.plays')}
        />
        <div className="melodee-stats-panels">
          <div className="melodee-stats-panel">
            <h3 className="melodee-stats-panel__title">{t('stats.listening.topArtists')}</h3>
            <BarList
              ariaLabel={t('stats.listening.topArtists')}
              items={listening.topArtists.map((artist) => ({
                key: artist.name,
                label: artist.name,
                value: artist.plays,
                valueText: t('stats.listening.playCount', {
                  count: formatNumber(artist.plays),
                }),
                detail: formatDuration(artist.seconds * 1000),
              }))}
            />
            {listening.topArtists.length === 0 ? (
              <p className="melodee-stats-empty">{t('stats.listening.noPlays')}</p>
            ) : null}
          </div>
          <div className="melodee-stats-panel">
            <h3 className="melodee-stats-panel__title">{t('stats.listening.topTracks')}</h3>
            <BarList
              ariaLabel={t('stats.listening.topTracks')}
              items={listening.topTracks.map((track) => ({
                key: track.id,
                label: track.title,
                value: track.plays,
                valueText: t('stats.listening.playCount', {
                  count: formatNumber(track.plays),
                }),
                detail: track.artist ?? undefined,
              }))}
            />
            {listening.topTracks.length === 0 ? (
              <p className="melodee-stats-empty">{t('stats.listening.noPlays')}</p>
            ) : null}
          </div>
        </div>
      </Surface>

      <Surface
        title={t('stats.database.title')}
        subtitle={t('stats.database.subtitle')}
        headingId="stats-database"
      >
        <div className="melodee-metric-grid">
          <MetricCard
            label={t('stats.database.size')}
            value={formatBytes(database.totalBytes)}
            hint={t('stats.database.sizeHint')}
          />
          <MetricCard
            label={t('stats.database.connections')}
            value={`${formatNumber(database.poolInUse)} / ${formatNumber(database.poolCapacity)}`}
            hint={t('stats.database.connectionsHint')}
          />
          <MetricCard
            label={t('stats.database.workItems')}
            value={formatNumber(database.workItems)}
            hint={t('stats.database.workItemsHint')}
          />
          <MetricCard
            label={t('stats.database.pendingProjection')}
            value={formatNumber(database.pendingProjection)}
            hint={t('stats.database.pendingProjectionHint')}
            status={database.pendingProjection > 0 ? 'warning' : undefined}
          />
          <MetricCard
            label={t('stats.database.playEvents')}
            value={formatNumber(database.playEvents)}
            hint={t('stats.database.playEventsHint')}
          />
        </div>
        <div className="melodee-stats-panel">
          <h3 className="melodee-stats-panel__title">{t('stats.database.tables')}</h3>
          <BarList
            ariaLabel={t('stats.database.tables')}
            items={database.tables.map((table) => ({
              key: table.name,
              label: table.name,
              value: table.bytes,
              valueText: formatBytes(table.bytes),
            }))}
          />
        </div>
      </Surface>

      <Surface
        title={t('stats.library.largestReleases')}
        subtitle={t('stats.library.largestReleasesSubtitle')}
        headingId="stats-largest-releases"
        variant="flush"
      >
        <table className="melodee-mini-table cds--data-table" aria-label={t('stats.library.largestReleases')}>
          <thead>
            <tr>
              <th scope="col">{t('stats.library.release')}</th>
              <th scope="col">{t('stats.library.artist')}</th>
              <th scope="col">{t('stats.library.tracksHeader')}</th>
              <th scope="col">{t('stats.library.sizeHeader')}</th>
            </tr>
          </thead>
          <tbody>
            {library.largestReleases.map((release) => (
              <tr key={release.id}>
                <td>{release.title}</td>
                <td>{release.artist ?? t('common.unknown')}</td>
                <td>{formatNumber(release.tracks)}</td>
                <td>{formatBytes(release.bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </Surface>
    </div>
  );
}
