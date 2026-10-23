import { OFFICIAL_CATALOG_URL, type AppEntry, type Catalog } from '@bridgething/catalog';
import { storeCatalog } from './catalogs.ts';
import type { SourceStatus } from './directory.ts';

export const NOW = '2026-07-24T12:00:00.000Z';
export const LATER = '2026-07-25T12:00:00.000Z';
export const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
export const OTHER_APP_ID = '019e6701-13f8-71b5-ba04-85d326630e99';
export const DEVICE = '8558R481Q61R';

export function serialOf(n: number): string {
  return `8558R${String(n).padStart(3, '0')}Q61R`;
}

export function app(id: string, name: string, overrides: Partial<AppEntry> = {}): AppEntry {
  return {
    id,
    name,
    description: `${name} for bridgething`,
    author: 'somebody',
    icon: null,
    homepage: null,
    source: null,
    versions: [
      {
        version: '1.0.0',
        released_at: '2026-07-01T00:00:00Z',
        download: { url: `https://downloads.example/${name}.zip`, size: 1024, sha256: 'a'.repeat(64) },
        permissions: [],
        min_libbridgething_version: '0.4.0',
        changelog: null,
      },
    ],
    ...overrides,
  };
}

export function catalog(name: string, apps: AppEntry[] = [app(APP_ID, 'thing')]): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-07-01T00:00:00Z',
    repo: { name, description: `apps from ${name}`, homepage: null, icon: null },
    apps,
    recommended_sources: [],
  };
}

export type Served = {
  body?: unknown;
  status?: number;
  cors?: boolean;
  etag?: string;
  throws?: boolean;
};

export type Origin = {
  fetch: typeof fetch;
  serve(url: string, served: Served): void;
  requests: { url: string; method: string; headers: Headers }[];
  downloadsCors: boolean;
};

export function origin(routes: Record<string, Served> = {}): Origin {
  const table = new Map(Object.entries(routes));
  const held: Origin = {
    requests: [],
    downloadsCors: true,
    serve(url, served) {
      table.set(url, served);
    },
    fetch: (async (input: string | URL | Request, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
      const method = init?.method ?? 'GET';
      const headers = new Headers(init?.headers);
      held.requests.push({ url, method, headers });

      if (method === 'HEAD') {
        return new Response(null, {
          status: 200,
          headers: held.downloadsCors ? { 'access-control-allow-origin': '*' } : {},
        });
      }

      const served = table.get(url);
      if (served === undefined) return new Response('not found', { status: 404, statusText: 'Not Found' });
      if (served.throws) throw new TypeError('network down');

      if (served.etag !== undefined && headers.get('if-none-match') === served.etag) {
        return new Response(null, { status: 304, headers: { etag: served.etag } });
      }

      const body = served.body ?? catalog(url);
      return new Response(typeof body === 'string' ? body : JSON.stringify(body), {
        status: served.status ?? 200,
        headers: {
          'content-type': 'application/json',
          ...(served.cors === false ? {} : { 'access-control-allow-origin': '*' }),
          ...(served.etag === undefined ? {} : { etag: served.etag }),
        },
      });
    }) as typeof fetch,
  };
  return held;
}

export async function seedCatalog(db: D1Database, url: string, held: Catalog = catalog(url), at = NOW) {
  await storeCatalog(
    db,
    url,
    { catalog: held, body: JSON.stringify(held), etag: null, lastModified: null, downloadsCorsOk: true },
    at,
  ).run();
}

export async function seedSource(
  db: D1Database,
  url: string,
  status: SourceStatus,
  held: Catalog = catalog(url),
  at = NOW,
) {
  await seedCatalog(db, url, held, at);
  await db.prepare('INSERT INTO sources (url, status, submitted_at) VALUES (?1, ?2, ?3)').bind(url, status, at).run();
}

export async function seedOfficial(db: D1Database, held: Catalog = catalog('bridgething apps')) {
  await seedCatalog(db, OFFICIAL_CATALOG_URL, held);
}

export const admitAll = async () => null;

export async function seedUnreadable(db: D1Database, url: string, status: SourceStatus, error: string, at = NOW) {
  await db.batch([
    db
      .prepare(
        `INSERT INTO catalogs (url, body, name, app_count, checked_at, check_ok, check_error)
         VALUES (?1, NULL, ?1, 0, ?2, 0, ?3)`,
      )
      .bind(url, at, error),
    db.prepare('INSERT INTO sources (url, status, submitted_at) VALUES (?1, ?2, ?3)').bind(url, status, at),
  ]);
}
