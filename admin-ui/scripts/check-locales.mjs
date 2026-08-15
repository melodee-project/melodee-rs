#!/usr/bin/env node
/**
 * Locale structural validation (PRD-I18N-006, ADR-0137).
 *
 * Fails (exit 1) on structural errors:
 *   - unparseable locale JSON
 *   - keys present in a locale but absent from the en-US source catalog
 *   - empty-string values in any locale (empty means "fall back", so omit instead)
 *
 * Reports but does NOT fail on untranslated keys — admin-console vocabulary
 * legitimately falls back to English until translated.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const localesDir = join(dirname(fileURLToPath(import.meta.url)), '../src/i18n/locales');
const SOURCE = 'en-US';

const files = readdirSync(localesDir)
  .filter((name) => name.endsWith('.json'))
  .sort();

let errors = 0;
const catalogs = new Map();
for (const file of files) {
  const culture = file.replace(/\.json$/, '');
  try {
    catalogs.set(culture, JSON.parse(readFileSync(join(localesDir, file), 'utf8')));
  } catch (error) {
    console.error(`error: ${file} does not parse: ${error.message}`);
    errors += 1;
  }
}

const source = catalogs.get(SOURCE);
if (!source) {
  console.error(`error: source catalog ${SOURCE}.json is missing`);
  process.exit(1);
}
const sourceKeys = new Set(Object.keys(source));

for (const [culture, catalog] of catalogs) {
  for (const [key, value] of Object.entries(catalog)) {
    if (!sourceKeys.has(key)) {
      console.error(`error: ${culture}.json has key absent from ${SOURCE}.json: ${key}`);
      errors += 1;
    }
    if (typeof value !== 'string' || value.trim() === '') {
      console.error(`error: ${culture}.json has an empty value for key: ${key}`);
      errors += 1;
    }
  }
}

console.log('');
console.log('locale coverage (translated / total source keys):');
for (const [culture, catalog] of [...catalogs.entries()].sort()) {
  const translated = Object.keys(catalog).length;
  const pct = ((100 * translated) / sourceKeys.size).toFixed(1);
  const marker = culture === SOURCE ? ' (source)' : '';
  console.log(`  ${culture}: ${translated}/${sourceKeys.size} (${pct}%)${marker}`);
}

if (errors > 0) {
  console.error(`\n${errors} structural error(s) found`);
  process.exit(1);
}
console.log('\nlocale structure OK');
