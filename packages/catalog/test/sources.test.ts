import { describe, expect, test } from 'bun:test';
import {
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  normalizeSourceUrl,
  OFFICIAL_CATALOG_URL,
  parseSourceUrl,
  reportInstall,
  SOURCE_DIRECTORY_URL,
  SourceUrlError,
} from '../src/sources.ts';
import type { Catalog } from '../src/types.ts';

const THIRD_PARTY = 'https://third.example.com/catalog.json';
const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';

function catalog(name: string): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-08-09T00:00:00Z',
    repo: { name, description: name, homepage: null, icon: null },
    apps: [],
    recommended_sources: [],
  };
}

function serve(served: Record<string, Catalog | number>): typeof fetch {
  return ((url: string) => {
    const body = served[url];
    if (body === undefined) return Promise.reject(new Error(`nothing is listening on ${url}`));
    if (typeof body === 'number') return Promise.resolve({ ok: false, status: body });
    return Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve(body) });
  }) as unknown as typeof fetch;
}

function withFetch<T>(impl: typeof fetch, run: () => Promise<T>): Promise<T> {
  const original = globalThis.fetch;
  globalThis.fetch = impl;
  return run().finally(() => {
    globalThis.fetch = original;
  });
}

describe('normalizeSourceUrl', () => {
  test('a url that is already canonical is left alone', () => {
    expect(normalizeSourceUrl(THIRD_PARTY)).toBe(THIRD_PARTY);
  });

  test('surrounding whitespace from a paste is trimmed', () => {
    expect(normalizeSourceUrl(`  ${THIRD_PARTY}\n`)).toBe(THIRD_PARTY);
  });

  test('a bare domain becomes an https catalog url', () => {
    expect(normalizeSourceUrl('third.example.com')).toBe('https://third.example.com/catalog.json');
  });

  test('a trailing slash resolves to catalog.json rather than a directory', () => {
    expect(normalizeSourceUrl('https://third.example.com/apps/')).toBe('https://third.example.com/apps/catalog.json');
  });

  test('a path with no trailing slash is taken as the manifest itself', () => {
    expect(normalizeSourceUrl('https://third.example.com/apps.json')).toBe('https://third.example.com/apps.json');
  });

  test('a fragment is dropped so two spellings of one url cannot double-subscribe', () => {
    expect(normalizeSourceUrl(`${THIRD_PARTY}#apps`)).toBe(THIRD_PARTY);
  });

  test('a host:port with no scheme keeps its port instead of reading the host as a scheme', () => {
    expect(normalizeSourceUrl('localhost:8080/catalog.json')).toBe('https://localhost:8080/catalog.json');
  });

  test('http is allowed, because a catalog served on the lan is a real case', () => {
    expect(normalizeSourceUrl('http://192.168.1.4:9000/catalog.json')).toBe('http://192.168.1.4:9000/catalog.json');
  });

  test('a non-web scheme is refused by name', () => {
    expect(() => normalizeSourceUrl('ftp://third.example.com/catalog.json')).toThrow(SourceUrlError);
    expect(() => normalizeSourceUrl('ftp://third.example.com/catalog.json')).toThrow(/ftp/);
  });

  test('credentials in the url are refused', () => {
    expect(() => normalizeSourceUrl('https://user:pw@third.example.com/catalog.json')).toThrow(SourceUrlError);
  });

  test('an empty url is refused', () => {
    expect(() => normalizeSourceUrl('   ')).toThrow(SourceUrlError);
  });

  test('something that cannot be a url at all is refused', () => {
    expect(() => normalizeSourceUrl('not a url')).toThrow(SourceUrlError);
  });
});

describe('parseSourceUrl', () => {
  test('a scheme is required, never inferred, so a stricter caller can rely on that', () => {
    expect(() => parseSourceUrl('third.example.com')).toThrow(SourceUrlError);
  });

  test('a trailing slash is left alone, so it stays the url the caller passed in', () => {
    expect(parseSourceUrl('https://third.example.com/apps/').toString()).toBe('https://third.example.com/apps/');
  });

  test('the shared hygiene still applies: fragment dropped, credentials and odd schemes refused', () => {
    expect(parseSourceUrl(`${THIRD_PARTY}#apps`).toString()).toBe(THIRD_PARTY);
    expect(() => parseSourceUrl('https://user:pw@third.example.com/catalog.json')).toThrow(SourceUrlError);
    expect(() => parseSourceUrl('ftp://third.example.com/catalog.json')).toThrow(SourceUrlError);
  });
});

