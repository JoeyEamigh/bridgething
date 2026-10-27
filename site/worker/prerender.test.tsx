import { afterEach, describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL, type AppEntry } from '@bridgething/catalog';
import type { VNode } from 'preact';
import { renderToString } from 'preact-render-to-string';
import { AppDetail } from '../src/components/store/AppDetail.tsx';
import { SourcesPage } from '../src/components/store/SourcesPage.tsx';
import { StoreBrowse } from '../src/components/store/StoreBrowse.tsx';
import { Storefront } from '../src/components/store/Storefront.tsx';
import { storeRoute, type StoreRoute } from '../src/lib/app-routes.ts';
import { storeData } from './apps.ts';
import { fakeD1 } from './d1-fake.ts';
import { admitAll, app, APP_ID, catalog, NOW, seedOfficial, seedSource, serialOf } from './fixtures.ts';
import { recordInstalled } from './installs.ts';
import { pinFeatured } from './pins.ts';
import { injectStoreData, STORE_DATA_ID } from './prerender.tsx';

const LISTED = 'https://listed.example/catalog.json';
const WEATHER_ID = '019e6701-13f8-71b5-ba04-85d326630e99';
const LAUNCHER_ID = '019e6701-13f8-71b5-ba04-85d326630e9a';
const OVERLAY_ID = '019e6701-13f8-71b5-ba04-85d326630e9b';

function version(overrides: Partial<AppEntry['versions'][number]> = {}): AppEntry['versions'][number] {
  return { ...app(APP_ID, 'x').versions[0]!, ...overrides };
}

const CALENDAR = app(APP_ID, 'calendar', { categories: ['productivity'], author: 'JoeyEamigh' });
const WEATHER = app(WEATHER_ID, 'weather', {
  categories: ['clocks-weather'],
  author: 'Ousa Chea',
  screenshots: ['https://listed.example/weather.png'],
  versions: [
    version({
      version: '1.1.0',
      released_at: '2026-07-20T00:00:00Z',
      changelog: 'fresher forecasts',
      permissions: ['net.fetch', 'geo'],
    }),
    version(),
  ],
});
const LAUNCHER = app(LAUNCHER_ID, 'metro', {
  author: 'Ousa Chea',
  screenshots: ['https://listed.example/metro.png'],
  versions: [version({ role: 'launcher' })],
});
const OVERLAY = app(OVERLAY_ID, 'glassy', { author: 'someone else', versions: [version({ provides_overlay: true })] });

async function data(): Promise<string> {
  const db = fakeD1();
  await seedOfficial(db, catalog('bridgething apps', [CALENDAR]));
  await seedSource(db, LISTED, 'listed', catalog('listed', [WEATHER, LAUNCHER, OVERLAY]));
  for (const [n, ids] of [
    [1, [APP_ID, WEATHER_ID]],
    [2, [WEATHER_ID, LAUNCHER_ID]],
    [3, [WEATHER_ID]],
  ] as const) {
    await recordInstalled({
      db,
      body: {
        device_id: serialOf(n),
        apps: ids.map(id => ({
          app_id: id,
          source_url: id === APP_ID ? OFFICIAL_CATALOG_URL : LISTED,
          version: '1.0.0',
        })),
      },
      now: NOW,
      admit: admitAll,
    });
  }
  await pinFeatured({ db, body: { app_id: LAUNCHER_ID, source_url: LISTED }, now: NOW });
  return storeData({ db, now: NOW });
}

const ISLANDS: Record<StoreRoute['page'], string> = {
  front: 'Storefront',
  browse: 'StoreBrowse',
  app: 'AppDetail',
  sources: 'SourcesPage',
};

function page(island: string, placeholder = `<script id="${STORE_DATA_ID}" type="application/json">null</script>`) {
  return new Response(
    `<!doctype html><html><body>${placeholder}<astro-island component-export="${island}" ssr>` +
      `<p>built without the directory</p><!--astro:end--></astro-island></body></html>`,
    { status: 200, headers: { 'content-type': 'text/html' } },
  );
}

const originals = {
  document: (globalThis as { document?: unknown }).document,
  window: (globalThis as { window?: unknown }).window,
};

afterEach(() => {
  (globalThis as { document?: unknown }).document = originals.document;
  (globalThis as { window?: unknown }).window = originals.window;
});

function asTheBrowserSeesIt(injected: string, href: string): void {
  const url = new URL(href, 'https://bridgething.com');
  (globalThis as { document?: unknown }).document = {
    getElementById: (id: string) => (id === STORE_DATA_ID ? { textContent: injected } : null),
  };
  (globalThis as { window?: unknown }).window = { location: { pathname: url.pathname, search: url.search } };
}

function islandOf(html: string, name: string): string {
  return html.split(`component-export="${name}" ssr>`)[1]!.split('</astro-island>')[0]!;
}

function routeOf(href: string): StoreRoute {
  const url = new URL(href, 'https://bridgething.com');
  return storeRoute(url.pathname, url.search)!;
}

async function served(href: string, injected: string, placeholder?: string): Promise<string> {
  const route = routeOf(href);
  const html = await injectStoreData(page(ISLANDS[route.page], placeholder), route, href, injected).text();
  return islandOf(html, ISLANDS[route.page]);
}

const FIRST_RENDER: Record<StoreRoute['page'], () => VNode> = {
  front: () => <Storefront />,
  browse: () => <StoreBrowse />,
  app: () => <AppDetail baked={null} />,
  sources: () => <SourcesPage />,
};

