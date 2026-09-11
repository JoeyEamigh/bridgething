import { describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL } from '@bridgething/catalog';
import type { SourceRecord, SourceStatus } from './directory.ts';
import {
  installKeyFor,
  listInstalls,
  rebuildInstalls,
  recordInstall,
  recountInstalls,
  seenKeyFor,
  toInstallCounts,
  UNKNOWN_VERSION,
} from './installs.ts';
import { fakeKv, withListLag, type FakeKv } from './kv-fake.ts';
import { listSources, writeSource } from './store.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const WEATHER_ID = '019e6701-13f8-71b5-ba04-81f347137de2';
const LISTED_URL = 'https://listed.example/catalog.json';
const QUARANTINED_URL = 'https://unreviewed.example/catalog.json';
const UNKNOWN_URL = 'https://nobody.example/catalog.json';

const ONE = '8558R481Q61R';
const TWO = '8558R58QQ716';
const THREE = '8557R18NQN1P';

const NOW = '2026-07-25T00:00:00.000Z';
const LATER = '2026-07-26T00:00:00.000Z';

function record(url: string, status: SourceStatus): SourceRecord {
  return {
    url,
    name: url,
    description: null,
    homepage: null,
    icon: null,
    status,
    submitted_at: '2026-07-01T00:00:00.000Z',
    reviewed_at: null,
    reviewed_by: null,
    app_count: 1,
    last_checked_at: '2026-07-20T00:00:00.000Z',
    last_check_ok: true,
    last_check_error: null,
    downloads_cors_ok: true,
    note: null,
  };
}

async function directory(): Promise<FakeKv> {
  const kv = fakeKv();
  await writeSource(kv, record(LISTED_URL, 'listed'));
  await writeSource(kv, record(QUARANTINED_URL, 'quarantined'));
  return kv;
}

function beacon(
  appId: string,
  sourceUrl: string,
  device: string = ONE,
  version: string | null = null,
): Record<string, unknown> {
  return { app_id: appId, source_url: sourceUrl, device_id: device, version };
}

function counted(appId: string, sourceUrl: string, count: number, versions: Record<string, number>) {
  return { app_id: appId, source_url: sourceUrl, count, versions };
}