describe('fetchCatalog', () => {
  test('a source with no caller signal still gets one, so a stall cannot wait forever', async () => {
    let handed: AbortSignal | null | undefined;
    await withFetch(
      ((_url: string, init?: RequestInit) => {
        handed = init?.signal;
        return Promise.resolve(new Response(JSON.stringify(catalog('third'))));
      }) as typeof fetch,
      () => fetchCatalog(THIRD_PARTY),
    );

    expect(handed).toBeInstanceOf(AbortSignal);
  });

  test('a caller supplied signal is used instead of the default deadline', async () => {
    const mine = new AbortController();
    let handed: AbortSignal | null | undefined;
    await withFetch(
      ((_url: string, init?: RequestInit) => {
        handed = init?.signal;
        return Promise.resolve(new Response(JSON.stringify(catalog('third'))));
      }) as typeof fetch,
      () => fetchCatalog(THIRD_PARTY, { signal: mine.signal }),
    );

    expect(handed).toBe(mine.signal);
  });

  test('a served catalog comes back validated', async () => {
    const fetched = await withFetch(serve({ [THIRD_PARTY]: catalog('third') }), () => fetchCatalog(THIRD_PARTY));

    expect(fetched.repo.name).toBe('third');
  });

  test('an http status is surfaced with the url that produced it', async () => {
    await withFetch(serve({ [THIRD_PARTY]: 404 }), async () => {
      await expect(fetchCatalog(THIRD_PARTY)).rejects.toThrow(/404/);
    });
  });

  test('a body that is not a catalog.v1 is refused rather than returned', async () => {
    const notACatalog = { schema: 'something.else' } as unknown as Catalog;
    await withFetch(serve({ [THIRD_PARTY]: notACatalog }), async () => {
      await expect(fetchCatalog(THIRD_PARTY)).rejects.toThrow();
    });
  });
});

describe('fetchSources', () => {
  test('the directory is kept apart from the catalogs of apps', async () => {
    const snapshot = await withFetch(
      serve({
        [OFFICIAL_CATALOG_URL]: catalog('official'),
        [SOURCE_DIRECTORY_URL]: catalog('directory'),
      }),
      () => fetchSources([OFFICIAL_CATALOG_URL]),
    );

    expect(snapshot.catalogs.map(c => c.url)).toEqual([OFFICIAL_CATALOG_URL]);
    expect(snapshot.directory?.repo.name).toBe('directory');
  });

  test('subscription order is preserved, because it decides which source wins a duplicate app', async () => {
    const snapshot = await withFetch(
      serve({
        [OFFICIAL_CATALOG_URL]: catalog('official'),
        [THIRD_PARTY]: catalog('third'),
        [SOURCE_DIRECTORY_URL]: catalog('directory'),
      }),
      () => fetchSources([THIRD_PARTY, OFFICIAL_CATALOG_URL]),
    );

    expect(snapshot.catalogs.map(c => c.url)).toEqual([THIRD_PARTY, OFFICIAL_CATALOG_URL]);
  });

  test('one dead source does not take the working ones down with it', async () => {
    const snapshot = await withFetch(
      serve({ [OFFICIAL_CATALOG_URL]: catalog('official'), [SOURCE_DIRECTORY_URL]: catalog('directory') }),
      () => fetchSources([OFFICIAL_CATALOG_URL, THIRD_PARTY]),
    );

    expect(snapshot.catalogs.map(c => c.url)).toEqual([OFFICIAL_CATALOG_URL]);
    expect(snapshot.failures.map(f => f.url)).toEqual([THIRD_PARTY]);
  });

  test('a directory that is down leaves the catalogs intact', async () => {
    const snapshot = await withFetch(serve({ [OFFICIAL_CATALOG_URL]: catalog('official') }), () =>
      fetchSources([OFFICIAL_CATALOG_URL]),
    );

    expect(snapshot.directory).toBeNull();
    expect(snapshot.catalogs.map(c => c.url)).toEqual([OFFICIAL_CATALOG_URL]);
    expect(snapshot.failures.map(f => f.url)).toEqual([SOURCE_DIRECTORY_URL]);
  });
});

