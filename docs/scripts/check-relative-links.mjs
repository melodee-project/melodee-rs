#!/usr/bin/env node
/*
 * Validates relative internal links and image references in docs content.
 *
 * starlight-links-validator intentionally ignores relative links when
 * `errorOnRelativeLinks` is false; this script closes that gap so broken
 * relative links and missing images fail CI. It checks that every relative
 * Markdown link resolves to an existing content page (`.md`/`.mdx` file or a
 * directory index) and every relative image resolves to a file under
 * `src/content` or `public/`.
 *
 * External URLs, absolute links (validated by starlight-links-validator at
 * build time), and pure anchor links are out of scope.
 */
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const docsRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const contentRoot = join(docsRoot, 'src', 'content', 'docs');
const publicRoot = join(docsRoot, 'public');

const LINK_RE = /!?\[[^\]]*\]\(([^)\s]+)[^)]*\)/g;

/** Lists content files recursively. */
function* walk(dir) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      yield* walk(path);
    } else if (/\.(md|mdx)$/.test(entry)) {
      yield path;
    }
  }
}

/**
 * A link target resolves if it names a page file, a page route (Starlight
 * serves `page.md` at `page/`), or a directory with an index page.
 */
function pageExists(target) {
  if (/\.(md|mdx)$/.test(target)) {
    return existsSync(target);
  }
  const withoutSlash = target.replace(/\/$/, '');
  return (
    ['.md', '.mdx'].some((ext) => existsSync(withoutSlash + ext)) ||
    ['index.md', 'index.mdx'].some((index) => existsSync(join(withoutSlash, index)))
  );
}

const errors = [];

for (const file of walk(contentRoot)) {
  const source = readFileSync(file, 'utf8');
  const relative = file.slice(contentRoot.length + 1);
  let match;
  while ((match = LINK_RE.exec(source)) !== null) {
    const raw = match[1];
    if (/^(https?:|mailto:|#|\/)/.test(raw)) {
      continue;
    }
    const isImage = match[0].startsWith('!');
    const target = raw.split('#')[0];
    if (target === '') {
      continue;
    }
    const resolved = resolve(dirname(file), target);
    if (isImage || extname(target) !== '') {
      // Assets may live next to the content file or under public/.
      const publicCandidate = resolve(publicRoot, target.replace(/^\.\//, ''));
      if (!existsSync(resolved) && !existsSync(publicCandidate)) {
        errors.push(`${relative}: missing asset "${raw}"`);
      }
    } else if (!pageExists(resolved)) {
      errors.push(`${relative}: broken link "${raw}"`);
    }
  }
}

if (errors.length > 0) {
  console.error(`Found ${errors.length} invalid relative link(s):`);
  for (const error of errors) {
    console.error(`  ${error}`);
  }
  process.exit(1);
}

console.log('All relative content links and assets are valid.');
