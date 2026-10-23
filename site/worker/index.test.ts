import { afterEach, beforeEach, describe, expect, setSystemTime, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL } from '@bridgething/catalog';
import { readCatalog } from './catalogs.ts';
import { fakeD1, type FakeD1 } from './d1-fake.ts';
import type { Env } from './env.ts';
import { APP_ID, catalog, DEVICE, NOW, origin, seedOfficial, seedSource, serialOf } from './fixtures.ts';
import { listInstallCounts } from './installs.ts';
import { INVENTORY_CHANGES, NEW_DEVICES, NEW_SOURCES, SOURCE_REFRESHES } from './limits.ts';
import { readSource } from './sources.ts';

const CLIENT = '203.0.113.7';
const ADMIN_TOKEN = 'admin-secret';
const LISTED = 'https://listed.example/catalog.json';

(globalThis as unknown as { caches: unknown }).caches = {
  default: { match: () => Promise.resolve(undefined), put: () => Promise.resolve() },
};

const worker = (await import('./index.ts')).default;

let db: FakeD1;
const originalFetch = globalThis.fetch;

function env(): Env {
  return { DB: db, ASSETS: {} as Fetcher, ADMIN_TOKEN };
}

function context(): ExecutionContext {
  return { waitUntil: () => undefined, passThroughOnException: () => undefined } as unknown as ExecutionContext;
}

function call(
  method: string,
  path: string,
  init: { body?: unknown; token?: string; client?: string } = {},
): Promise<Response> {
  const headers: Record<string, string> = { 'cf-connecting-ip': init.client ?? CLIENT };
  if (init.body !== undefined) headers['content-type'] = 'application/json';
  if (init.token) headers['authorization'] = `Bearer ${init.token}`;
  const request = new Request(`https://bridgething.com${path}`, {
    method,
    headers,
    body: init.body === undefined ? undefined : JSON.stringify(init.body),
  });
  return worker.fetch(request, env(), context());
}

function inventory(device = DEVICE, apps = [{ app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL, version: '1.0.0' }]) {
  return { device_id: device, apps };
}

beforeEach(() => {
  db = fakeD1();
  setSystemTime(new Date(NOW));
});

afterEach(() => {
  globalThis.fetch = originalFetch;
  setSystemTime();
});

describe('POST /api/installed', () => {
  test('an inventory is accepted and answered with what it counted', async () => {
    const response = await call('POST', '/api/installed', { body: inventory() });

    expect(response.status).toBe(202);
    expect(await response.json<{ apps: number }>()).toEqual({ apps: 1 });
  });

  test('one network cannot mint devices without limit', async () => {
    const statuses = [];
    for (let n = 0; n <= NEW_DEVICES.limit; n += 1) {
      statuses.push((await call('POST', '/api/installed', { body: inventory(serialOf(n)) })).status);
    }

    expect(statuses.slice(0, NEW_DEVICES.limit).every(status => status === 202)).toBe(true);
    expect(statuses.at(-1)).toBe(429);
    expect((await listInstallCounts(db))[0]!.count).toBe(NEW_DEVICES.limit);
  });

  test('the device limit is per network, so one busy household cannot silence everyone else', async () => {
    for (let n = 0; n < NEW_DEVICES.limit; n += 1) {
      await call('POST', '/api/installed', { body: inventory(serialOf(n)) });
    }

    const elsewhere = await call('POST', '/api/installed', { body: inventory(serialOf(99)), client: '198.51.100.1' });
    expect(elsewhere.status).toBe(202);
  });

  test('a known device reporting the same apps never spends budget', async () => {
    for (let n = 0; n < INVENTORY_CHANGES.limit + 5; n += 1) {
      expect((await call('POST', '/api/installed', { body: inventory() })).status).toBe(202);
    }
  });

  test('a device flapping its apps is cut off for the day', async () => {
    await call('POST', '/api/installed', { body: inventory() });
    const statuses = [];
    for (let n = 0; n <= INVENTORY_CHANGES.limit; n += 1) {
      statuses.push((await call('POST', '/api/installed', { body: inventory(DEVICE, n % 2 ? undefined : []) })).status);
    }

    expect(statuses.at(-1)).toBe(429);
  });

  test('a body that is not json is refused rather than counted', async () => {
    const request = new Request('https://bridgething.com/api/installed', { method: 'POST', body: 'not json' });

    expect((await worker.fetch(request, env(), context())).status).toBe(400);
  });

  test('the endpoint only takes posts', async () => {
    expect((await call('GET', '/api/installed')).status).toBe(404);
  });
});

