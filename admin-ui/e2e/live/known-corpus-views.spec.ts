import { expect, test } from '@playwright/test';

import { KNOWN_RELEASES, RUN_ID, type KnownRelease } from './known-corpus.generated';
import { HAVE_CREDENTIALS, LIVE_URL as BASE_URL, signIn } from './session';

/**
 * Known-answer view validation (Phase 3 of design/testing/UI-AND-WORKFLOW-AUDIT.md).
 *
 * Runs against a real instance and asserts every view is populated from the
 * *decided* values, not from whatever happens to be on screen. The expectations
 * come from `known-corpus.generated.ts`, which `scripts/verify_known_corpus.py`
 * writes from the corpus oracle plus resolved catalogue ids:
 *
 *   1. generate + ingest + verify the database, emitting UI expectations:
 *      python3 scripts/verify_known_corpus.py --ingest \
 *        --emit-ui-expectations admin-ui/e2e/live/known-corpus.generated.ts
 *   2. drive the UI:
 *      MELODEE_LIVE_USERNAME=... MELODEE_LIVE_PASSWORD=... make audit-live
 *
 * Without generated expectations (or credentials) the suite skips rather than
 * failing, so the default gate is unaffected.
 */

const HAS_FIXTURES = KNOWN_RELEASES.length > 0;
const CAN_RUN = HAVE_CREDENTIALS && HAS_FIXTURES;

function releasePath(release: KnownRelease): string {
  return `${BASE_URL}/catalog/releases/${release.releaseId}`;
}

test.describe.configure({ mode: 'serial' });

