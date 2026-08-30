import { aggregate, type AppEntry, type Catalog } from '@bridgething/catalog';

import { DEVICE } from './fixtures';
import { rig, type Rig } from './harness';

const OFFICIAL = 'https://apps.bridgething.com/catalog.json';
const DIRECTORY = 'https://bridgething.com/api/sources.json';
const THIRD_PARTY = 'https://example.test/catalog.json';

function app(
  id: string,
  name: string,
  versions: string[] = ['1.2.0'],
): AppEntry {
  return {
    id,
    name,
    description: `${name} does a thing`,
    author: 'someone',
    icon: null,
    homepage: null,
    source: null,
    versions: versions.map((v, index) => ({
      version: v,
      released_at: `2026-05-${String(31 - index).padStart(2, '0')}T00:00:00Z`,
      download: {
        url: `https://example.test/r/${id}/${v}.zip`,
        size: 4096,
        sha256: 'a'.repeat(64),
      },
      permissions: [],
      min_libbridgething_version: '0.1.0',
      changelog: null,
    })),
  };
}

function catalog(name: string, apps: AppEntry[] = []): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-05-31T00:00:00Z',
    repo: { name, description: name, homepage: null, icon: null },
    apps,
    recommended_sources: [],
  };
}

function serve(served: Record<string, unknown>): jest.Mock {
  const fetchMock = jest.fn((url: string) => {
    const body = served[url];
    if (!body)
      return Promise.resolve({ ok: false, status: 503, json: () => ({}) });
    return Promise.resolve({ ok: true, status: 200, json: () => body });
  });
  globalThis.fetch = fetchMock as unknown as typeof fetch;
  return fetchMock;
}

const sourcesOf = (r: Rig) => r.catalog.useCatalogStore.getState().sources;

describe('reading a source url a user typed', () => {
  test('a bare host resolves to the catalog it implies', () => {
    expect(rig().catalog.parseSourceInput('example.test')).toEqual({
      ok: true,
      url: THIRD_PARTY,
    });
  });

  test('padding and a fragment do not make a second spelling of one source', () => {
    expect(
      rig().catalog.parseSourceInput(
        '  https://example.test/catalog.json#apps ',
      ),
    ).toEqual({ ok: true, url: THIRD_PARTY });
  });

  test('an empty input comes back as a reason the caller can render', () => {
    expect(rig().catalog.parseSourceInput('   ')).toEqual({
      ok: false,
      error: expect.stringContaining('empty'),
    });
  });

  test('a url that cannot be a source explains itself instead of throwing', () => {
    expect(rig().catalog.parseSourceInput('ftp://example.test/c.json')).toEqual(
      {
        ok: false,
        error: expect.stringContaining('http'),
      },
    );
  });
});

describe('catalog sources', () => {
  test('a source that fails does not take the working ones down with it', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('official'), [DIRECTORY]: catalog('dir') });
    await r.catalog.addSource(THIRD_PARTY);

    const state = r.catalog.useCatalogStore.getState();
    expect(state.catalogs.map(c => c.url)).toEqual([OFFICIAL]);
    expect(state.failures.map(f => f.url)).toEqual([THIRD_PARTY]);
    expect(state.refreshing).toBe(false);
  });

  test('the source directory is not offered as a catalog of apps', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('official'), [DIRECTORY]: catalog('dir') });
    await r.catalog.refreshCatalog();

    const state = r.catalog.useCatalogStore.getState();
    expect(state.catalogs.map(c => c.url)).not.toContain(DIRECTORY);
    expect(state.directory).not.toBeNull();
  });

  test('subscriptions survive an app relaunch', async () => {
    const first = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third'),
    });
    await first.catalog.addSource(THIRD_PARTY);

    expect(sourcesOf(first.relaunch())).toContain(THIRD_PARTY);
  });

  test('unsubscribing drops the source and its apps', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third'),
    });
    await r.catalog.addSource(THIRD_PARTY);
    await r.catalog.removeSource(THIRD_PARTY);

    expect(sourcesOf(r)).not.toContain(THIRD_PARTY);
    expect(
      r.catalog.useCatalogStore.getState().catalogs.map(c => c.url),
    ).not.toContain(THIRD_PARTY);
    expect(sourcesOf(r.relaunch())).not.toContain(THIRD_PARTY);
  });

  test('unsubscribing wins over a refresh that was already in flight', async () => {
    const r = rig();
    let releaseThirdParty = () => {};
    const held = new Promise<void>(resolve => {
      releaseThirdParty = resolve;
    });

    globalThis.fetch = jest.fn(async (url: string) => {
      if (url === THIRD_PARTY) await held;
      const body: Record<string, Catalog> = {
        [OFFICIAL]: catalog('official'),
        [DIRECTORY]: catalog('dir'),
        [THIRD_PARTY]: catalog('third'),
      };
      return { ok: true, status: 200, json: () => body[url] };
    }) as unknown as typeof fetch;

    const adding = r.catalog.addSource(THIRD_PARTY);
    await r.catalog.removeSource(THIRD_PARTY);
    releaseThirdParty();
    await adding;

    expect(sourcesOf(r)).not.toContain(THIRD_PARTY);
    expect(
      r.catalog.useCatalogStore.getState().catalogs.map(c => c.url),
    ).not.toContain(THIRD_PARTY);
  });

  test('a corrupt subscription list falls back to the official catalog', () => {
    const r = rig();
    r.storage.storage.set('catalog.sources', 'not json at all');

    expect(sourcesOf(r.relaunch())).toEqual([OFFICIAL]);
  });

  test('an empty subscription list falls back to the official catalog', () => {
    const r = rig();
    r.storage.storage.set('catalog.sources', '[]');

    expect(sourcesOf(r.relaunch())).toEqual([OFFICIAL]);
  });
});

