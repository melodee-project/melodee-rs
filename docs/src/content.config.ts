import { defineCollection } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema, i18nSchema } from '@astrojs/starlight/schema';
import { glob } from 'astro/loaders';

export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
  // Starlight resolves user UI-string overrides from an `i18n` collection and
  // warns when the collection is absent. The English source locale ships an
  // empty override file so the collection exists; additional locales add their
  // own `src/content/i18n/<lang>.json` entry.
  i18n: defineCollection({
    loader: glob({ pattern: '*.json', base: './src/content/i18n' }),
    schema: i18nSchema(),
  }),
};
