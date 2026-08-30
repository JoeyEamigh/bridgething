import { describe, expect, test } from 'bun:test';
import type { AppEntry, Catalog } from '@bridgething/catalog';
import { mergedApps, OFFICIAL_CATALOG_URL } from './apps.ts';
import type { SourceRecord, SourceStatus } from './directory.ts';
import { recordInstall } from './installs.ts';
import { fakeKv } from './kv-fake.ts';
import { writeSource } from './store.ts';

const ATTESTED_URL = 'https://attested.example/catalog.json';
const LISTED_URL = 'https://listed.example/catalog.json';
const QUARANTINED_URL = 'https://unreviewed.example/catalog.json';

function app(id: string, name: string): AppEntry {
  return {
    id,
    name,
    description: `${name} for bridgething`,
    author: 'somebody',
    icon: `https://example.com/${name}.svg`,
    homepage: null,
    source: null,
    versions: [
      {
        version: '1.0.0',
        released_at: '2026-07-01T00:00:00Z',
        download: { url: `https://example.com/${name}.zip`, size: 1024, sha256: 'a'.repeat(64) },
        permissions: [],
        min_libbridgething_version: '0.4.0',
        changelog: null,
      },
    ],
  };
}

function catalog(name: string, apps: AppEntry[]): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-07-01T00:00:00Z',
    repo: { name, description: `apps from ${name}`, homepage: null, icon: 'https://example.com/repo.svg' },
    apps,
    recommended_sources: [],
  };
}

function record(url: string, status: SourceStatus, name: string): SourceRecord {
  return {
    url,
    name,
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

const BODIES: Record<string, Catalog> = {
  [OFFICIAL_CATALOG_URL]: catalog('bridgething apps', [app('019e6701-13f8-71b5-ba04-85d326630e98', 'calendar')]),
  [ATTESTED_URL]: catalog('attested apps', [app('019e6701-13f8-71b5-ba04-000000000001', 'transit')]),
  [LISTED_URL]: catalog('listed apps', [app('019e6701-13f8-71b5-ba04-000000000002', 'gauges')]),
};

function stubFetch(
  overrides: Record<string, { status?: number; body?: unknown; throws?: boolean }> = {},
): typeof fetch {
  return (async (input: string) => {
    const override = overrides[input];
    if (override?.throws) throw new TypeError('network down');
    const body = override?.body ?? BODIES[input];
    if (body === undefined) throw new TypeError(`no stub for ${input}`);
    const text = typeof body === 'string' ? body : JSON.stringify(body);
    return new Response(text, {
      status: override?.status ?? 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as unknown as typeof fetch;
}

async function kvWithDirectory() {
  const kv = fakeKv();
  await writeSource(kv, record(ATTESTED_URL, 'attested', 'attested apps'));
  await writeSource(kv, record(LISTED_URL, 'listed', 'listed apps'));
  await writeSource(kv, record(QUARANTINED_URL, 'quarantined', 'unreviewed apps'));
  return kv;
}

const NOW = '2026-07-25T00:00:00.000Z';

describe('mergedApps', () => {
  test('merges the official catalog with every published source', async () => {
    const merged = await mergedApps({ kv: await kvWithDirectory(), now: NOW, fetchImpl: stubFetch() });

    expect(merged.catalogs.map(entry => entry.url)).toEqual([OFFICIAL_CATALOG_URL, ATTESTED_URL, LISTED_URL]);
    expect(merged.failures).toEqual([]);
  });

  test('leaves quarantined sources out, matching the relay allowlist', async () => {
    const merged = await mergedApps({ kv: await kvWithDirectory(), now: NOW, fetchImpl: stubFetch() });

    expect(merged.catalogs.some(entry => entry.url === QUARANTINED_URL)).toBe(false);
  });

  test('orders official first and attested ahead of listed, so a uuid collision resolves to the official app', async () => {
    const collision = catalog('listed apps', [app('019e6701-13f8-71b5-ba04-85d326630e98', 'not-calendar')]);
    const merged = await mergedApps({
      kv: await kvWithDirectory(),
      now: NOW,
      fetchImpl: stubFetch({ [LISTED_URL]: { body: collision } }),
    });

    expect(merged.catalogs[0]!.url).toBe(OFFICIAL_CATALOG_URL);
    expect(merged.catalogs[0]!.official).toBe(true);
    expect(merged.catalogs[1]!.attested).toBe(true);
    expect(merged.catalogs[2]!.attested).toBe(false);
  });

  test('a dead source is reported without losing the rest', async () => {
    const merged = await mergedApps({
      kv: await kvWithDirectory(),
      now: NOW,
      fetchImpl: stubFetch({ [LISTED_URL]: { throws: true } }),
    });

    expect(merged.catalogs.map(entry => entry.url)).toEqual([OFFICIAL_CATALOG_URL, ATTESTED_URL]);
    expect(merged.failures).toHaveLength(1);
    expect(merged.failures[0]!.url).toBe(LISTED_URL);
  });

  test('a source serving something that is not catalog.v1 is a failure, not a merge', async () => {
    const merged = await mergedApps({
      kv: await kvWithDirectory(),
      now: NOW,
      fetchImpl: stubFetch({ [ATTESTED_URL]: { body: '<html>gotcha</html>' } }),
    });

    expect(merged.catalogs.map(entry => entry.url)).toEqual([OFFICIAL_CATALOG_URL, LISTED_URL]);
    expect(merged.failures[0]!.reason).toContain('valid json');
  });

  test('the official catalog going down still serves community apps', async () => {
    const merged = await mergedApps({
      kv: await kvWithDirectory(),
      now: NOW,
      fetchImpl: stubFetch({ [OFFICIAL_CATALOG_URL]: { throws: true } }),
    });

    expect(merged.catalogs.map(entry => entry.url)).toEqual([ATTESTED_URL, LISTED_URL]);
    expect(merged.failures[0]!.url).toBe(OFFICIAL_CATALOG_URL);
  });

  test('install counts ride along, so a client can sort by popularity from one request', async () => {
    const kv = await kvWithDirectory();
    await recordInstall({
      kv,
      body: { app_id: '019e6701-13f8-71b5-ba04-000000000002', source_url: LISTED_URL },
      now: NOW,
    });

    const merged = await mergedApps({ kv, now: NOW, fetchImpl: stubFetch() });

    expect(merged.installs).toEqual([
      { app_id: '019e6701-13f8-71b5-ba04-000000000002', source_url: LISTED_URL, count: 1 },
    ]);
  });

  test('a directory nobody has installed from reports no counts rather than omitting the field', async () => {
    const merged = await mergedApps({ kv: await kvWithDirectory(), now: NOW, fetchImpl: stubFetch() });

    expect(merged.installs).toEqual([]);
  });

  test('a source that also publishes the official url is not fetched twice', async () => {
    const kv = await kvWithDirectory();
    await writeSource(kv, record(OFFICIAL_CATALOG_URL, 'attested', 'bridgething apps'));
    const merged = await mergedApps({ kv, now: NOW, fetchImpl: stubFetch() });

    expect(merged.catalogs.filter(entry => entry.url === OFFICIAL_CATALOG_URL)).toHaveLength(1);
  });
});
