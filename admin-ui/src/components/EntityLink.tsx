import { Link } from '@carbon/react';
import type { ReactNode } from 'react';
import { Link as RouterLink } from 'react-router-dom';

/**
 * Canonical links to catalog entities. Admin product language is
 * Release/Track; these are the only sanctioned way to link to them.
 */

interface EntityLinkProps {
  id: string;
  children?: ReactNode;
  name: string;
}

export function ArtistLink({ id, name, children }: EntityLinkProps) {
  return (
    <Link as={RouterLink} to={`/catalog/artists/${encodeURIComponent(id)}`}>
      {children ?? name}
    </Link>
  );
}

export function ReleaseLink({ id, name, children }: EntityLinkProps) {
  return (
    <Link as={RouterLink} to={`/catalog/releases/${encodeURIComponent(id)}`}>
      {children ?? name}
    </Link>
  );
}

export function TrackLink({ id, name, children }: EntityLinkProps) {
  return (
    <Link as={RouterLink} to={`/catalog/tracks/${encodeURIComponent(id)}`}>
      {children ?? name}
    </Link>
  );
}
