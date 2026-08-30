import { describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL } from '@bridgething/catalog';
import type { SourceRecord, SourceStatus } from './directory.ts';
import { installKeyFor, listInstalls, rebuildInstalls, recordInstall, toInstallCounts } from './installs.ts';
import { fakeKv, withListLag, type FakeKv } from './kv-fake.ts';
import { listSources, writeSource } from './store.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const WEATHER_ID = '019e6701-13f8-71b5-ba04-81f347137de2';
const LISTED_URL = 'https://listed.example/catalog.json';
const QUARANTINED_URL = 'https://unreviewed.example/catalog.json';
const UNKNOWN_URL = 'https://nobody.example/catalog.json';

const NOW = '2026-07-25T00:00:00.000Z';

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

function beacon(appId: string, sourceUrl: string, version?: string): Record<string, unknown> {
  return { app_id: appId, source_url: sourceUrl, version: version ?? null };
}

describe('recordInstall', () => {
  test('the first install of an app starts its tally at one', async () => {
    const kv = await directory();

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    expect(outcome.ok).toBe(true);
    expect(outcome.ok && outcome.record.count).toBe(1);
  });

  test('installs of one app from one source accumulate', async () => {
    const kv = await directory();

    for (let i = 0; i < 3; i += 1) await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    const counts = toInstallCounts(await listInstalls(kv));
    expect(counts).toEqual([{ app_id: CALENDAR_ID, source_url: LISTED_URL, count: 3 }]);
  });

  test('the same app from two sources is counted per source', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, OFFICIAL_CATALOG_URL), now: NOW });

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

    expect(toInstallCounts(await listInstalls(kv))).toEqual([{ app_id: WEATHER_ID, source_url: LISTED_URL, count: 1 }]);
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

  test('two installs recorded at once both land in the tally', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    await Promise.all([
      recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW }),
      recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW }),
    ]);

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 2 },
      { app_id: WEATHER_ID, source_url: LISTED_URL, count: 1 },
    ]);
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

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, `${LISTED_URL}#apps`), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 2 },
    ]);
  });

  test('two spellings of one app id are one tally', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID.toUpperCase(), LISTED_URL), now: NOW });

    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 2 },
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

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, 'ftp://listed.example/c.json'), now: NOW });

    expect(outcome).toMatchObject({ ok: false, status: 400 });
  });

  test('a version that is not a string is refused rather than stored', async () => {
    const kv = await directory();

    const outcome = await recordInstall({
      kv,
      body: { app_id: CALENDAR_ID, source_url: LISTED_URL, version: { major: 1 } },
      now: NOW,
    });

    expect(outcome).toMatchObject({ ok: false, status: 400 });
  });

  test('the version of the last install is kept, and an absent one reads as none', async () => {
    const kv = await directory();

    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, '1.0.0'), now: NOW });
    const second = await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL, '1.1.0'), now: NOW });
    expect(second.ok && second.record.last_version).toBe('1.1.0');

    const third = await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    expect(third.ok && third.record.last_version).toBeNull();
  });

  test('a corrupt tally restarts at one instead of poisoning the sort', async () => {
    const kv = await directory();
    await kv.put(installKeyFor(CALENDAR_ID, LISTED_URL), JSON.stringify({ count: 'lots' }));

    const outcome = await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    expect(outcome.ok && outcome.record.count).toBe(1);
  });
});

describe('rebuildInstalls', () => {
  test('repairs a snapshot that drifted from the per-app records', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await kv.put('directory:installs', JSON.stringify([{ app_id: WEATHER_ID, source_url: LISTED_URL, count: 99 }]));

    expect(toInstallCounts(await rebuildInstalls(kv))).toEqual([
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 1 },
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

    expect(toInstallCounts(await rebuildInstalls(kv))).toEqual([
      { app_id: WEATHER_ID, source_url: LISTED_URL, count: 1 },
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 1 },
    ]);
    expect(toInstallCounts(await listInstalls(kv))).toHaveLength(2);
  });

  test('drops a snapshot tally whose backing record is gone', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL), now: NOW });
    await kv.delete(installKeyFor(WEATHER_ID, LISTED_URL));

    expect(toInstallCounts(await rebuildInstalls(kv))).toEqual([
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 1 },
    ]);
  });

  test('leaves the source directory alone', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    const sources = await listSources(kv);

    expect(await rebuildInstalls(kv)).toHaveLength(1);
    expect(await listSources(kv)).toEqual(sources);
  });
});

describe('toInstallCounts', () => {
  test('drops what a client has no business seeing and orders by tally', async () => {
    const kv = await directory();
    await recordInstall({ kv, body: beacon(WEATHER_ID, LISTED_URL, '2.0.0'), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });
    await recordInstall({ kv, body: beacon(CALENDAR_ID, LISTED_URL), now: NOW });

    const counts = toInstallCounts(await listInstalls(kv));

    expect(counts.map(entry => entry.count)).toEqual([2, 1]);
    expect(Object.keys(counts[0]!)).toEqual(['app_id', 'source_url', 'count']);
  });

  test('a zeroed record is left out of the public tally', () => {
    const counts = toInstallCounts([
      { app_id: CALENDAR_ID, source_url: LISTED_URL, count: 0, last_at: NOW, last_version: null },
    ]);

    expect(counts).toEqual([]);
  });
});