function flush(): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, 0));
}

type Beacon = { url: string; init: RequestInit };

function collect(
  beacons: Beacon[],
  respond: () => Promise<unknown> = () => Promise.resolve({ ok: true }),
): typeof fetch {
  return ((url: string, init: RequestInit) => {
    beacons.push({ url, init });
    return respond();
  }) as unknown as typeof fetch;
}

describe('fetchMergedApps', () => {
  function merged(body: unknown): typeof fetch {
    return (() =>
      Promise.resolve({ ok: true, status: 200, json: () => Promise.resolve(body) })) as unknown as typeof fetch;
  }

  test('install counts come back as the directory reported them', async () => {
    const counts = [{ app_id: APP_ID, source_url: THIRD_PARTY, count: 7 }];
    const apps = await withFetch(
      merged({ updated_at: 'now', catalogs: [], failures: [], skipped: [], installs: counts }),
      () => fetchMergedApps(),
    );

    expect(apps.installs).toEqual(counts);
  });

  test('a directory that reports no counts reads as no counts, not as a broken response', async () => {
    const apps = await withFetch(merged({ updated_at: 'now', catalogs: [], failures: [], skipped: [] }), () =>
      fetchMergedApps(),
    );

    expect(apps.installs).toEqual([]);
  });

  test('a counts field of the wrong shape is discarded rather than passed to the store', async () => {
    const apps = await withFetch(
      merged({ updated_at: 'now', catalogs: [], failures: [], skipped: [], installs: 'lots' }),
      () => fetchMergedApps(),
    );

    expect(apps.installs).toEqual([]);
  });
});

describe('reportInstall', () => {
  test('posts the app and the source it came from', async () => {
    const beacons: Beacon[] = [];

    await withFetch(collect(beacons), async () => {
      reportInstall({ appId: APP_ID, sourceUrl: THIRD_PARTY, version: '1.2.0' });
      await flush();
    });

    expect(beacons).toHaveLength(1);
    expect(beacons[0]!.url).toBe('https://bridgething.com/api/installs');
    expect(beacons[0]!.init.method).toBe('POST');
  });

  test('carries nothing beyond the app, its source, and its version', async () => {
    const beacons: Beacon[] = [];

    await withFetch(collect(beacons), async () => {
      reportInstall({ appId: APP_ID, sourceUrl: THIRD_PARTY, version: '1.2.0' });
      await flush();
    });

    expect(JSON.parse(String(beacons[0]!.init.body))).toEqual({
      app_id: APP_ID,
      source_url: THIRD_PARTY,
      version: '1.2.0',
    });
  });

  test('an install with no version still counts', async () => {
    const beacons: Beacon[] = [];

    await withFetch(collect(beacons), async () => {
      reportInstall({ appId: APP_ID, sourceUrl: THIRD_PARTY });
      await flush();
    });

    expect(JSON.parse(String(beacons[0]!.init.body)).version).toBeNull();
  });

  test('a directory that refuses the beacon never reaches the caller', async () => {
    const beacons: Beacon[] = [];

    await withFetch(
      collect(beacons, () => Promise.reject(new Error('offline'))),
      async () => {
        expect(() => reportInstall({ appId: APP_ID, sourceUrl: THIRD_PARTY })).not.toThrow();
        await flush();
      },
    );

    expect(beacons).toHaveLength(1);
  });

  test('a fetch that throws where it stands is swallowed too', async () => {
    const impl = (() => {
      throw new TypeError('no network stack here');
    }) as unknown as typeof fetch;

    await withFetch(impl, async () => {
      expect(() => reportInstall({ appId: APP_ID, sourceUrl: THIRD_PARTY })).not.toThrow();
      await flush();
    });
  });

  test('the origin can be pointed at a local worker', async () => {
    const beacons: Beacon[] = [];

    await withFetch(collect(beacons), async () => {
      reportInstall({ appId: APP_ID, sourceUrl: THIRD_PARTY }, { origin: 'http://localhost:8787' });
      await flush();
    });

    expect(beacons[0]!.url).toBe('http://localhost:8787/api/installs');
  });
});
