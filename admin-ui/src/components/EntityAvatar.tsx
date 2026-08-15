/**
 * Artwork/identity avatar. Artwork carries meaningful alt text by default;
 * pass `decorative` when the adjacent label already names the entity
 * (UI-SPEC §9). The fallback initial sits on the contrasting artwork
 * surface token, never a raw color.
 */
export function EntityAvatar({
  name,
  imageUrl,
  size = 40,
  decorative = false,
}: {
  name: string;
  imageUrl?: string | null;
  size?: number;
  decorative?: boolean;
}) {
  const initial = [...name.trim()][0] ?? '?';
  return (
    <span
      className="melodee-entity-avatar"
      style={{ inlineSize: size, blockSize: size, fontSize: size / 2 }}
    >
      {imageUrl ? (
        <img src={imageUrl} alt={decorative ? '' : name} loading="lazy" />
      ) : (
        <span role={decorative ? undefined : 'img'} aria-label={decorative ? undefined : name}>
          {initial}
        </span>
      )}
    </span>
  );
}
