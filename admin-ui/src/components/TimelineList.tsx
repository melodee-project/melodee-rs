import type { ReactNode } from 'react';

/**
 * Ordered event list (UI-SPEC §6.1, ADR-0184) for review timelines and decision
 * feeds. Unordered `<ul>` because the sequence is conveyed by the rendered
 * times; the marker tone is decorative and never the only signal.
 */
export type TimelineTone = 'default' | 'success' | 'info' | 'warning' | 'error';

export interface TimelineEvent {
  id: string;
  title: ReactNode;
  detail?: ReactNode;
  time: ReactNode;
  tone?: TimelineTone;
}

export function TimelineList({ events }: { events: TimelineEvent[] }) {
  return (
    <ul className="melodee-timeline">
      {events.map((event) => (
        <li className="melodee-timeline__item" key={event.id}>
          <span
            aria-hidden
            className={`melodee-timeline__marker${
              event.tone && event.tone !== 'default' ? ` melodee-timeline__marker--${event.tone}` : ''
            }`}
          />
          <span className="melodee-timeline__content">
            <span>
              <span className="melodee-timeline__title">{event.title}</span>
              {event.detail ? <span className="melodee-timeline__detail">{event.detail}</span> : null}
            </span>
            <span className="melodee-timeline__time">{event.time}</span>
          </span>
        </li>
      ))}
    </ul>
  );
}