const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';

describe('installing from a catalog you added yourself', () => {
  test('a source you add contributes its apps to the store listing', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third', [app(APP_ID, 'my app')]),
    });
    await r.catalog.addSource(THIRD_PARTY);

    const listings = aggregate({
      orderedCatalogs: r.catalog.useCatalogStore.getState().catalogs,
      installed: [],
      deviceLibVersion: '0.6.0',
      extensions: 'omitted',
    });

    expect(listings.map(l => l.app.name)).toEqual(['my app']);
    expect(listings[0]!.sourceUrl).toBe(THIRD_PARTY);
    expect(listings[0]!.newestCompatible?.version).toBe('1.2.0');
  });

  test('installing that app asks native for its bundle, tagged with the source it came from', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third', [app(APP_ID, 'my app')]),
    });
    await r.catalog.addSource(THIRD_PARTY);

    const calls: unknown[][] = [];
    r.native.__returns.set('installWebappFromUrl', (...args: unknown[]) => {
      calls.push(args);
      return Promise.resolve({});
    });

    const [listing] = aggregate({
      orderedCatalogs: r.catalog.useCatalogStore.getState().catalogs,
      installed: [],
      deviceLibVersion: '0.6.0',
      extensions: 'omitted',
    });
    await r.catalog.installApp(DEVICE, listing!);

    expect(calls).toEqual([
      [
        DEVICE,
        `https://example.test/r/${APP_ID}/1.2.0.zip`,
        'a'.repeat(64),
        4096,
        THIRD_PARTY,
        APP_ID,
        'my app',
      ],
    ]);
  });

  test('a source is normalized once, so two spellings cannot double-subscribe', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      'https://example.test/catalog.json': catalog('third'),
    });

    await r.catalog.addSource('example.test');
    await r.catalog.addSource('  https://example.test/catalog.json#apps ');

    expect(sourcesOf(r)).toEqual([OFFICIAL, THIRD_PARTY]);
  });

  test('a url that cannot be a source is refused rather than subscribed', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('official'), [DIRECTORY]: catalog('dir') });

    await expect(
      r.catalog.addSource('ftp://example.test/c.json'),
    ).rejects.toThrow();
    expect(sourcesOf(r)).toEqual([OFFICIAL]);
  });
});

