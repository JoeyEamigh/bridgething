import { describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL, type MergedApps } from '@bridgething/catalog';
import { mergedApps, storeData } from './apps.ts';
import { fakeD1, type FakeD1 } from './d1-fake.ts';
import { admitAll, app, APP_ID, catalog, DEVICE, NOW, seedOfficial, seedSource, seedUnreadable } from './fixtures.ts';
import { recordInstalled } from './installs.ts';

const ATTESTED = 'https://attested.example/catalog.json';
const LISTED = 'https://listed.example/catalog.json';
const ZEBRA = 'https://zebra.example/catalog.json';
const QUARANTINED = 'https://unreviewed.example/catalog.json';
const REJECTED = 'https://rejected.example/catalog.json';

async function merged(db: FakeD1): Promise<MergedApps> {
  return JSON.parse(await mergedApps({ db, now: NOW })) as MergedApps;
}

async function seeded(): Promise<FakeD1> {
  const db = fakeD1();
  await seedOfficial(db);
  await seedSource(db, ZEBRA, 'listed', catalog('zebra'));
  await seedSource(db, LISTED, 'listed', catalog('alpha'));
  await seedSource(db, ATTESTED, 'attested', catalog('mid'));
  await seedSource(db, QUARANTINED, 'quarantined');
  await seedSource(db, REJECTED, 'rejected');
  return db;
}

describe('mergedApps', () => {
  test('orders official, then attested, then listed by name, and leaves the rest out', async () => {
    const apps = await merged(await seeded());

    expect(apps.catalogs.map(entry => [entry.url, entry.official, entry.attested])).toEqual([
      [OFFICIAL_CATALOG_URL, true, true],
      [ATTESTED, false, true],
      [LISTED, false, false],
      [ZEBRA, false, false],
    ]);
  });

  test('serves the stored catalogs without touching the network', async () => {
    const original = globalThis.fetch;
    let fetched = 0;
    globalThis.fetch = (async () => {
      fetched += 1;
      return new Response('{}');
    }) as unknown as typeof fetch;
    try {
      await merged(await seeded());
    } finally {
      globalThis.fetch = original;
    }

    expect(fetched).toBe(0);
  });

  test('the stored catalog comes back as the document the source published', async () => {
    const db = fakeD1();
    const published = catalog('alpha', [app(APP_ID, 'thing', { icon: 'https://listed.example/<icon>.svg' })]);
    await seedOfficial(db);
    await seedSource(db, LISTED, 'listed', published);

    expect((await merged(db)).catalogs[1]!.catalog).toEqual(published);
  });

  test('an official catalog the directory has not fetched yet is reported, not silently missing', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed');

    const apps = await merged(db);
    expect(apps.catalogs.map(entry => entry.url)).toEqual([LISTED]);
    expect(apps.failures.map(failure => failure.url)).toEqual([OFFICIAL_CATALOG_URL]);
  });

  test('a published source with no readable catalog is a failure and the rest still merge', async () => {
    const db = await seeded();
    await seedUnreadable(db, 'https://broken.example/catalog.json', 'listed', 'not a valid catalog.v1: nope');

    const apps = await merged(db);
    expect(apps.catalogs).toHaveLength(4);
    expect(apps.failures).toEqual([
      { url: 'https://broken.example/catalog.json', reason: 'not a valid catalog.v1: nope' },
    ]);
  });

  test('install counts ride along, so a client can sort by popularity from one request', async () => {
    const db = await seeded();
    await recordInstalled({
      db,
      body: { device_id: DEVICE, apps: [{ app_id: APP_ID, source_url: LISTED, version: '1.0.0' }] },
      now: NOW,
      admit: admitAll,
    });

    expect((await merged(db)).installs).toEqual([
      { app_id: APP_ID, source_url: LISTED, count: 1, versions: { '1.0.0': 1 } },
    ]);
  });

  test('a directory nobody has installed from reports no counts rather than omitting the field', async () => {
    const apps = await merged(await seeded());

    expect(apps.installs).toEqual([]);
    expect(apps.failures).toEqual([]);
  });

  test('reads everything in one round trip', async () => {
    const db = await seeded();
    db.statements.length = 0;
    await merged(db);

    expect(db.statements).toHaveLength(4);
  });
});

describe('storeData', () => {
  test('carries the merged apps and the site directory together', async () => {
    const data = JSON.parse(await storeData({ db: await seeded(), now: NOW })) as {
      apps: MergedApps;
      directory: { url: string; note?: unknown }[];
    };

    expect(data.apps.catalogs).toHaveLength(4);
    expect(data.directory.map(entry => entry.url)).toEqual([ATTESTED, LISTED, QUARANTINED, ZEBRA]);
    expect(data.directory.every(entry => !('note' in entry))).toBe(true);
  });
});
