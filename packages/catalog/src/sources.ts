import type { Catalog, InstallCount, SourceCatalog } from './types.ts';
import { validate } from './validate.ts';

export const OFFICIAL_CATALOG_URL = 'https://apps.bridgething.com/catalog.json';

export const SOURCE_DIRECTORY_URL = 'https://bridgething.com/api/sources.json';

export const DIRECTORY_ORIGIN = 'https://bridgething.com';

export type SourceFailure = { url: string; reason: string };

export type CatalogSnapshot = {
  catalogs: SourceCatalog[];
  directory: Catalog | null;
  failures: SourceFailure[];
};

export class SourceUrlError extends Error {}

const HAS_SCHEME = /^[a-z][a-z0-9+.-]*:\/\//i;

export function parseSourceUrl(raw: string): URL {
  const trimmed = raw.trim();
  if (!trimmed) throw new SourceUrlError('a source url cannot be empty');

  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    throw new SourceUrlError(`"${trimmed}" is not a url`);
  }

  if (url.protocol !== 'https:' && url.protocol !== 'http:') {
    throw new SourceUrlError(`a source url must be http or https, not ${url.protocol.replace(':', '')}`);
  }
  if (url.username || url.password) {
    throw new SourceUrlError('a source url must not carry credentials');
  }

  url.hash = '';
  return url;
}

export function normalizeSourceUrl(raw: string): string {
  const trimmed = raw.trim();
  if (!trimmed) throw new SourceUrlError('a source url cannot be empty');

  const url = parseSourceUrl(HAS_SCHEME.test(trimmed) ? trimmed : `https://${trimmed}`);
  if (url.pathname.endsWith('/')) url.pathname += 'catalog.json';
  return url.toString();
}

export const CATALOG_FETCH_TIMEOUT_MS = 15_000;

export async function fetchCatalog(url: string, init?: { signal?: AbortSignal }): Promise<Catalog> {
  const controller = new AbortController();
  const deadline = init?.signal ? null : setTimeout(() => controller.abort(), CATALOG_FETCH_TIMEOUT_MS);

  try {
    const response = await fetch(url, {
      headers: { accept: 'application/json' },
      signal: init?.signal ?? controller.signal,
    });
    if (!response.ok) throw new Error(`${url} returned ${response.status}`);
    return validate(await response.json());
  } finally {
    if (deadline) clearTimeout(deadline);
  }
}

export type MergedCatalog = {
  url: string;
  official: boolean;
  attested: boolean;
  catalog: Catalog;
};

export type MergedApps = {
  updated_at: string;
  catalogs: MergedCatalog[];
  failures: SourceFailure[];
  skipped: string[];
  installs: InstallCount[];
};

export async function fetchMergedApps(init?: { origin?: string; signal?: AbortSignal }): Promise<MergedApps> {
  const origin = init?.origin ?? DIRECTORY_ORIGIN;
  const response = await fetch(`${origin}/api/apps.json`, {
    headers: { accept: 'application/json' },
    signal: init?.signal,
  });
  if (!response.ok) throw new Error(`the app directory returned ${response.status}`);
  const body = (await response.json()) as MergedApps | null;
  if (!body || !Array.isArray(body.catalogs)) {
    throw new Error('the app directory returned an unexpected shape');
  }
  return { ...body, installs: Array.isArray(body.installs) ? body.installs : [] };
}

export type InstallReport = { appId: string; sourceUrl: string; version?: string | null };

export function reportInstall(install: InstallReport, init?: { origin?: string }): void {
  const origin = init?.origin ?? DIRECTORY_ORIGIN;
  const body = JSON.stringify({
    app_id: install.appId,
    source_url: install.sourceUrl,
    version: install.version ?? null,
  });

  void Promise.resolve()
    .then(() =>
      fetch(`${origin}/api/installs`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        keepalive: true,
        body,
      }),
    )
    .catch(() => undefined);
}

export async function fetchSources(urls: string[]): Promise<CatalogSnapshot> {
  const results = await Promise.all(
    [...urls, SOURCE_DIRECTORY_URL].map(async (url): Promise<SourceCatalog | SourceFailure> => {
      try {
        return { url, catalog: await fetchCatalog(url) };
      } catch (reason) {
        return { url, reason: reason instanceof Error ? reason.message : String(reason) };
      }
    }),
  );

  const catalogs: SourceCatalog[] = [];
  const failures: SourceFailure[] = [];
  let directory: Catalog | null = null;

  for (const result of results) {
    if ('reason' in result) failures.push(result);
    else if (result.url === SOURCE_DIRECTORY_URL) directory = result.catalog;
    else catalogs.push(result);
  }

  return { catalogs, directory, failures };
}