describe('source priority', () => {
  test('moving a source up decides which one wins an app both offer', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official', [app(APP_ID, 'my app')]),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third', [app(APP_ID, 'my app')]),
    });
    await r.catalog.addSource(THIRD_PARTY);

    const winner = () =>
      aggregate({
        orderedCatalogs: r.catalog.useCatalogStore.getState().catalogs,
        installed: [],
        deviceLibVersion: '0.6.0',
        extensions: 'omitted',
      })[0]!.sourceUrl;

    expect(winner()).toBe(OFFICIAL);

    r.catalog.moveSource(THIRD_PARTY, -1);

    expect(sourcesOf(r)).toEqual([THIRD_PARTY, OFFICIAL]);
    expect(winner()).toBe(THIRD_PARTY);
  });

  test('a reorder survives an app relaunch', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third'),
    });
    await r.catalog.addSource(THIRD_PARTY);
    r.catalog.moveSource(THIRD_PARTY, -1);

    expect(sourcesOf(r.relaunch())).toEqual([THIRD_PARTY, OFFICIAL]);
  });

  test('moving the top source up does nothing', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third'),
    });
    await r.catalog.addSource(THIRD_PARTY);

    r.catalog.moveSource(OFFICIAL, -1);

    expect(sourcesOf(r)).toEqual([OFFICIAL, THIRD_PARTY]);
  });
});

const INSTALLS = 'https://bridgething.com/api/installs';
const MERGED = 'https://bridgething.com/api/apps.json';
const OTHER_APP_ID = '019e6701-13f8-71b5-ba04-81f347137de2';

function mergedApps(
  installs: { app_id: string; source_url: string; count: number }[],
) {
  return {
    updated_at: '2026-05-31T00:00:00Z',
    catalogs: [],
    failures: [],
    skipped: [],
    installs,
  };
}

function beacons(fetchMock: jest.Mock): { url: string; init: RequestInit }[] {
  return fetchMock.mock.calls
    .filter(([url]) => url === INSTALLS)
    .map(([url, init]) => ({ url: url as string, init: init as RequestInit }));
}

describe('reporting an install to the directory', () => {
  async function installed() {
    const r = rig();
    const fetchMock = serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third', [app(APP_ID, 'my app')]),
    });
    await r.catalog.addSource(THIRD_PARTY);

    r.native.__returns.set('installWebappFromUrl', () => Promise.resolve({}));
    const [listing] = aggregate({
      orderedCatalogs: r.catalog.useCatalogStore.getState().catalogs,
      installed: [],
      deviceLibVersion: '0.6.0',
      extensions: 'omitted',
    });
    await r.catalog.installApp(DEVICE, listing!);
    await new Promise(resolve => setTimeout(resolve, 0));

    return { r, fetchMock };
  }

  test('a store install tells the directory which app and source it was', async () => {
    const { fetchMock } = await installed();

    const sent = beacons(fetchMock);
    expect(sent).toHaveLength(1);
    expect(sent[0]!.init.method).toBe('POST');
    expect(JSON.parse(String(sent[0]!.init.body))).toEqual({
      app_id: APP_ID,
      source_url: THIRD_PARTY,
      version: '1.2.0',
    });
  });

  test('nothing about the phone or the device rides along with it', async () => {
    const { fetchMock } = await installed();

    const body = JSON.parse(String(beacons(fetchMock)[0]!.init.body)) as Record<
      string,
      unknown
    >;
    expect(Object.keys(body).sort()).toEqual([
      'app_id',
      'source_url',
      'version',
    ]);
    expect(JSON.stringify(body)).not.toContain(DEVICE);
  });

  test('a directory that refuses the report does not fail the install', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third', [app(APP_ID, 'my app')]),
    });
    await r.catalog.addSource(THIRD_PARTY);

    r.native.__returns.set('installWebappFromUrl', () => Promise.resolve({}));
    globalThis.fetch = jest.fn((url: string) => {
      if (url === INSTALLS) return Promise.reject(new Error('offline'));
      return Promise.resolve({ ok: false, status: 503, json: () => ({}) });
    }) as unknown as typeof fetch;

    const [listing] = aggregate({
      orderedCatalogs: r.catalog.useCatalogStore.getState().catalogs,
      installed: [],
      deviceLibVersion: '0.6.0',
      extensions: 'omitted',
    });

    await expect(
      r.catalog.installApp(DEVICE, listing!),
    ).resolves.toBeUndefined();
    await new Promise(resolve => setTimeout(resolve, 0));
  });

  test('an install the device refuses is never reported as one', async () => {
    const r = rig();
    const fetchMock = serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third', [app(APP_ID, 'my app')]),
    });
    await r.catalog.addSource(THIRD_PARTY);

    r.native.__returns.set('installWebappFromUrl', () =>
      Promise.reject(new Error('the bundle hash did not match')),
    );
    const [listing] = aggregate({
      orderedCatalogs: r.catalog.useCatalogStore.getState().catalogs,
      installed: [],
      deviceLibVersion: '0.6.0',
      extensions: 'omitted',
    });

    await expect(r.catalog.installApp(DEVICE, listing!)).rejects.toThrow();
    await new Promise(resolve => setTimeout(resolve, 0));

    expect(beacons(fetchMock)).toHaveLength(0);
  });
});

