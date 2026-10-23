import { describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL, type InstallCount } from '@bridgething/catalog';
import { fakeD1, type FakeD1 } from './d1-fake.ts';
import { admitAll, APP_ID, DEVICE, LATER, NOW, OTHER_APP_ID, seedSource, serialOf } from './fixtures.ts';
import { listInstallCounts, recordInstalled, type InventoryChange } from './installs.ts';

const LISTED = 'https://listed.example/catalog.json';
const QUARANTINED = 'https://unreviewed.example/catalog.json';

type Reported = { app_id: unknown; source_url: unknown; version?: unknown };

function held(appId = APP_ID, sourceUrl = OFFICIAL_CATALOG_URL, version: unknown = '1.0.0'): Reported {
  return { app_id: appId, source_url: sourceUrl, version };
}

async function report(db: FakeD1, apps: unknown[], device: unknown = DEVICE, now = NOW) {
  return recordInstalled({ db, body: { device_id: device, apps }, now, admit: admitAll });
}

async function tally(db: FakeD1): Promise<InstallCount[]> {
  return listInstallCounts(db);
}

function counted(count: number, versions: Record<string, number>, appId = APP_ID, sourceUrl = OFFICIAL_CATALOG_URL) {
  return { app_id: appId, source_url: sourceUrl, count, versions };
}

describe('recordInstalled', () => {
  test('the first device holding an app starts its tally at one', async () => {
    const db = fakeD1();
    await report(db, [held()]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 })]);
  });

  test('distinct devices holding one app accumulate', async () => {
    const db = fakeD1();
    for (let n = 0; n < 3; n += 1) await report(db, [held()], serialOf(n));

    expect(await tally(db)).toEqual([counted(3, { '1.0.0': 3 })]);
  });

  test('one device reporting the same inventory over and over never moves the tally', async () => {
    const db = fakeD1();
    for (let n = 0; n < 3; n += 1) await report(db, [held()]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 })]);
  });

  test('an unchanged inventory writes nothing', async () => {
    const db = fakeD1();
    await report(db, [held()]);
    db.statements.length = 0;

    await report(db, [held()], DEVICE, LATER);

    expect(db.statements.filter(sql => /^\s*(INSERT|UPDATE|DELETE)/.test(sql))).toEqual([]);
  });

  test('the order apps arrive in is not a change', async () => {
    const db = fakeD1();
    await report(db, [held(APP_ID), held(OTHER_APP_ID)]);
    db.statements.length = 0;

    await report(db, [held(OTHER_APP_ID), held(APP_ID)]);

    expect(db.statements.filter(sql => /^\s*(INSERT|UPDATE|DELETE)/.test(sql))).toEqual([]);
  });

  test('an app the device no longer holds leaves the tally with it', async () => {
    const db = fakeD1();
    await report(db, [held(APP_ID), held(OTHER_APP_ID)]);
    await report(db, [held(OTHER_APP_ID)]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 }, OTHER_APP_ID)]);
  });

  test('a device that gives everything up counts for nothing', async () => {
    const db = fakeD1();
    await report(db, [held()]);
    await report(db, []);

    expect(await tally(db)).toEqual([]);
  });

  test('one device dropping an app never takes another device down with it', async () => {
    const db = fakeD1();
    await report(db, [held()], serialOf(1));
    await report(db, [held()], serialOf(2));
    await report(db, [], serialOf(1));

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 })]);
  });

  test('serials differing only in case are one device', async () => {
    const db = fakeD1();
    await report(db, [held()], '8558r481q61r');
    await report(db, [held()], DEVICE);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 })]);
  });

  test('a device that is not a car thing serial is refused', async () => {
    expect(await report(fakeD1(), [held()], 'not-a-serial')).toMatchObject({ ok: false, status: 400 });
  });

  test('an inventory with no device is refused rather than counted anonymously', async () => {
    expect(await report(fakeD1(), [held()], null)).toMatchObject({ ok: false, status: 400 });
  });

  test('a body with no apps array is refused, because it is not an empty device', async () => {
    const outcome = await recordInstalled({ db: fakeD1(), body: { device_id: DEVICE }, now: NOW, admit: admitAll });

    expect(outcome).toMatchObject({ ok: false, status: 400 });
  });

  test('a device claiming more apps than one could hold is refused whole', async () => {
    const db = fakeD1();
    const outcome = await report(
      db,
      Array.from({ length: 201 }, () => held()),
    );

    expect(outcome).toMatchObject({ ok: false, status: 400 });
    expect(await tally(db)).toEqual([]);
  });

  test('the same app from two sources is counted per source', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed');
    await report(db, [held(APP_ID, OFFICIAL_CATALOG_URL), held(APP_ID, LISTED)]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 }), counted(1, { '1.0.0': 1 }, APP_ID, LISTED)]);
  });

  test('an app from a source nobody submitted is dropped, so the tally cannot be stuffed from anywhere', async () => {
    const db = fakeD1();
    const outcome = await report(db, [held(APP_ID, 'https://nobody.example/catalog.json')]);

    expect(outcome.ok && outcome.record.apps).toEqual([]);
    expect(await tally(db)).toEqual([]);
  });

  test('a quarantined source is dropped the same way, matching what the store will merge', async () => {
    const db = fakeD1();
    await seedSource(db, QUARANTINED, 'quarantined');
    await report(db, [held(APP_ID, QUARANTINED)]);

    expect(await tally(db)).toEqual([]);
  });

  test('a source that gets rejected stops counting on the next report', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed');
    await report(db, [held(APP_ID, LISTED)]);
    await db.prepare(`UPDATE sources SET status = 'rejected'`).run();

    await report(db, [held(APP_ID, LISTED)]);

    expect(await tally(db)).toEqual([]);
  });

  test('one uncountable app never costs the device the rest of its inventory', async () => {
    const db = fakeD1();
    await report(db, [held(APP_ID), held('not-a-uuid'), 'garbage', held(OTHER_APP_ID, 'http://insecure.example/')]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 })]);
  });

  test('two spellings of one source url are one tally', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed');
    await report(db, [held(APP_ID, `${LISTED}#frag`)], serialOf(1));
    await report(db, [held(APP_ID, LISTED)], serialOf(2));

    expect(await tally(db)).toEqual([counted(2, { '1.0.0': 2 }, APP_ID, LISTED)]);
  });

  test('two spellings of one app id are one tally, and one device listing it twice is one device', async () => {
    const db = fakeD1();
    await report(db, [held(APP_ID.toUpperCase()), held(APP_ID)]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 })]);
  });

  test('a version that is not a string lands in the unknown bucket rather than refusing the device', async () => {
    const db = fakeD1();
    await report(db, [held(APP_ID, OFFICIAL_CATALOG_URL, 42)], serialOf(1));
    await report(db, [{ app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL }], serialOf(2));

    expect(await tally(db)).toEqual([counted(2, { unknown: 2 })]);
  });
});

