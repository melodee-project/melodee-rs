import { Heading } from '@carbon/react';
import { useTranslation } from 'react-i18next';

import type { SubsystemHealth, SubsystemHealthStatus, SystemHealth } from '../api/types';
import { HealthIndicator, type HealthState } from './HealthIndicator';

/**
 * Aggregated subsystem health panel (SPEC §22): PostgreSQL, Tantivy search
 * index, storage, FFmpeg, podcast storage, disk free space, background
 * workers, and the mounted API surfaces. Every verdict is label + icon +
 * detail (never color-only, UI-SPEC §5); `disabled` means a deliberately
 * unconfigured optional subsystem and renders neutral, never as a failure.
 * The states (`unavailable`, `readOnly`, `misconfigured`,
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

function SubsystemRow({ name, subsystem }: { name: string; subsystem: SubsystemHealth }) {
  const { t } = useTranslation();
  return (
    <li className="melodee-health-row">
      <span className="melodee-health-row__name">{name}</span>
      <span className="melodee-health-row__status">
        <HealthIndicator
          state={STATE_BY_STATUS[subsystem.status]}
          label={t(`system.health.value.${subsystem.status}`)}
        />
        <span className="melodee-health-row__detail">{subsystem.detail}</span>
      </span>
    </li>
  );
}

export function SystemHealthPanel({
  health,
  title,
  headingId,
}: {
  health: SystemHealth;
  title: string;
  headingId?: string;
}) {
  const { t } = useTranslation();
  return (
    <section className="melodee-panel" aria-labelledby={headingId}>
      <Heading id={headingId} className="melodee-panel__title">
        {title}
      </Heading>
      <ul className="melodee-health-list">
        <SubsystemRow name={t('system.health.postgresql')} subsystem={health.postgresql} />
        <SubsystemRow name={t('system.health.tantivy')} subsystem={health.tantivy} />
        <SubsystemRow name={t('system.health.storage')} subsystem={health.storage} />
        <SubsystemRow name={t('system.health.ffmpeg')} subsystem={health.ffmpeg} />
        <SubsystemRow name={t('system.health.podcastStorage')} subsystem={health.podcastStorage} />
        <SubsystemRow name={t('system.health.disk')} subsystem={health.disk} />
        <SubsystemRow name={t('system.health.workers')} subsystem={health.workers} />
        <li className="melodee-health-row">
          <span className="melodee-health-row__name">{t('system.health.apiSurfaces')}</span>
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
    </section>
  );
}