describe('installing a version other than the newest', () => {
  async function listed(versions: string[]) {
    const r = rig();
    const fetchMock = serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [THIRD_PARTY]: catalog('third', [app(APP_ID, 'my app', versions)]),
    });
    await r.catalog.addSource(THIRD_PARTY);

    const calls: unknown[][] = [];
    r.native.__returns.set('installWebappFromUrl', (...args: unknown[]) => {
      calls.push(args);
      return Promise.resolve({});
    });

    const [listing] = aggregate({
      orderedCatalogs: r.catalog.useCatalogStore.getState().catalogs,
      installed: [],
      deviceLibVersion: '0.6.0',
      extensions: 'omitted',
    });

    return { r, fetchMock, calls, listing: listing! };
  }

  test('the picked version decides which bundle the device is handed', async () => {
    const { r, calls, listing } = await listed(['2.0.0', '1.2.0']);
    const older = listing.app.versions.find(v => v.version === '1.2.0')!;

    await r.catalog.installApp(DEVICE, listing, older);

    expect(calls).toEqual([
      [
        DEVICE,
        `https://example.test/r/${APP_ID}/1.2.0.zip`,
        'a'.repeat(64),
        4096,
        THIRD_PARTY,
        APP_ID,
        'my app',
      ],
    ]);
  });

  test('picking nothing still installs the newest compatible build', async () => {
    const { r, calls, listing } = await listed(['2.0.0', '1.2.0']);

    await r.catalog.installApp(DEVICE, listing);

    expect(calls[0]?.[1]).toBe(`https://example.test/r/${APP_ID}/2.0.0.zip`);
  });

  test('the directory hears the version that was actually installed', async () => {
    const { r, fetchMock, listing } = await listed(['2.0.0', '1.2.0']);
    const older = listing.app.versions.find(v => v.version === '1.2.0')!;

    await r.catalog.installApp(DEVICE, listing, older);
    await new Promise(resolve => setTimeout(resolve, 0));

    expect(JSON.parse(String(beacons(fetchMock)[0]!.init.body))).toMatchObject({
      version: '1.2.0',
    });
  });
});

describe('popularity from the directory', () => {
  test('counts land in the store and order the listing', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official', [
        app(APP_ID, 'aardvark'),
        app(OTHER_APP_ID, 'zebra'),
      ]),
      [DIRECTORY]: catalog('dir'),
      [MERGED]: mergedApps([
        { app_id: OTHER_APP_ID, source_url: OFFICIAL, count: 11 },
      ]),
    });
    await r.catalog.refreshCatalog();

    const state = r.catalog.useCatalogStore.getState();
    expect(state.installs).toEqual([
      { app_id: OTHER_APP_ID, source_url: OFFICIAL, count: 11 },
    ]);

    const listings = aggregate({
      orderedCatalogs: state.catalogs,
      installed: [],
      deviceLibVersion: '0.6.0',
      extensions: 'omitted',
      installs: state.installs,
    });
    expect(listings.map(l => l.app.name)).toEqual(['zebra', 'aardvark']);
  });

  test('a directory that is down leaves the counts it last knew in place', async () => {
    const r = rig();
    serve({
      [OFFICIAL]: catalog('official'),
      [DIRECTORY]: catalog('dir'),
      [MERGED]: mergedApps([
        { app_id: APP_ID, source_url: OFFICIAL, count: 4 },
      ]),
    });
    await r.catalog.refreshCatalog();

    serve({ [OFFICIAL]: catalog('official'), [DIRECTORY]: catalog('dir') });
    await r.catalog.refreshCatalog();

    expect(r.catalog.useCatalogStore.getState().installs).toEqual([
      { app_id: APP_ID, source_url: OFFICIAL, count: 4 },
    ]);
  });
});
