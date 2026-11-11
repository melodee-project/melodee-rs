import { useTranslation } from 'react-i18next';

import type { SubsystemHealth, SubsystemHealthStatus, SystemHealth } from '../api/types';
import { HealthIndicator, HealthDot, type HealthState, type HealthTone } from './HealthIndicator';
import { Surface } from './Surface';

/**
 * Aggregated subsystem health panel (SPEC §22, UI-SPEC §6.1): PostgreSQL,
 * Tantivy search index, storage, FFmpeg, podcast storage, disk free space,
 * background workers, and the mounted API surfaces. Every verdict is dot +
 * label + detail (never color-only, UI-SPEC §5); `disabled` means a
 * deliberately unconfigured optional subsystem and renders neutral, never as a
 * failure. The states (`unavailable`, `readOnly`, `misconfigured`,
 * `capacityExhausted`) distinguish failure modes for operators.
 */

const STATE_BY_STATUS: Record<SubsystemHealthStatus, HealthState> = {
  ok: 'online',
  degraded: 'degraded',
  disabled: 'disabled',
  unavailable: 'offline',
  readOnly: 'degraded',
  misconfigured: 'degraded',
  capacityExhausted: 'degraded',
};

const TONE_BY_STATUS: Record<SubsystemHealthStatus, HealthTone> = {
  ok: 'success',
  degraded: 'warning',
  disabled: 'neutral',
  unavailable: 'error',
  readOnly: 'warning',
  misconfigured: 'warning',
  capacityExhausted: 'warning',
};

function SubsystemRow({ name, subsystem }: { name: string; subsystem: SubsystemHealth }) {
  const { t } = useTranslation();
  const state = STATE_BY_STATUS[subsystem.status];
  const tone = TONE_BY_STATUS[subsystem.status];
  return (
    <li className="melodee-health-row">
      <div className="melodee-health-row__main">
        <span className="melodee-health-row__name">
          <HealthDot
            state={state}
            tone={tone}
            label={name}
            className="melodee-health-row__dot-label"
          />
        </span>
        <span className="melodee-health-row__detail">{subsystem.detail}</span>
      </div>
      <span
        className={`melodee-health-row__verdict${
          tone === 'success' ? '' : ` melodee-health-row__verdict--${tone}`
        }`}
      >
        {t(`system.health.value.${subsystem.status}`)}
      </span>
    </li>
  );
}

export function SystemHealthPanel({
  health,
  title,
  headingId,
  headingLevel = 2,
}: {
  health: SystemHealth;
  title: string;
  headingId?: string;
  headingLevel?: 2 | 3;
}) {
  const { t } = useTranslation();
  return (
    <Surface title={title} headingId={headingId} headingLevel={headingLevel}>
      <ul className="melodee-health-list">
        <SubsystemRow name={t('system.health.postgresql')} subsystem={health.postgresql} />
        <SubsystemRow name={t('system.health.tantivy')} subsystem={health.tantivy} />
        <SubsystemRow name={t('system.health.storage')} subsystem={health.storage} />
        <SubsystemRow name={t('system.health.ffmpeg')} subsystem={health.ffmpeg} />
        <SubsystemRow name={t('system.health.podcastStorage')} subsystem={health.podcastStorage} />
        <SubsystemRow name={t('system.health.disk')} subsystem={health.disk} />
        <SubsystemRow name={t('system.health.workers')} subsystem={health.workers} />
        <li className="melodee-health-row melodee-health-row--stacked">
          <div className="melodee-health-row__main">
            <span className="melodee-health-row__name">{t('system.health.apiSurfaces')}</span>
          </div>
          <span className="melodee-health-row__status">
            {health.apiSurfaces.map((surface) => (
              <HealthIndicator
                key={surface.surface}
                state={surface.status === 'serving' ? 'online' : 'degraded'}
                label={t(`system.health.surface.${surface.surface}`, {
                  defaultValue: surface.surface,
                })}
              />
            ))}
          </span>
        </li>
      </ul>
    </Surface>
  );
}
