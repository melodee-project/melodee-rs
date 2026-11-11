import { formatNumber } from '../i18n/format';

/**
 * Stage progress (UI-SPEC §5, ADR-0184): a deterministic position in the
 * Release lifecycle, never an elapsed-time estimate. The numeric value is
 * exposed as text so the bar is never color-only, and `stage` supplies the
 * meaningful label behind the percentage.
 */
export function ProgressBar({
  value,
  label,
  stage,
}: {
  /** 0–100. */
  value: number;
  /** Accessible name for the bar (e.g. the Release title). */
  label: string;
  /** Human-readable stage behind the number (e.g. "Validating"). */
  stage?: string;
}) {
  const clamped = Math.max(0, Math.min(100, Math.round(value)));
  const valueText = stage
    ? `${stage} · ${formatNumber(clamped)}%`
    : `${formatNumber(clamped)}%`;
  return (
    <div className="melodee-progress">
      <div
        className="melodee-progress__track"
        role="progressbar"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={clamped}
        aria-valuetext={valueText}
      >
        <div className="melodee-progress__bar" style={{ inlineSize: `${clamped}%` }} />
      </div>
      <span className="melodee-progress__value" aria-hidden>
        {clamped}%
      </span>
    </div>
  );
}
