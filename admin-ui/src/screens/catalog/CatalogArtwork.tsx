import { useState } from 'react';

import { catalogImageUrl } from '../../api/admin';
import { EntityAvatar } from '../../components/EntityAvatar';

/**
 * Catalog artwork via the frozen native `/images/{id}/{size}` endpoint
 * (same-origin, cookie-authenticated). Absent artwork classes come back as
 * an undecodable empty 200 body, so a failed image load is the normal
 * "no artwork" outcome — it falls back to the EntityAvatar initials rather
 * than leaving a broken image (PRD-UI-REF-002 absent-artwork classes).
 */
export function CatalogArtwork({
  imageId,
  name,
  size = 40,
  imageSize = 'small',
  decorative = false,
  cacheKey,
}: {
  imageId: string;
  name: string;
  /** Pixel box of the avatar. */
  size?: number;
  /** Named size on the native image endpoint. */
  imageSize?: 'thumbnail' | 'small' | 'medium' | 'large';
  decorative?: boolean;
  /**
   * Bumps the query string so a just-mutated override is re-fetched instead
   * of reusing the cached predecessor (the endpoint ignores the query).
   */
  cacheKey?: number;
}) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return <EntityAvatar name={name} size={size} decorative={decorative} />;
  }
  const base = catalogImageUrl(imageId, imageSize);
  return (
    <span
      className="melodee-entity-avatar"
      style={{ inlineSize: size, blockSize: size, fontSize: size / 2 }}
    >
      <img
        src={cacheKey === undefined ? base : `${base}?v=${cacheKey}`}
        alt={decorative ? '' : name}
        // The fixed-size fallback must settle before a mobile user reaches the
        // adjacent upload controls; lazy loading otherwise shifts the tab
        // panel while the browser is scrolling to the button.
        loading="eager"
        onError={() => setFailed(true)}
      />
    </span>
  );
}