describe('version tallies', () => {
  test('an upgrade moves a device between buckets and leaves the count alone', async () => {
    const db = fakeD1();
    await report(db, [held()], serialOf(1));
    await report(db, [held()], serialOf(2));
    await report(db, [held(APP_ID, OFFICIAL_CATALOG_URL, '2.0.0')], serialOf(1));

    expect(await tally(db)).toEqual([counted(2, { '1.0.0': 1, '2.0.0': 1 })]);
  });

  test('an emptied bucket is dropped rather than left at zero', async () => {
    const db = fakeD1();
    await report(db, [held()]);
    await report(db, [held(APP_ID, OFFICIAL_CATALOG_URL, '2.0.0')]);

    expect(await tally(db)).toEqual([counted(1, { '2.0.0': 1 })]);
  });

  test('a downgrade moves the device back the same way', async () => {
    const db = fakeD1();
    await report(db, [held(APP_ID, OFFICIAL_CATALOG_URL, '2.0.0')]);
    await report(db, [held()]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 })]);
  });

  test('an app that moves to another source takes its tally with it', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed');
    await report(db, [held(APP_ID, OFFICIAL_CATALOG_URL)]);
    await report(db, [held(APP_ID, LISTED)]);

    expect(await tally(db)).toEqual([counted(1, { '1.0.0': 1 }, APP_ID, LISTED)]);
  });

  test('the tally orders by count so a client can sort by popularity', async () => {
    const db = fakeD1();
    await report(db, [held(OTHER_APP_ID)], serialOf(1));
    await report(db, [held(APP_ID), held(OTHER_APP_ID)], serialOf(2));

    expect((await tally(db)).map(entry => entry.app_id)).toEqual([OTHER_APP_ID, APP_ID]);
  });
});

describe('admission', () => {
  async function admitted(db: FakeD1, apps: unknown[]): Promise<InventoryChange[]> {
    const seen: InventoryChange[] = [];
    await recordInstalled({
      db,
      body: { device_id: DEVICE, apps },
      now: NOW,
      admit: async change => {
        seen.push(change);
        return null;
      },
    });
    return seen;
  }

  test('a device nobody has seen is asked about as new', async () => {
    expect(await admitted(fakeD1(), [held()])).toEqual([{ known: false, serial: DEVICE }]);
  });

  test('a known device changing its apps is asked about as a change', async () => {
    const db = fakeD1();
    await report(db, [held()]);

    expect(await admitted(db, [])).toEqual([{ known: true, serial: DEVICE }]);
  });

  test('an unchanged report is never asked about, so it costs no budget', async () => {
    const db = fakeD1();
    await report(db, [held()]);

    expect(await admitted(db, [held()])).toEqual([]);
  });

  test('a refusal writes nothing', async () => {
    const db = fakeD1();
    const outcome = await recordInstalled({
      db,
      body: { device_id: DEVICE, apps: [held()] },
      now: NOW,
      admit: async () => ({ ok: false, status: 429, reason: 'slow down' }),
    });

    expect(outcome).toEqual({ ok: false, status: 429, reason: 'slow down' });
    expect(await tally(db)).toEqual([]);
    expect(await db.prepare('SELECT * FROM devices').all()).toMatchObject({ results: [] });
  });
});
