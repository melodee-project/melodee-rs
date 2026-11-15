import type { ArtistDeletePreview, ReleaseDeletePreview } from '../../api/types';

/** Deletion scope shared by the Artist and Release bulk actions (ADR-0199). */
export type CatalogDeleteMode = 'catalogOnly' | 'releasesAndFiles';

/** Normalized deletion preview shown by the shared confirmation dialog. */
export interface CatalogDeletePreview {
  impact: {
    releases: number;
    tracks: number;
    files: number;
    bytes: number;
    /** Releases also billed by another primary Artist: kept, never deleted. */
    retainedSharedReleases?: number;
  };
  storageReadyReleases: number;
  storageConfigured: boolean;
  warnings: string[];
}

/** Normalizes the Artist deletion preview for the shared dialog. */
export function artistDeletePreviewView(preview: ArtistDeletePreview): CatalogDeletePreview {
  return {
    impact: {
      releases: preview.impact.releases,
      tracks: preview.impact.tracks,
      files: preview.impact.mediaFiles,
      bytes: preview.impact.mediaBytes,
      retainedSharedReleases: preview.impact.retainedSharedReleases,
    },
    storageReadyReleases: preview.storageReadyReleases,
    storageConfigured: preview.storageConfigured,
    warnings: preview.warnings,
  };
}

/** Normalizes the Release deletion preview for the shared dialog. */
export function releaseDeletePreviewView(preview: ReleaseDeletePreview): CatalogDeletePreview {
  return {
    impact: {
      releases: preview.impact.releases,
      tracks: preview.impact.tracks,
      files: preview.impact.mediaFiles,
      bytes: preview.impact.mediaBytes,
    },
    storageReadyReleases: preview.storageReadyReleases,
    storageConfigured: preview.storageConfigured,
    warnings: preview.warnings,
  };
}