describe('injectStoreData', () => {
  test('fills the placeholder so the island hydrates from the same data the page was rendered from', async () => {
    const route = routeOf('/apps/');
    const html = await injectStoreData(page('Storefront'), route, '/apps/', await data()).text();
    const payload = JSON.parse(html.split(`type="application/json">`)[1]!.split('</script>')[0]!) as {
      apps: { catalogs: { url: string }[]; installs: { count: number }[]; featured: { app_id: string } | null };
      directory: { url: string }[];
    };

    expect(payload.apps.catalogs.map(entry => entry.url)).toEqual([OFFICIAL_CATALOG_URL, LISTED]);
    expect(payload.apps.featured?.app_id).toBe(LAUNCHER_ID);
    expect(payload.directory.map(entry => entry.url)).toEqual([LISTED]);
  });

  const ROUTES = [
    '/apps/',
    '/apps/popular',
    '/apps/new',
    '/apps/updated',
    '/apps/trending',
    '/apps/c/clocks-weather',
    '/apps/c/productivity?sort=new',
    '/apps/home-screens',
    '/apps/overlays?sort=updated',
    '/apps/by/Ousa%20Chea',
    '/apps/search?q=weather',
    '/apps/search?q=ousa&sort=new',
    '/apps/search',
    `/apps/${WEATHER_ID}`,
    `/apps/${APP_ID}`,
    '/apps/sources',
  ];

  for (const href of ROUTES) {
    test(`${href} renders exactly what the browser renders first from the injected data`, async () => {
      const injected = await data();
      const island = await served(href, injected);

      expect(island).not.toContain('built without the directory');
      expect(island.endsWith('<!--astro:end-->')).toBe(true);
      asTheBrowserSeesIt(injected, href);
      expect(island).toBe(`${renderToString(FIRST_RENDER[routeOf(href).page]())}<!--astro:end-->`);
    });
  }

  test('the store front leads with the pinned app and fills its shelves', async () => {
    const island = await served('/apps/', await data());

    expect(island.indexOf('featured')).toBeLessThan(island.indexOf('>metro<'));
    expect(island).toContain('https://listed.example/metro.png');
    expect(island).toContain('popular');
    expect(island).toContain('href="/apps/popular"');
    expect(island).toContain('href="/apps/c/clocks-weather"');
    expect(island).toContain('href="/apps/sources"');
  });

  test('a category page lists only that category', async () => {
    const island = await served('/apps/c/clocks-weather', await data());

    expect(island).toContain('weather');
    expect(island).not.toContain('>calendar<');
    expect(island).toMatch(/href="\/apps\/c\/clocks-weather" aria-current="page"/);
  });

  test('an author page shows every app by that author and names them as written', async () => {
    const island = await served('/apps/by/ousa%20chea', await data());

    expect(island).toContain('Ousa Chea');
    expect(island).toContain('>weather<');
    expect(island).toContain('>metro<');
    expect(island).not.toContain('>glassy<');
  });

  test('search answers the query from the url', async () => {
    const island = await served('/apps/search?q=clocks', await data());

    expect(island).toContain('value="clocks"');
    expect(island).toContain('>weather<');
    expect(island).toContain('1 app');
  });

  test('an app page carries its permissions, its notes and more from its author', async () => {
    const island = await served(`/apps/${WEATHER_ID}`, await data());

    expect(island).toContain('new in 1.1.0');
    expect(island).toContain('fresher forecasts');
    expect(island).toContain('internet');
    expect(island).toContain('location');
    expect(island).toContain('more from Ousa Chea');
    expect(island).toContain('>metro<');
    expect(island).toContain('not reviewed by bridgething');
  });

  test('an app page renders apps the build never knew about', async () => {
    const island = await served(`/apps/${OVERLAY_ID}`, await data());

    expect(island).toContain('glassy');
  });

  test('the sources page lists the directory and the submit form', async () => {
    const island = await served('/apps/sources', await data());

    expect(island).toContain('submit a source');
    expect(island).toContain(LISTED);
  });

  test('the app page ignores what was baked in when the directory has spoken', async () => {
    const injected = await data();
    const href = `/apps/${APP_ID}`;
    asTheBrowserSeesIt(injected, href);
    const stale = {
      app: app(APP_ID, 'stale build'),
      source: { url: OFFICIAL_CATALOG_URL, name: 'x', icon: null, trust: 'official' as const },
    };

    expect(renderToString(<AppDetail baked={stale} />)).toBe(renderToString(<AppDetail baked={null} />));
  });

  test('escapes a closing tag so a catalog cannot break out of the script', async () => {
    const poisoned = JSON.stringify({
      apps: { updated_at: NOW, catalogs: [], failures: [], installs: [], trending: [], featured: null },
      directory: [{ name: '</script><script>alert(1)</script>' }],
    });

    const route = routeOf(`/apps/${APP_ID}`);
    const html = await injectStoreData(page('AppDetail'), route, `/apps/${APP_ID}`, poisoned).text();

    expect(html).not.toContain('</script><script>alert(1)');
    expect(html).toContain('\\u003c/script');
  });

  test('a page without the placeholder still gets its island rendered', async () => {
    expect(await served('/apps/', await data(), '')).toContain('weather');
  });

  test('is cached at the edge but never in the browser, so counts stay fresh', async () => {
    const response = injectStoreData(page('Storefront'), routeOf('/apps/'), '/apps/', await data());

    expect(response.headers.get('cache-control')).toBe('public, max-age=0, s-maxage=60');
  });
});
