import { describe, expect, test } from 'bun:test';
import { isCategory, MAX_CATEGORIES, OFFICIAL_CATALOG_URL, type MergedApps } from '@bridgething/catalog';
import categoryBackfill from './category-backfill.json' with { type: 'json' };
import { mergedApps } from './apps.ts';
import { fakeD1, type FakeD1 } from './d1-fake.ts';
import { admitAll, APP_ID, DEVICE, NOW, OTHER_APP_ID, seedOfficial, seedSource, serialOf } from './fixtures.ts';
import { recordInstalled } from './installs.ts';
import { featuredPin, pinFeatured, unpinFeatured } from './pins.ts';

const LISTED = 'https://listed.example/catalog.json';
const QUARANTINED = 'https://unreviewed.example/catalog.json';

const DAY = 24 * 60 * 60 * 1000;

function at(offsetDays: number): string {
  return new Date(Date.parse(NOW) + offsetDays * DAY).toISOString();
}

async function report(db: FakeD1, device: string, apps: { app_id: string; version: string }[], now: string) {
  await recordInstalled({
    db,
    body: { device_id: device, apps: apps.map(app => ({ ...app, source_url: OFFICIAL_CATALOG_URL })) },
    now,
    admit: admitAll,
  });
}

async function merged(db: FakeD1, now: string): Promise<MergedApps> {
  return JSON.parse(await mergedApps({ db, now })) as MergedApps;
}

describe('trending', () => {
  test('counts devices that picked an app up inside the last week', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await report(db, serialOf(1), [{ app_id: APP_ID, version: '1.0.0' }], at(-10));
    await report(db, serialOf(2), [{ app_id: APP_ID, version: '1.0.0' }], at(-2));
    await report(db, serialOf(3), [{ app_id: OTHER_APP_ID, version: '1.0.0' }], at(-1));

    const apps = await merged(db, NOW);
    expect(apps.trending).toEqual([
      { app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL, count: 1 },
      { app_id: OTHER_APP_ID, source_url: OFFICIAL_CATALOG_URL, count: 1 },
    ]);
    expect(apps.installs.find(entry => entry.app_id === APP_ID)?.count).toBe(2);
  });

  test('an update keeps the date the device first installed the app', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await report(db, DEVICE, [{ app_id: APP_ID, version: '1.0.0' }], at(-30));
    await report(db, DEVICE, [{ app_id: APP_ID, version: '1.1.0' }], at(-1));

    expect((await merged(db, NOW)).trending).toEqual([]);
  });

  test('a reinstall after removing the app counts again', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await report(db, DEVICE, [{ app_id: APP_ID, version: '1.0.0' }], at(-30));
    await report(db, DEVICE, [], at(-3));
    await report(db, DEVICE, [{ app_id: APP_ID, version: '1.0.0' }], at(-1));

    expect((await merged(db, NOW)).trending).toEqual([{ app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL, count: 1 }]);
  });
});

describe('featured pin', () => {
  test('nothing is pinned until an admin pins something', async () => {
    const db = fakeD1();
    await seedOfficial(db);

    expect((await merged(db, NOW)).featured).toBeNull();
    expect(await featuredPin(db)).toBeNull();
  });

  test('a pin rides apps.json and replaces any earlier pin', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await seedSource(db, LISTED, 'listed');

    expect(await pinFeatured({ db, body: { app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL }, now: NOW })).toEqual({
      ok: true,
      pin: { app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL },
    });
    await pinFeatured({ db, body: { app_id: APP_ID.toUpperCase(), source_url: LISTED }, now: NOW });

    expect((await merged(db, NOW)).featured).toEqual({ app_id: APP_ID, source_url: LISTED });
  });

  test('unpinning hands the slot back to the rotation', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await pinFeatured({ db, body: { app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL }, now: NOW });
    await unpinFeatured(db);

    expect((await merged(db, NOW)).featured).toBeNull();
  });

  test('refuses an app no published source lists', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await seedSource(db, QUARANTINED, 'quarantined');

    const unlisted = await pinFeatured({
      db,
      body: { app_id: OTHER_APP_ID, source_url: OFFICIAL_CATALOG_URL },
      now: NOW,
    });
    const quarantined = await pinFeatured({ db, body: { app_id: APP_ID, source_url: QUARANTINED }, now: NOW });

    expect(unlisted).toMatchObject({ ok: false, status: 404 });
    expect(quarantined).toMatchObject({ ok: false, status: 404 });
  });

  test('refuses a malformed body', async () => {
    const db = fakeD1();

    expect(await pinFeatured({ db, body: { app_id: 'nope', source_url: LISTED }, now: NOW })).toMatchObject({
      ok: false,
      status: 400,
    });
    expect(await pinFeatured({ db, body: null, now: NOW })).toMatchObject({ ok: false, status: 400 });
  });
});

describe('category backfill', () => {
  test('every entry is an app id with one or two known categories', () => {
    for (const [appId, categories] of Object.entries(categoryBackfill)) {
      expect(appId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
      expect(categories.length).toBeGreaterThanOrEqual(1);
      expect(categories.length).toBeLessThanOrEqual(MAX_CATEGORIES);
      for (const category of categories) expect(isCategory(category)).toBe(true);
    }
  });

  test('rides apps.json', async () => {
    const db = fakeD1();
    await seedOfficial(db);

    expect((await merged(db, NOW)).category_backfill).toEqual(categoryBackfill);
  });
});
