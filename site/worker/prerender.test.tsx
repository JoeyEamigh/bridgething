import { afterEach, describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL } from '@bridgething/catalog';
import { renderToString } from 'preact-render-to-string';
import { AppDetail } from '../src/components/store/AppDetail.tsx';
import { StoreBrowser } from '../src/components/store/StoreBrowser.tsx';
import { storeData } from './apps.ts';
import { fakeD1 } from './d1-fake.ts';
import { admitAll, app, APP_ID, catalog, DEVICE, NOW, seedOfficial, seedSource } from './fixtures.ts';
import { recordInstalled } from './installs.ts';
import { injectStoreData, STORE_DATA_ID } from './prerender.tsx';

const LISTED = 'https://listed.example/catalog.json';
const STORE_PATH = '/apps/';
const DETAIL_PATH = `/apps/${APP_ID}`;

async function data(): Promise<string> {
  const db = fakeD1();
  await seedOfficial(db, catalog('bridgething apps', [app(APP_ID, 'calendar')]));
  await seedSource(db, LISTED, 'listed', catalog('listed', [app('019e6701-13f8-71b5-ba04-85d326630e99', 'weather')]));
  await recordInstalled({
    db,
    body: { device_id: DEVICE, apps: [{ app_id: APP_ID, source_url: OFFICIAL_CATALOG_URL, version: '1.0.0' }] },
    now: NOW,
    admit: admitAll,
  });
  return storeData({ db, now: NOW });
}

function page(island: string, placeholder = `<script id="${STORE_DATA_ID}" type="application/json">null</script>`) {
  return new Response(
    `<!doctype html><html><body>${placeholder}<astro-island component-export="${island}" ssr>` +
      `<p>built without the directory</p><!--astro:end--></astro-island></body></html>`,
    { status: 200, headers: { 'content-type': 'text/html' } },
  );
}

const original = (globalThis as { document?: unknown }).document;

afterEach(() => {
  (globalThis as { document?: unknown }).document = original;
});

function asTheBrowserSeesIt(injected: string): void {
  (globalThis as { document?: unknown }).document = {
    getElementById: (id: string) => (id === STORE_DATA_ID ? { textContent: injected } : null),
  };
}

function islandOf(html: string, name: string): string {
  return html.split(`component-export="${name}" ssr>`)[1]!.split('</astro-island>')[0]!;
}

describe('injectStoreData', () => {
  test('fills the placeholder so the island hydrates from the same data the page was rendered from', async () => {
    const html = await injectStoreData(page('StoreBrowser'), STORE_PATH, await data()).text();
    const payload = JSON.parse(html.split(`type="application/json">`)[1]!.split('</script>')[0]!) as {
      apps: { catalogs: { url: string }[]; installs: { count: number }[] };
      directory: { url: string }[];
    };

    expect(payload.apps.catalogs.map(entry => entry.url)).toEqual([OFFICIAL_CATALOG_URL, LISTED]);
    expect(payload.apps.installs[0]!.count).toBe(1);
    expect(payload.directory.map(entry => entry.url)).toEqual([LISTED]);
  });

  test('renders every listing into the store island, so nothing pops in after hydration', async () => {
    const island = islandOf(
      await injectStoreData(page('StoreBrowser'), STORE_PATH, await data()).text(),
      'StoreBrowser',
    );

    expect(island).not.toContain('built without the directory');
    expect(island).toContain('calendar');
    expect(island).toContain('weather');
    expect(island.endsWith('<!--astro:end-->')).toBe(true);
  });

  test('the store island is exactly what the browser renders first from the injected data', async () => {
    const injected = await data();
    const island = islandOf(await injectStoreData(page('StoreBrowser'), STORE_PATH, injected).text(), 'StoreBrowser');

    asTheBrowserSeesIt(injected);
    expect(island).toBe(`${renderToString(<StoreBrowser initial={[]} />)}<!--astro:end-->`);
  });

  test('an app page is rendered from the directory, including apps the build never knew about', async () => {
    const path = '/apps/019e6701-13f8-71b5-ba04-85d326630e99';
    const injected = await data();
    const island = islandOf(await injectStoreData(page('AppDetail'), path, injected).text(), 'AppDetail');

    expect(island).toContain('weather');
    asTheBrowserSeesIt(injected);
    expect(island).toBe(`${renderToString(<AppDetail baked={null} path={path} />)}<!--astro:end-->`);
  });

  test('the app page ignores what was baked in when the directory has spoken', async () => {
    const injected = await data();
    asTheBrowserSeesIt(injected);
    const stale = {
      app: app(APP_ID, 'stale build'),
      source: { url: OFFICIAL_CATALOG_URL, name: 'x', icon: null, official: true, attested: true },
    };

    expect(renderToString(<AppDetail baked={stale} path={DETAIL_PATH} />)).toBe(
      renderToString(<AppDetail baked={null} path={DETAIL_PATH} />),
    );
  });

  test('escapes a closing tag so a catalog cannot break out of the script', async () => {
    const poisoned = JSON.stringify({
      apps: { updated_at: NOW, catalogs: [], failures: [], installs: [] },
      directory: [{ name: '</script><script>alert(1)</script>' }],
    });

    const html = await injectStoreData(page('AppDetail'), DETAIL_PATH, poisoned).text();

    expect(html).not.toContain('</script><script>alert(1)');
    expect(html).toContain('\\u003c/script');
  });

  test('a page without the placeholder still gets its island rendered', async () => {
    const html = await injectStoreData(page('StoreBrowser', ''), STORE_PATH, await data()).text();

    expect(html).toContain('weather');
  });

  test('is cached at the edge but never in the browser, so counts stay fresh', async () => {
    const response = injectStoreData(page('StoreBrowser'), STORE_PATH, await data());

    expect(response.headers.get('cache-control')).toBe('public, max-age=0, s-maxage=60');
  });
});
