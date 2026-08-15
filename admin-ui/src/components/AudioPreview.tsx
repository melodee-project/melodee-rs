import { useTranslation } from 'react-i18next';

/**
 * Audio evidence preview (UI-SPEC §6): labeled transport, keyboard operable
 * (native <audio controls>), never autoplays.
 */
export function AudioPreview({
  src,
  label,
  onError,
}: {
  src: string;
  label?: string;
  /** Fires when the stream cannot be loaded (e.g. 404 preview_unavailable). */
  onError?: () => void;
}) {
  const { t } = useTranslation();
  return (
    // Native controls are the accessible transport; the label describes what
    // is being previewed, and preload="none" guarantees no autoplay.
    <audio
      controls
      preload="none"
      src={src}
      aria-label={label ?? t('audio.previewLabel')}
      onError={onError}
      style={{ inlineSize: '100%' }}
    />
  );
}
