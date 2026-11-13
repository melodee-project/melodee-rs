import i18n from './index';

/**
 * Standards-based Intl formatting driven by the active locale (PRD-I18N-004,
 * ADR-0137). Components call these helpers instead of constructing Intl
 * formatters inline so locale changes are honored uniformly.
 */

function activeLocale(): string {
  return i18n.resolvedLanguage ?? i18n.language ?? 'en-US';
}

/** Formats an ISO timestamp or epoch as a locale date+time string. */
export function formatDateTime(value: string | number | Date): string {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return new Intl.DateTimeFormat(activeLocale(), {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(date);
}

/** Formats an ISO timestamp's clock time (chart axis ticks). */
export function formatTime(value: string | number | Date): string {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return new Intl.DateTimeFormat(activeLocale(), { timeStyle: 'short' }).format(date);
}

/** Formats a date without a time component. */
export function formatDate(value: string | number | Date): string {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return new Intl.DateTimeFormat(activeLocale(), { dateStyle: 'medium' }).format(date);
}

/**
 * Relative time for recent timestamps ("5 minutes ago"), falling back to a
 * locale date once the timestamp is more than a day old.
 */
export function formatRelativeTime(value: string | number | Date): string {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  const diffMs = date.getTime() - Date.now();
  const absoluteMinutes = Math.abs(diffMs) / 60_000;
  if (absoluteMinutes >= 60 * 24) {
    return formatDate(date);
  }
  const relative = new Intl.RelativeTimeFormat(activeLocale(), { numeric: 'auto' });
  if (absoluteMinutes < 1) {
    return relative.format(Math.round(diffMs / 1000), 'second');
  }
  if (absoluteMinutes < 60) {
    return relative.format(Math.round(diffMs / 60_000), 'minute');
  }
  return relative.format(Math.round(diffMs / 3_600_000), 'hour');
}

/** Formats a number with locale grouping. */
export function formatNumber(value: number): string {
  return new Intl.NumberFormat(activeLocale()).format(value);
}

/** Formats a byte count using locale unit conventions. */
export function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value < 0) {
    return '';
  }
  const units = ['byte', 'kilobyte', 'megabyte', 'gigabyte', 'terabyte'] as const;
  let amount = value;
  let unitIndex = 0;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  const unit = units[unitIndex] ?? 'byte';
  return new Intl.NumberFormat(activeLocale(), {
    style: 'unit',
    unit,
    unitDisplay: 'short',
    maximumFractionDigits: unitIndex === 0 ? 0 : 1,
  }).format(amount);
}

/** Formats a millisecond duration as a compact localized string. */
export function formatDuration(milliseconds: number): string {
  if (!Number.isFinite(milliseconds) || milliseconds < 0) {
    return '';
  }
  const totalSeconds = Math.round(milliseconds / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const parts: string[] = [];
  const formatter = new Intl.NumberFormat(activeLocale());
  if (hours > 0) {
    parts.push(`${formatter.format(hours)} h`);
  }
  if (minutes > 0 || hours > 0) {
    parts.push(`${formatter.format(minutes)} min`);
  }
  parts.push(`${formatter.format(seconds)} s`);
  return parts.join(' ');
}