describe('recordInstall', () => {
  test('the first device to install an app starts its tally at one', async () => {
    const kv = await directory();

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    expect(outcome.ok).toBe(true);
    expect(outcome.ok && outcome.record.count).toBe(1);
  });

  test('distinct devices installing one app accumulate', async () => {
    const kv = await directory();

    for (const device of [ONE, TWO, THREE]) {
      await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, device), now: NOW });
    }

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      counted(CALENDAR_ID, LISTED_URL, 3, { [UNKNOWN_VERSION]: 3 }),
    ]);
  });

  test('one device reinstalling the same version never moves the tally', async () => {
    const kv = await directory();

    for (let i = 0; i < 5; i += 1) {
      await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    }

    expect(toInstallCounts(await listInstalls(kv))).toEqual([counted(CALENDAR_ID, LISTED_URL, 1, { '1.0.0': 1 })]);
  });

  test('a repeat of the same version costs one kv read and writes nothing', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    kv.resetCounts();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: LATER });

    expect(kv.counts.put).toBe(0);
  });

  test('serials differing only in case are one device', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE.toLowerCase()), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))[0]!.count).toBe(1);
  });

  test('a device that is not a car thing serial is refused', async () => {
    const kv = await directory();

    for (const device of ['', '   ', 'superbird0', '8558R481Q61', '8558R481Q61RR', '8558:481Q61R', '../../etc']) {
      expect(await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, device), now: NOW })).toMatchObject({
        ok: false,
        status: 400,
      });
    }
  });

  test('a beacon with no device is refused rather than counted anonymously', async () => {
    const kv = await directory();

    const outcome = await recordInstall({
      kv,
      body: { app_id: CALENDAR_ID, source_url: LISTED_URL, version: '1.0.0' },
      now: NOW,
    });

    expect(outcome).toMatchObject({ ok: false, status: 400 });
    expect(await listInstalls(kv)).toHaveLength(0);
  });

  test('the same app from two sources is counted per source', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, OFFICIAL_CATALOG_URL, TWO), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))).toHaveLength(2);
  });

  test('a tally is readable straight after the install that made it', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW });

    expect(await listInstalls(kv)).toHaveLength(1);
  });

  test('a fresh tally is served before kv list can enumerate the record behind it', async () => {
    const kv = withListLag(await directory());

    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      counted(WEATHER_ID, LISTED_URL, 1, { [UNKNOWN_VERSION]: 1 }),
    ]);
  });

  test('two installs recorded at once both land in the tally under list lag', async () => {
    const kv = withListLag(await directory());

    await Promise.all([
      recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW }),
      recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW }),
    ]);

    expect(
      toInstallCounts(await listInstalls(kv))
        .map(entry => entry.app_id)
        .sort(),
    ).toEqual([CALENDAR_ID, WEATHER_ID].sort());
  });

  test('a warm tally reads with one kv get, not one per app', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW });
    await listInstalls(kv);
    kv.resetCounts();

    expect(await listInstalls(kv)).toHaveLength(1);
    expect(kv.counts.get).toBe(1);
    expect(kv.counts.list).toBe(0);
  });

  test('the official catalog counts without being submitted to the directory', async () => {
    const kv = fakeKv();

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, OFFICIAL_CATALOG_URL), now: NOW });

    expect(outcome.ok).toBe(true);
  });

  test('a source nobody submitted is refused, so the tally cannot be stuffed from anywhere', async () => {
    const kv = await directory();

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, UNKNOWN_URL), now: NOW });

    expect(outcome).toMatchObject({ ok: false, status: 404 });
    expect(await listInstalls(kv)).toHaveLength(0);
  });

  test('a quarantined source is refused the same way, matching what the store will merge', async () => {
    const kv = await directory();

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, QUARANTINED_URL), now: NOW });

    expect(outcome).toMatchObject({ ok: false, status: 404 });
  });

  test('two spellings of one source url are one tally', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, `${LISTED_URL}#apps`, TWO), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      counted(CALENDAR_ID, LISTED_URL, 2, { [UNKNOWN_VERSION]: 2 }),
    ]);
  });

  test('two spellings of one app id are one tally', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID.toUpperCase(), LISTED_URL, TWO), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      counted(CALENDAR_ID, LISTED_URL, 2, { [UNKNOWN_VERSION]: 2 }),
    ]);
  });

  test('an app id that is not a catalog uuid is refused', async () => {
    const kv = await directory();

    for (const id of ['', '  ', 'calendar', '../../etc/passwd', `${CALENDAR_ID}extra`]) {
      expect(await recordInstall({ kv, body: beacon(id, LISTED_URL), now: NOW })).toMatchObject({
        ok: false,
        status: 400,
      });
    }
  });

  test('a body missing its fields is refused rather than counted as something', async () => {
    const kv = await directory();

    expect(await recordInstall({ kv, body: null, now: NOW })).toMatchObject({ ok: false, status: 400 });
    expect(await recordInstall({ kv, body: { app_id: CALENDAR_ID }, now: NOW })).toMatchObject({
      ok: false,
      status: 400,
    });
    expect(await recordInstall({ kv, body: { source_url: LISTED_URL }, now: NOW })).toMatchObject({
      ok: false,
      status: 400,
    });
  });

  test('a source url that cannot be a source is refused by name', async () => {
    const kv = await directory();

    const outcome = await recordInstall({
      kv,
      body: beacon(CALENDAR_ID, 'ftp://listed.example/c.json'),
      now: NOW,
    });

    expect(outcome).toMatchObject({ ok: false, status: 400 });
  });

  test('a version that is not a string is refused rather than stored', async () => {
    const kv = await directory();

    const outcome = await recordInstall({
      kv,
      body: { app_id: CALENDAR_ID, source_url: LISTED_URL, device_id: ONE, version: { major: 1 } },
      now: NOW,
    });

    expect(outcome).toMatchObject({ ok: false, status: 400 });
  });

  test('a corrupt tally restarts at one instead of poisoning the sort', async () => {
    const kv = await directory();
    await kv.put(installKeyFor(CALENDAR_ID, LISTED_URL), JSON.stringify({ count: 'lots', versions: 'nope' }));

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    expect(outcome.ok && outcome.record.count).toBe(1);
    expect(outcome.ok && outcome.record.versions).toEqual({ [UNKNOWN_VERSION]: 1 });
  });
});

describe('version tallies', () => {
  test('each device lands in the bucket for the version it installed', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, TWO, '1.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, THREE, '1.1.0'), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      counted(CALENDAR_ID, LISTED_URL, 3, { '1.0.0': 2, '1.1.0': 1 }),
    ]);
  });

  test('an upgrade moves a device between buckets and leaves the count alone', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.1.0'), now: LATER });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([counted(CALENDAR_ID, LISTED_URL, 1, { '1.1.0': 1 })]);
  });

  test('a downgrade moves the device back the same way', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.1.0'), now: LATER });

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: LATER });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([counted(CALENDAR_ID, LISTED_URL, 1, { '1.0.0': 1 })]);
  });

  test('an emptied bucket is dropped rather than left at zero', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.1.0'), now: LATER });

    const counts = toInstallCounts(await listInstalls(kv));

    expect(Object.keys(counts[0]!.versions)).toEqual(['1.1.0']);
  });

  test('the buckets always sum to the count', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, TWO, '1.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.2.0'), now: LATER });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, THREE, '1.2.0'), now: LATER });

    const [tally] = toInstallCounts(await listInstalls(kv));

    expect(Object.values(tally!.versions).reduce((sum, each) => sum + each, 0)).toBe(tally!.count);
  });

  test('an install with no version lands in the unknown bucket', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))[0]!.versions).toEqual({ [UNKNOWN_VERSION]: 1 });
  });

  test('an upgrade arriving from a second source stays credited to the first', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });

    await recordInstall({ kv, body: beacon(CALENDAR_ID, OFFICIAL_CATALOG_URL, ONE, '1.1.0'), now: LATER });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([counted(CALENDAR_ID, LISTED_URL, 1, { '1.1.0': 1 })]);
  });
});