describe('POST /api/sources', () => {
  test('a new source is listed and answers 201', async () => {
    globalThis.fetch = origin({ [LISTED]: {} }).fetch;
    const response = await call('POST', '/api/sources', { body: { url: LISTED } });

    expect(response.status).toBe(201);
    expect((await response.json<{ source: { status: string } }>()).source.status).toBe('listed');
  });

  test('one network cannot flood the directory with new sources', async () => {
    const urls = Array.from({ length: NEW_SOURCES.limit + 1 }, (_, n) => `https://s${n}.example/catalog.json`);
    globalThis.fetch = origin(Object.fromEntries(urls.map(url => [url, {}]))).fetch;

    const statuses = [];
    for (const url of urls) statuses.push((await call('POST', '/api/sources', { body: { url } })).status);

    expect(statuses).toEqual([...Array(NEW_SOURCES.limit).fill(201), 429]);
  });

  test('refreshing a source is limited per source, not per network', async () => {
    globalThis.fetch = origin({ [LISTED]: {} }).fetch;
    await call('POST', '/api/sources', { body: { url: LISTED } });

    const statuses = [];
    for (let n = 0; n <= SOURCE_REFRESHES.limit; n += 1) {
      statuses.push((await call('POST', '/api/sources', { body: { url: LISTED }, client: `198.51.100.${n}` })).status);
    }

    expect(statuses).toEqual([...Array(SOURCE_REFRESHES.limit).fill(200), 429]);
  });

  test('reporting an inventory does not spend the budget for submitting sources', async () => {
    for (let n = 0; n < NEW_DEVICES.limit; n += 1) {
      await call('POST', '/api/installed', { body: inventory(serialOf(n)) });
    }
    globalThis.fetch = origin({ [LISTED]: {} }).fetch;

    expect((await call('POST', '/api/sources', { body: { url: LISTED } })).status).toBe(201);
  });

  test('a body with no url is refused', async () => {
    expect((await call('POST', '/api/sources', { body: {} })).status).toBe(400);
  });
});

describe('/api/admin/sources', () => {
  test('needs the admin token', async () => {
    expect((await call('GET', '/api/admin/sources')).status).toBe(401);
    expect((await call('GET', '/api/admin/sources', { token: 'wrong' })).status).toBe(401);
  });

  test('lists every source with the admin note', async () => {
    await seedSource(db, LISTED, 'rejected');
    const response = await call('GET', '/api/admin/sources', { token: ADMIN_TOKEN });

    expect(await response.json<unknown>()).toMatchObject({
      sources: [{ url: LISTED, status: 'rejected', note: null }],
    });
  });

  test('moves a source and records the review', async () => {
    await seedSource(db, LISTED, 'listed');
    const response = await call('PATCH', '/api/admin/sources', {
      token: ADMIN_TOKEN,
      body: { url: LISTED, status: 'attested', note: 'checked it' },
    });

    expect(response.status).toBe(200);
    expect(await readSource(db, LISTED)).toMatchObject({
      status: 'attested',
      reviewed_at: NOW,
      reviewed_by: 'admin',
      note: 'checked it',
    });
  });

  test('refuses a status that is not one of the four', async () => {
    await seedSource(db, LISTED, 'listed');

    expect(
      (await call('PATCH', '/api/admin/sources', { token: ADMIN_TOKEN, body: { url: LISTED, status: 'banned' } }))
        .status,
    ).toBe(400);
  });
});

describe('public reads', () => {
  test('apps.json, sources.json and directory.json come from the store as compact json', async () => {
    await seedOfficial(db);
    await seedSource(db, LISTED, 'listed', catalog('listed'));

    for (const path of ['/api/apps.json', '/api/sources.json', '/api/directory.json']) {
      const response = await call('GET', path);
      const text = await response.text();
      expect(response.headers.get('content-type')).toContain('application/json');
      expect(response.headers.get('cache-control')).toBe('public, max-age=60');
      expect(text).not.toContain('\n');
      expect(() => JSON.parse(text)).not.toThrow();
    }
  });

  test('the relay answers with the stored catalog', async () => {
    await seedSource(db, LISTED, 'listed', catalog('listed'));
    const response = await call('GET', `/api/catalog?url=${encodeURIComponent(LISTED)}`);

    expect(await response.json<unknown>()).toEqual(catalog('listed'));
  });

  test('a head request answers with headers and no body', async () => {
    await seedOfficial(db);
    const response = await call('HEAD', '/api/apps.json');

    expect(response.status).toBe(200);
    expect(await response.text()).toBe('');
  });
});

describe('GET /oauth/callback', () => {
  test('renders the bounce page without touching the asset store', async () => {
    const response = await call('GET', '/oauth/callback?code=abc&state=xyz');

    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toContain('text/html');
    expect(await response.text()).toContain('<a href="bridgething://oauth/callback?code=abc&amp;state=xyz">');
  });
});

describe('the scheduled refresh', () => {
  test('refreshes catalogs and prunes spent rate limit windows', async () => {
    globalThis.fetch = origin({ [OFFICIAL_CATALOG_URL]: { body: catalog('bridgething apps') } }).fetch;
    await db
      .prepare('INSERT INTO rate_limits (bucket, key, window_start, used) VALUES (?1, ?2, ?3, ?4)')
      .bind('device:new', CLIENT, 0, 1)
      .run();

    await worker.scheduled({} as ScheduledController, env());

    expect((await readCatalog(db, OFFICIAL_CATALOG_URL))?.name).toBe('bridgething apps');
    expect((await db.prepare('SELECT * FROM rate_limits').all()).results).toEqual([]);
  });
});
