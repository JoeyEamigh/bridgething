import { SOURCE_STATUSES, type SourceStatus } from '@bridgething/catalog';
import { shellFor, storeRoute } from '../src/lib/app-routes.ts';
import { mergedApps, storeData } from './apps.ts';
import { isAdmin } from './auth.ts';
import { refreshCatalogs } from './catalogs.ts';
import { toCatalogDocument, toDirectoryView } from './directory.ts';
import type { Env } from './env.ts';
import { recordInstalled } from './installs.ts';
import {
  INVENTORY_CHANGES,
  NEW_DEVICES,
  NEW_SOURCES,
  pruneTokens,
  SOURCE_REFRESHES,
  takeToken,
  type Limit,
} from './limits.ts';
import { OAUTH_CALLBACK_PATH, oauthBounce } from './oauth.ts';
import { featuredPin, pinFeatured, unpinFeatured } from './pins.ts';
import { injectStoreData } from './prerender.tsx';
import { relayCatalog } from './relay.ts';
import { listSources, setSourceStatus, submitSource } from './sources.ts';

const PUBLIC_CACHE = 'public, max-age=60';

const CORS_HEADERS: Record<string, string> = {
  'access-control-allow-origin': '*',
  'access-control-allow-methods': 'GET, POST, PATCH, OPTIONS',
  'access-control-allow-headers': 'content-type, authorization',
  'access-control-max-age': '86400',
};

const CACHED_ROUTES = ['/api/sources.json', '/api/directory.json', '/api/apps.json', '/api/catalog'];

function jsonText(body: string, init: { status?: number; cache?: string } = {}): Response {
  const headers: Record<string, string> = {
    'content-type': 'application/json; charset=utf-8',
    ...CORS_HEADERS,
  };
  if (init.cache) headers['cache-control'] = init.cache;
  return new Response(body, { status: init.status ?? 200, headers });
}

function json(body: unknown, init: { status?: number; cache?: string } = {}): Response {
  return jsonText(JSON.stringify(body), init);
}

function fail(status: number, reason: string): Response {
  return json({ error: reason }, { status });
}

type EdgeCache = {
  match(request: Request): Promise<Response | undefined>;
  put(request: Request, response: Response): Promise<void>;
};

const edgeCache = (caches as unknown as { default: EdgeCache }).default;