describe('recountInstalls', () => {
  test('derives every record from the device markers', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, TWO, '1.1.0'), now: NOW });
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL, ONE, '2.0.0'), now: NOW });

    expect(toInstallCounts(await recountInstalls(kv))).toEqual([
      counted(CALENDAR_ID, LISTED_URL, 2, { '1.0.0': 1, '1.1.0': 1 }),
      counted(WEATHER_ID, LISTED_URL, 1, { '2.0.0': 1 }),
    ]);
  });

  test('repairs a record that drifted away from its markers', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    await kv.put(
      installKeyFor(CALENDAR_ID, LISTED_URL),
      JSON.stringify({ app_id: CALENDAR_ID, source_url: LISTED_URL, count: 99, versions: { '9.9.9': 99 } }),
    );

    expect(toInstallCounts(await recountInstalls(kv))).toEqual([counted(CALENDAR_ID, LISTED_URL, 1, { '1.0.0': 1 })]);
  });

  test('never lowers a count that the markers still back', async () => {
    const kv = await directory();
    for (const device of [ONE, TWO, THREE]) {
      await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, device, '1.0.0'), now: NOW });
    }

    expect(toInstallCounts(await recountInstalls(kv))[0]!.count).toBe(3);
    expect(toInstallCounts(await recountInstalls(kv))[0]!.count).toBe(3);
  });

  test('rewrites nothing when the records already match the markers', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });
    await recountInstalls(kv);
    kv.resetCounts();

    await recountInstalls(kv);

    expect(kv.counts.put).toBe(1);
  });

  test('drops a record no marker backs, so the read path cannot resurrect it', async () => {
    const kv = await directory();
    await kv.put(
      installKeyFor(WEATHER_ID, LISTED_URL),
      JSON.stringify({ app_id: WEATHER_ID, source_url: LISTED_URL, count: 500, versions: { '1.0.0': 500 } }),
    );
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE, '1.0.0'), now: NOW });

    await recountInstalls(kv);
    await kv.delete('directory:installs');

    expect(toInstallCounts(await listInstalls(kv))).toEqual([counted(CALENDAR_ID, LISTED_URL, 1, { '1.0.0': 1 })]);
  });

  test('leaves the source directory alone', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    const sources = await listSources(kv);

    expect(await recountInstalls(kv)).toHaveLength(1);
    expect(await listSources(kv)).toEqual(sources);
  });
});

describe('rebuildInstalls', () => {
  test('repairs a snapshot that drifted from the per-app records', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await kv.put('directory:installs', JSON.stringify([{ app_id: WEATHER_ID, source_url: LISTED_URL, count: 99 }]));

    expect(toInstallCounts(await rebuildInstalls(kv))).toEqual([
      counted(CALENDAR_ID, LISTED_URL, 1, { [UNKNOWN_VERSION]: 1 }),
    ]);
  });

  test('rebuilds from the records when the snapshot is missing or corrupt', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    await kv.delete('directory:installs');
    expect(await listInstalls(kv)).toHaveLength(1);

    await kv.put('directory:installs', 'not json');
    expect(await listInstalls(kv)).toHaveLength(1);
  });

  test('keeps a fresh tally the snapshot holds but kv list cannot enumerate yet', async () => {
    const kv = withListLag(await directory());
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW });

    expect(toInstallCounts(await rebuildInstalls(kv))).toHaveLength(2);
    expect(toInstallCounts(await listInstalls(kv))).toHaveLength(2);
  });

  test('drops a snapshot tally whose backing record is gone', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW });
    await kv.delete(installKeyFor(WEATHER_ID, LISTED_URL));

    expect(toInstallCounts(await rebuildInstalls(kv))).toEqual([
      counted(CALENDAR_ID, LISTED_URL, 1, { [UNKNOWN_VERSION]: 1 }),
    ]);
  });

  test('does not walk the device markers on the read path', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE), now: NOW });
    await kv.delete('directory:installs');
    kv.resetCounts();

    await listInstalls(kv);

    expect(kv.snapshot()[seenKeyFor(CALENDAR_ID, ONE)]).toBeDefined();
    expect(kv.counts.get).toBeLessThan(5);
  });
});

describe('toInstallCounts', () => {
  test('drops what a client has no business seeing and orders by tally', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL, ONE, '2.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, ONE), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, TWO), now: NOW });

    const counts = toInstallCounts(await listInstalls(kv));

    expect(counts.map(entry => entry.count)).toEqual([2, 1]);
    expect(Object.keys(counts[0]!)).toEqual(['app_id', 'source_url', 'count', 'versions']);
  });

  test('a zeroed record is left out of the public tally', () => {
    const counts = toInstallCounts([
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 0, versions: {}, last_at: NOW },
    ]);

    expect(counts).toEqual([]);
  });
});
