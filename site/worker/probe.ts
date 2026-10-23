import { CatalogValidationError, sortNewestFirst, validate, type Catalog } from '@bridgething/catalog';

export const SITE_ORIGIN = 'https://bridgething.com';
export const MAX_CATALOG_BYTES = 1024 * 1024;
export const PROBE_TIMEOUT_MS = 10_000;
const MAX_REPORTED_ERRORS = 5;

function summarize(err: unknown): string {
  if (!(err instanceof CatalogValidationError)) return String(err);
  const shown = err.errors.slice(0, MAX_REPORTED_ERRORS);
  const extra = err.errors.length - shown.length;
  return shown.join('; ') + (extra > 0 ? ` (and ${extra} more)` : '');
}

function corsPermits(headers: Headers): boolean {
  const allow = headers.get('access-control-allow-origin');
  if (allow === null) return false;
  const value = allow.trim();
  return value === '*' || value === SITE_ORIGIN;
}

async function readCapped(response: Response, maxBytes: number): Promise<string | null> {
  const declared = Number.parseInt(response.headers.get('content-length') ?? '', 10);
  if (!Number.isNaN(declared) && declared > maxBytes) return null;

  const body = response.body;
  if (!body) return null;

  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > maxBytes) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }

  const joined = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    joined.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(joined);
}

async function probeDownloadCors(catalog: Catalog, fetchImpl: typeof fetch): Promise<boolean | null> {
  const first = catalog.apps[0];
  if (!first) return null;
  const newest = sortNewestFirst(first.versions)[0];
  if (!newest) return null;

  try {
    const response = await fetchImpl(newest.download.url, {
      method: 'HEAD',
      redirect: 'follow',
      headers: { origin: SITE_ORIGIN },
      signal: AbortSignal.timeout(PROBE_TIMEOUT_MS),
    });
    if (!response.ok) return false;
    return corsPermits(response.headers);
  } catch {
    return null;
  }
}

export type ParsedCatalog = { ok: true; catalog: Catalog } | { ok: false; reason: string };

export async function parseCatalogBody(response: Response, url: string): Promise<ParsedCatalog> {
  const body = await readCapped(response, MAX_CATALOG_BYTES);
  if (body === null) {
    return { ok: false, reason: `${url} is larger than ${MAX_CATALOG_BYTES} bytes` };
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return { ok: false, reason: `${url} did not return valid json` };
  }

  try {
    return { ok: true, catalog: validate(parsed) };
  } catch (err) {
    return { ok: false, reason: `${url} is not a valid catalog.v1: ${summarize(err)}` };
  }
}

export function corsFailureReason(url: string): string {
  return (
    `${url} does not send "Access-Control-Allow-Origin". a source must serve its catalog and its ` +
    `download urls with "Access-Control-Allow-Origin: *" so a browser can read them.`
  );
}

export type CatalogValidators = { etag: string | null; lastModified: string | null };

export type HeldCatalog = CatalogValidators & { body: string };

export type ProbeResult =
  | ({ ok: true; changed: false } & CatalogValidators)
  | ({ ok: true; changed: true; catalog: Catalog; body: string; downloadsCorsOk: boolean | null } & CatalogValidators)
  | { ok: false; reason: string };

function requestHeaders(held: HeldCatalog | null): Record<string, string> {
  const headers: Record<string, string> = { accept: 'application/json', origin: SITE_ORIGIN };
  if (held?.etag) headers['if-none-match'] = held.etag;
  if (held?.lastModified) headers['if-modified-since'] = held.lastModified;
  return headers;
}

export async function probeCatalog(
  url: string,
  init: { held?: HeldCatalog | null; fetchImpl?: typeof fetch } = {},
): Promise<ProbeResult> {
  const { held = null, fetchImpl = fetch } = init;

  let response: Response;
  try {
    response = await fetchImpl(url, {
      redirect: 'follow',
      headers: requestHeaders(held),
      signal: AbortSignal.timeout(PROBE_TIMEOUT_MS),
    });
  } catch {
    return { ok: false, reason: `could not reach ${url}` };
  }

  const validators: CatalogValidators = {
    etag: response.headers.get('etag') ?? held?.etag ?? null,
    lastModified: response.headers.get('last-modified') ?? held?.lastModified ?? null,
  };

  if (response.status === 304 && held !== null) {
    await response.body?.cancel();
    return { ok: true, changed: false, ...validators };
  }

  if (!response.ok) {
    await response.body?.cancel();
    return { ok: false, reason: `${url} returned ${response.status} ${response.statusText}`.trimEnd() };
  }

  if (!corsPermits(response.headers)) {
    await response.body?.cancel();
    return { ok: false, reason: corsFailureReason(url) };
  }

  const parsed = await parseCatalogBody(response, url);
  if (!parsed.ok) return { ok: false, reason: parsed.reason };

  const body = JSON.stringify(parsed.catalog);
  if (held !== null && held.body === body) return { ok: true, changed: false, ...validators };

  return {
    ok: true,
    changed: true,
    catalog: parsed.catalog,
    body,
    downloadsCorsOk: await probeDownloadCors(parsed.catalog, fetchImpl),
    ...validators,
  };
}