async function readJsonBody(request: Request): Promise<Record<string, unknown> | null> {
  try {
    const body = await request.json();
    return body !== null && typeof body === 'object' ? (body as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

function clientOf(request: Request): string {
  return request.headers.get('cf-connecting-ip') ?? 'unknown';
}

async function gate(db: D1Database, limit: Limit, key: string, now: string) {
  return (await takeToken(db, limit, key, now)) ? null : { ok: false as const, status: 429, reason: limit.refusal };
}

async function handleFeatured(request: Request, env: Env, now: string): Promise<Response | null> {
  if (request.method === 'GET') return json({ pin: await featuredPin(env.DB) }, { cache: 'no-store' });

  const outcome =
    request.method === 'PUT'
      ? await pinFeatured({ db: env.DB, body: await readJsonBody(request), now })
      : request.method === 'DELETE'
        ? await unpinFeatured(env.DB)
        : null;
  if (outcome === null) return null;
  if (!outcome.ok) return fail(outcome.status, outcome.reason);
  return json({ pin: outcome.pin }, { cache: 'no-store' });
}

async function handleAdmin(request: Request, env: Env, url: URL, now: string): Promise<Response | null> {
  if (url.pathname !== '/api/admin/sources' && url.pathname !== '/api/admin/featured') return null;
  if (!isAdmin(request, env.ADMIN_TOKEN)) return fail(401, 'admin token required');

  if (url.pathname === '/api/admin/featured') return handleFeatured(request, env, now);

  if (request.method === 'GET') {
    return json({ sources: await listSources(env.DB) }, { cache: 'no-store' });
  }

  if (request.method === 'PATCH') {
    const body = await readJsonBody(request);
    const raw = body?.['url'];
    const status = body?.['status'];
    const note = body?.['note'];

    if (typeof raw !== 'string' || !raw.trim()) return fail(400, 'send a json body with a "url" string');
    if (typeof status !== 'string' || !SOURCE_STATUSES.includes(status as SourceStatus)) {
      return fail(400, `"status" must be one of ${SOURCE_STATUSES.join(', ')}`);
    }
    if (note !== undefined && note !== null && typeof note !== 'string') {
      return fail(400, '"note" must be a string or null');
    }

    const outcome = await setSourceStatus({
      db: env.DB,
      rawUrl: raw,
      status: status as SourceStatus,
      reviewedBy: 'admin',
      note: note as string | null | undefined,
      now,
    });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return json({ source: outcome.record }, { cache: 'no-store' });
  }

  return null;
}

async function handleApi(request: Request, env: Env, url: URL): Promise<Response> {
  const db = env.DB;
  const now = new Date().toISOString();

  if (request.method === 'OPTIONS') return new Response(null, { status: 204, headers: CORS_HEADERS });

  if (url.pathname === '/api/sources.json' && request.method === 'GET') {
    return json(toCatalogDocument(await listSources(db), now), { cache: PUBLIC_CACHE });
  }

  if (url.pathname === '/api/directory.json' && request.method === 'GET') {
    return json({ updated_at: now, sources: toDirectoryView(await listSources(db)) }, { cache: PUBLIC_CACHE });
  }

  if (url.pathname === '/api/apps.json' && request.method === 'GET') {
    return jsonText(await mergedApps({ db, now }), { cache: PUBLIC_CACHE });
  }

  if (url.pathname === '/api/catalog' && request.method === 'GET') {
    const outcome = await relayCatalog({ db, rawUrl: url.searchParams.get('url') });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return jsonText(outcome.body, { cache: PUBLIC_CACHE });
  }

  if (url.pathname === '/api/sources' && request.method === 'POST') {
    const body = await readJsonBody(request);
    const raw = body?.['url'];
    if (typeof raw !== 'string' || !raw.trim()) return fail(400, 'send a json body with a "url" string');

    const outcome = await submitSource({
      db,
      rawUrl: raw,
      now,
      admit: (existing, normalized) =>
        existing === null ? gate(db, NEW_SOURCES, clientOf(request), now) : gate(db, SOURCE_REFRESHES, normalized, now),
    });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return json({ source: outcome.record }, { status: outcome.created ? 201 : 200 });
  }

  if (url.pathname === '/api/installed' && request.method === 'POST') {
    const outcome = await recordInstalled({
      db,
      body: await readJsonBody(request),
      now,
      admit: change =>
        change.known ? gate(db, INVENTORY_CHANGES, change.serial, now) : gate(db, NEW_DEVICES, clientOf(request), now),
    });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return json({ apps: outcome.record.apps.length }, { status: 202 });
  }

  if (url.pathname.startsWith('/api/admin/')) {
    const handled = await handleAdmin(request, env, url, now);
    if (handled !== null) return handled;
  }

  return fail(404, `no api route for ${request.method} ${url.pathname}`);
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(request.url);

    if (url.pathname === OAUTH_CALLBACK_PATH && (request.method === 'GET' || request.method === 'HEAD')) {
      const bounce = oauthBounce(url);
      return request.method === 'HEAD' ? new Response(null, bounce) : bounce;
    }

    if (url.pathname !== '/api' && !url.pathname.startsWith('/api/')) {
      const direct = await env.ASSETS.fetch(request);
      const shell = direct.status === 404 ? shellFor(url.pathname) : null;
      const asset = shell ? await env.ASSETS.fetch(new Request(new URL(shell, url.origin), request)) : direct;

      const route = storeRoute(url.pathname, url.search);
      if (!asset.ok || request.method !== 'GET' || route === null) return asset;
      const data = await storeData({ db: env.DB, now: new Date().toISOString() });
      return injectStoreData(asset, route, `${url.pathname}${url.search}`, data);
    }

    const head = request.method === 'HEAD';
    const effective = head ? new Request(request.url, { method: 'GET', headers: request.headers }) : request;

    const cacheable = effective.method === 'GET' && CACHED_ROUTES.includes(url.pathname);
    if (cacheable) {
      const hit = await edgeCache.match(effective);
      if (hit) return head ? new Response(null, hit) : hit;
    }

    const response = await handleApi(effective, env, url);
    if (cacheable && response.ok) {
      ctx.waitUntil(edgeCache.put(effective, response.clone()));
    }
    return head ? new Response(null, response) : response;
  },

  async scheduled(_event: ScheduledController, env: Env): Promise<void> {
    const now = new Date().toISOString();
    await refreshCatalogs({ db: env.DB, now });
    await pruneTokens(env.DB, now);
  },
};
