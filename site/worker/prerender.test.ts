import { describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL } from '@bridgething/catalog';
import { injectStoreData, storeData, STORE_DATA_ID } from './prerender.ts';
import { fakeKv } from './kv-fake.ts';
import { recordInstall } from './installs.ts';

const NOW = '2026-07-25T00:00:00.000Z';
const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const DEVICE = '8558R481Q61R';

const CATALOG = {
  schema: 'catalog.v1',
  updated_at: '2026-07-01T00:00:00Z',
  repo: { name: 'official', description: 'the official catalog', homepage: null, icon: null },
  apps: [
    {
      id: APP_ID,
      name: 'calendar',
      description: 'calendar for bridgething',
      author: 'somebody',
      icon: null,
      homepage: null,
      source: null,
      versions: [
        {
          version: '1.0.0',
          released_at: '2026-07-01T00:00:00Z',
          download: { url: 'https://example.com/calendar.zip', size: 1024, sha256: 'a'.repeat(64) },
          permissions: [],
          min_libbridgething_version: '0.4.0',
          changelog: null,
        },
      ],
    },
  ],
  recommended_sources: [],
};

function stubFetch(): typeof fetch {
  return (() =>
    Promise.resolve(
      new Response(JSON.stringify(CATALOG), { status: 200, headers: { 'content-type': 'application/json' } }),
    )) as unknown as typeof fetch;
}

function page(body = `<script id="${STORE_DATA_ID}" type="application/json">null</script>`): Response {
  return new Response(`<!doctype html><html><body>${body}</body></html>`, {
    status: 200,
    headers: { 'content-type': 'text/html' },
  });
}

async function injected(body?: string): Promise<string> {
  const kv = fakeKv();
  await recordInstall({
    kv,
    body: { app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL, device_id: DEVICE, version: '1.0.0' },
    now: NOW,
  });
  const data = await storeData({ kv, now: NOW, fetchImpl: stubFetch() });
  return injectStoreData(page(body), data).text();
}

describe('injectStoreData', () => {
  test('fills the placeholder so the first paint already knows every listing', async () => {
    const html = await injected();
    const payload = JSON.parse(html.split(`type="application/json">`)[1]!.split('</script>')[0]!) as {
      apps: { catalogs: { url: string }[]; installs: { count: number }[] };
    };

    expect(payload.apps.catalogs.map(entry => entry.url)).toEqual([OFFICIAL_CATALOG_URL]);
    expect(payload.apps.installs[0]!.count).toBe(1);
  });

  test('carries the source directory too, so that section does not pop in either', async () => {
    const html = await injected();

    expect(html).toContain('"directory"');
  });

  test('escapes a closing tag so a catalog cannot break out of the script', async () => {
    const kv = fakeKv();
    const data = await storeData({ kv, now: NOW, fetchImpl: stubFetch() });
    const poisoned = { ...data, directory: [{ name: '</script><script>alert(1)</script>' }] } as typeof data;

    const html = await injectStoreData(page(), poisoned).text();

    expect(html).not.toContain('</script><script>alert(1)');
    expect(html).toContain('\\u003c/script');
  });

  test('a page without the placeholder is passed through untouched', async () => {
    const html = await injected('<p>no placeholder here</p>');

    expect(html).toContain('<p>no placeholder here</p>');
  });

  test('is cached at the edge but never in the browser, so counts stay fresh', async () => {
    const kv = fakeKv();
    const data = await storeData({ kv, now: NOW, fetchImpl: stubFetch() });

    const response = injectStoreData(page(), data);

    expect(response.headers.get('cache-control')).toBe('public, max-age=0, s-maxage=300');
  });
});
