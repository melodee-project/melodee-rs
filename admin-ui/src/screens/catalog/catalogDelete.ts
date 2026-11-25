import type {
  ArtistDeletePreview,
  CatalogDeleteImpact,
  ReleaseDeletePreview,
  TrackDeletePreview,
} from '../../api/types';

/** Normalized hard-deletion preview shown by the shared confirmation dialog. */
export interface CatalogDeletePreview {
  impact: CatalogDeleteImpact;
  storageConfigured: boolean;
  warnings: string[];
}

/** Normalizes the Artist hard-deletion preview for the shared dialog. */
export function artistDeletePreviewView(preview: ArtistDeletePreview): CatalogDeletePreview {
  return {
    impact: preview.impact,
    storageConfigured: preview.storageConfigured,
    warnings: preview.warnings,
  };
}

/** Normalizes the Release hard-deletion preview for the shared dialog. */
export function releaseDeletePreviewView(preview: ReleaseDeletePreview): CatalogDeletePreview {
  return {
    impact: preview.impact,
    storageConfigured: preview.storageConfigured,
    warnings: preview.warnings,
  };
}

/** Normalizes the Track hard-deletion preview for the shared dialog. */
export function trackDeletePreviewView(preview: TrackDeletePreview): CatalogDeletePreview {
  return {
    impact: preview.impact,
    storageConfigured: preview.storageConfigured,
    warnings: preview.warnings,
  };
}