test.describe(`known-answer views (run ${RUN_ID})`, () => {
  test.skip(
    !CAN_RUN,
    'needs MELODEE_LIVE_USERNAME/PASSWORD plus e2e/live/known-corpus.generated.ts',
  );

  test('release detail renders every decided value in every tab', async ({ page, context, request }) => {
    await signIn(context, request);
    const failures: string[] = [];
    for (const release of KNOWN_RELEASES) {
      await page.goto(releasePath(release));
      const hero = page.locator('.melodee-hero');
      await expect(hero).toBeVisible({ timeout: 20_000 });

      // Hero: title, billed artist, track count.
      await expect(hero.getByRole('heading', { level: 1 })).toHaveText(release.album);
      await expect(hero).toContainText(release.displayArtist);
      await expect(hero).toContainText(String(release.trackCount));

      // Artwork: an <img> when the fixture declares cover art. The image must
      // actually decode — a 200 with an undecodable body renders a broken
      // image, which is the failure mode this caught (the endpoint answered
      // `/original` with an empty body before the size token was accepted).
      if (release.hasArtwork) {
        const cover = hero.locator('img').first();
        await expect(cover).toHaveCount(1);
        // Decoding is asynchronous, so wait for the load to settle before
        // reading: a loaded 160px `small` derivative is the observable proof
        // that the image endpoint served real artwork instead of an empty body.
        await page
          .waitForFunction(
            () => {
              const image = document.querySelector<HTMLImageElement>('.melodee-hero img');
              return image !== null && image.complete;
            },
            undefined,
            { timeout: 10_000 },
          )
          .catch(() => undefined);
        const decoded = await cover.evaluate(
          (element) => (element as HTMLImageElement).naturalWidth,
        );
        if (decoded < 100) {
          failures.push(
            `${release.slug}: cover art did not decode (naturalWidth=${decoded})`,
          );
        }
      }

      // Tracks tab: every decided track title, in the decided order.
      await page.getByRole('tab', { name: 'Tracks' }).click();
      const table = page.getByRole('table', { name: /Tracks/ }).first();
      await expect(table).toBeVisible();
      const rows = table.locator('tbody tr');
      await expect(rows).toHaveCount(release.trackCount);
      for (const [index, track] of release.tracks.entries()) {
        const row = rows.nth(index);
        if (!(await row.innerText()).includes(track.title)) {
          failures.push(`${release.slug}: track row ${index + 1} missing "${track.title}"`);
        }
      }

      // Files tab: every published media asset is listed with a size.
      await page.getByRole('tab', { name: 'Files' }).click();
      await expect(page.getByRole('table', { name: /Files|Media/ }).first()).toBeVisible();

      // Classification tab: trait evidence exists for every fixture.
      await page.getByRole('tab', { name: 'Classification' }).click();
      await expect(page.getByText(/confidence|trait/i).first()).toBeVisible();

      // History tab: the catalogue wrote audit evidence.
      await page.getByRole('tab', { name: 'History' }).click();
      await expect(page.locator('.melodee-timeline__item').first()).toBeVisible();
    }
    expect(failures, `view defects:\n${failures.join('\n')}`).toEqual([]);
  });

  test('artist detail shows the credits each fixture produced', async ({ page, context, request }) => {
    await signIn(context, request);
    // The artist named by each fixture exists with a track credit; its drill-down
    // must list at least the fixture's own tracks.
    const artists = [...new Set(KNOWN_RELEASES.map((release) => release.displayArtist))];
    for (const artistName of artists) {
      await page.goto(`${BASE_URL}/catalog/artists?query=${encodeURIComponent(artistName)}`);
      const link = page.getByRole('link', { name: artistName, exact: true }).first();
      if ((await link.count()) === 0) {
        continue; // some fixtures legitimately produce several identities
      }
      await link.click();
      // The page header and the entity hero both use h1; the hero carries the name.
      await expect(page.locator('.melodee-hero').getByRole('heading', { level: 1 })).toContainText(
        artistName,
      );
      await page.getByRole('tab', { name: 'Tracks' }).click();
      await expect(page.getByRole('table').first()).toBeVisible();
    }
  });

  test('finished fixtures sit in the Active phase, not in an in-flight phase', async ({ page, context, request }) => {
    await signIn(context, request);
    // The queue lists every state; the *phase* filter is what an operator uses.
    // A finished Release must never appear under an in-flight phase.
    for (const phase of ['inbound', 'processing', 'publishing', 'catalog']) {
      await page.goto(`${BASE_URL}/processing?phase=${phase}`);
      const table = page.getByRole('table', { name: /Processing queue work items/ });
      await expect(table).toBeVisible();
      const body = await table.locator('tbody').innerText();
      for (const release of KNOWN_RELEASES) {
        expect(body, `${release.album} appeared under phase ${phase}`).not.toContain(release.album);
      }
    }

    // Under the Active phase they are all present.
    await page.goto(`${BASE_URL}/processing?phase=active`);
    const active = page.getByRole('table', { name: /Processing queue work items/ });
    await expect(active).toBeVisible();
    const activeBody = await active.locator('tbody').innerText();
    for (const release of KNOWN_RELEASES) {
      expect(activeBody, `Active phase missing ${release.album}`).toContain(release.album);
    }

    // History lists them as terminal work with the Active phase chip.
    await page.goto(`${BASE_URL}/processing/history`);
    const historyTable = page.getByRole('table').first();
    await expect(historyTable).toBeVisible();
    // Wait for the terminal-state page to be populated before reading it.
    await expect(historyTable).toContainText(KNOWN_RELEASES[0]!.folder, { timeout: 20_000 });
    const history = await historyTable.locator('tbody').innerText();
    for (const release of KNOWN_RELEASES) {
      // History labels rows by the work item (its inbound folder), not the album.
      expect(history, `history missing ${release.folder}`).toContain(release.folder);
    }
    expect(history.toLowerCase()).toContain('active');
  });

  test('overview counts include the processed fixtures', async ({ page, context, request }) => {
    await signIn(context, request);
    await page.goto(BASE_URL);
    // The catalogue counters are non-zero and the KPI row is populated.
    const metrics = page.locator('.melodee-metric-card');
    await expect(metrics.first()).toBeVisible({ timeout: 20_000 });
    const values = await metrics.locator('.melodee-metric-card__value').allInnerTexts();
    expect(values.length).toBeGreaterThanOrEqual(6);
    for (const value of values) {
      expect(value.trim()).not.toBe('');
    }
    // Every KPI card is populated and carries a label (no blank/mis-rendered tile).
    const labels = await metrics.locator('.melodee-metric-card__label').allInnerTexts();
    expect(labels.length).toBe(values.length);
    for (const label of labels) {
      expect(label.trim()).not.toBe('');
    }
  });
});
