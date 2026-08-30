import { APP_DETAIL_SHELL, appIdFromPath } from '../src/lib/app-routes.ts';
import { mergedApps } from './apps.ts';
import { isVisible, SOURCE_STATUSES, toCatalogDocument, toDirectoryView, type SourceStatus } from './directory.ts';
import { kvOf, type Env } from './env.ts';
import { rebuildInstalls, recordInstall } from './installs.ts';
import {
  entryView,
  jamGallery,
  jamReview,
  jamTally,
  parseJamSubmission,
  patchJamEntry,
  putJamScore,
  rebuildEntries,
  submitJamEntry,
} from './jam.ts';
import { OAUTH_CALLBACK_PATH, oauthBounce } from './oauth.ts';
import {
  bearerToken,
  createJudge,
  listJudges,
  principal,
  principalLabel,
  revokeJudge,
  scoringHandle,
  type Principal,
} from './principal.ts';
import { SITE_ORIGIN } from './probe.ts';
import { relayCatalog } from './relay.ts';
import { recheckSource, setSourceStatus, submitSource } from './sources.ts';
import { listSources, rebuildSources, takeRateLimitToken } from './store.ts';

const SUBMIT_LIMIT = 5;
const SUBMIT_WINDOW_SECONDS = 3600;
const INSTALL_LIMIT = 40;
const INSTALL_WINDOW_SECONDS = 3600;
const JAM_LIMIT = 5;
const JAM_WINDOW_SECONDS = 3600;
const JAM_CATALOG_LIMIT = 60;
const JAM_CATALOG_WINDOW_SECONDS = 3600;
const RECHECK_BATCH = 20;

const CORS_HEADERS: Record<string, string> = {
  'access-control-allow-origin': '*',
  'access-control-allow-methods': 'GET, POST, PUT, PATCH, DELETE, OPTIONS',
  'access-control-allow-headers': 'content-type, authorization',
  'access-control-max-age': '86400',
};

function json(body: unknown, init: { status?: number; cache?: string } = {}): Response {
  const headers: Record<string, string> = {
    'content-type': 'application/json; charset=utf-8',
    ...CORS_HEADERS,
  };
  if (init.cache) headers['cache-control'] = init.cache;
  return new Response(JSON.stringify(body, null, 2), { status: init.status ?? 200, headers });
}

function fail(status: number, reason: string): Response {
  return json({ error: reason }, { status });
}

const DIRECTORY_ROUTES = ['/api/sources.json', '/api/directory.json', '/api/apps.json'];
const JAM_ROUTES = ['/api/jam/entries.json'];
const CACHED_ROUTES = [...DIRECTORY_ROUTES, ...JAM_ROUTES, '/api/catalog'];

export function relayPath(sourceUrl: string): string {
  return `/api/catalog?url=${encodeURIComponent(sourceUrl)}`;
}

type EdgeCache = {
  match(request: Request): Promise<Response | undefined>;
  put(request: Request, response: Response): Promise<void>;
  delete(request: Request): Promise<boolean>;
};

const edgeCache = (caches as unknown as { default: EdgeCache }).default;

async function dropCached(origin: string, init: { routes?: string[]; sourceUrl?: string } = {}): Promise<void> {
  const targets = (init.routes ?? DIRECTORY_ROUTES).map(route => origin + route);
  if (init.sourceUrl) targets.push(origin + relayPath(init.sourceUrl));
  await Promise.all(targets.map(target => edgeCache.delete(new Request(target))));
}

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

async function handleAdmin(request: Request, env: Env, url: URL, caller: Principal | null, now: string) {
  const kv = kvOf(env);

  if (url.pathname === '/api/admin/me' && request.method === 'GET') {
    if (caller === null) return fail(401, 'a token is required');
    return json({ principal: caller }, { cache: 'no-store' });
  }

  if (caller === null || caller.role !== 'admin') return fail(401, 'admin token required');

  if (url.pathname === '/api/admin/sources') {
    if (request.method === 'GET') {
      return json({ sources: await listSources(kv) }, { cache: 'no-store' });
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
        kv,
        rawUrl: raw,
        status: status as SourceStatus,
        reviewedBy: principalLabel(caller),
        note: note as string | null | undefined,
        now,
      });
      if (!outcome.ok) return fail(outcome.status, outcome.reason);
      await dropCached(url.origin, { sourceUrl: outcome.record.url });
      return json({ source: outcome.record });
    }
  }

  if (url.pathname === '/api/admin/judges') {
    if (request.method === 'GET') {
      return json({ judges: await listJudges(kv) }, { cache: 'no-store' });
    }

    if (request.method === 'POST') {
      const body = await readJsonBody(request);
      const outcome = await createJudge({ kv, rawHandle: body?.['handle'], now });
      if (!outcome.ok) return fail(outcome.status, outcome.reason);
      return json({ judge: outcome.judge, token: outcome.token }, { status: 201, cache: 'no-store' });
    }

    if (request.method === 'DELETE') {
      const body = await readJsonBody(request);
      const outcome = await revokeJudge({ kv, rawHandle: body?.['handle'] });
      if (!outcome.ok) return fail(outcome.status, outcome.reason);
      return json({ revoked: outcome.handle }, { cache: 'no-store' });
    }
  }

  return null;
}

async function handleJam(request: Request, env: Env, url: URL, caller: Principal | null, now: string) {
  const kv = kvOf(env);

  if (url.pathname === '/api/jam/entries.json' && request.method === 'GET') {
    return json({ updated_at: now, entries: await jamGallery({ kv }) }, { cache: 'public, max-age=300' });
  }

  if (url.pathname === '/api/jam/catalog' && request.method === 'GET') {
    const client = `jam-catalog:${clientOf(request)}`;
    if (!(await takeRateLimitToken(kv, client, JAM_CATALOG_LIMIT, JAM_CATALOG_WINDOW_SECONDS))) {
      return fail(429, `at most ${JAM_CATALOG_LIMIT} catalog lookups per hour. try again later.`);
    }

    const outcome = await relayCatalog({ kv, rawUrl: url.searchParams.get('url'), access: 'known' });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return json({ url: outcome.url, catalog: outcome.catalog }, { cache: 'no-store' });
  }

  if (url.pathname === '/api/jam/entries' && request.method === 'POST') {
    const parsed = parseJamSubmission(await readJsonBody(request));
    if (!parsed.ok) return fail(parsed.status, parsed.reason);

    if (!(await takeRateLimitToken(kv, `jam:${clientOf(request)}`, JAM_LIMIT, JAM_WINDOW_SECONDS))) {
      return fail(429, `at most ${JAM_LIMIT} jam submissions per hour. try again later.`);
    }

    const submission = { ...parsed.submission, claim: bearerToken(request) ?? parsed.submission.claim };
    const outcome = await submitJamEntry({ kv, submission, now });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    await dropCached(url.origin, {
      routes: [...DIRECTORY_ROUTES, ...JAM_ROUTES],
      sourceUrl: outcome.entry.source_url,
    });
    return outcome.created
      ? json({ entry: entryView(outcome.entry), claim: outcome.claim }, { status: 201, cache: 'no-store' })
      : json({ entry: entryView(outcome.entry) }, { cache: 'no-store' });
  }

  if (url.pathname === '/api/jam/review' && request.method === 'GET') {
    if (caller === null) return fail(401, 'a judge or admin token is required');
    return json({ entries: await jamReview({ kv, handle: scoringHandle(caller) }) }, { cache: 'no-store' });
  }

  if (url.pathname === '/api/jam/scores' && request.method === 'PUT') {
    if (caller === null || caller.role !== 'judge') return fail(403, 'only a judge scores entries');
    const outcome = await putJamScore({ kv, body: await readJsonBody(request), handle: caller.handle, now });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return json({ score: outcome.score }, { cache: 'no-store' });
  }

  if (url.pathname === '/api/jam/tally' && request.method === 'GET') {
    if (caller === null || caller.role !== 'admin') return fail(401, 'admin token required');
    return json({ tally: await jamTally({ kv }) }, { cache: 'no-store' });
  }

  if (url.pathname === '/api/jam/entries' && request.method === 'PATCH') {
    if (caller === null || caller.role !== 'admin') return fail(401, 'admin token required');
    const outcome = await patchJamEntry({
      kv,
      body: await readJsonBody(request),
      reviewedBy: principalLabel(caller),
      now,
    });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    await dropCached(url.origin, {
      routes: [...DIRECTORY_ROUTES, ...JAM_ROUTES],
      sourceUrl: outcome.entry.source_url,
    });
    return json({ entry: entryView(outcome.entry), promoted: outcome.promoted }, { cache: 'no-store' });
  }

  return null;
}

async function handleApi(request: Request, env: Env, url: URL): Promise<Response> {
  const kv = kvOf(env);
  const now = new Date().toISOString();

  if (request.method === 'OPTIONS') return new Response(null, { status: 204, headers: CORS_HEADERS });

  if (url.pathname === '/api/sources.json' && request.method === 'GET') {
    const records = await listSources(kv);
    return json(toCatalogDocument(records, now), { cache: 'public, max-age=300' });
  }

  if (url.pathname === '/api/directory.json' && request.method === 'GET') {
    const records = await listSources(kv);
    return json({ updated_at: now, sources: toDirectoryView(records) }, { cache: 'public, max-age=60' });
  }

  if (url.pathname === '/api/apps.json' && request.method === 'GET') {
    return json(await mergedApps({ kv, now }), { cache: 'public, max-age=300' });
  }

  if (url.pathname === '/api/catalog' && request.method === 'GET') {
    const outcome = await relayCatalog({ kv, rawUrl: url.searchParams.get('url') });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return json(outcome.catalog, { cache: 'public, max-age=300' });
  }

  if (url.pathname === '/api/sources' && request.method === 'POST') {
    const body = await readJsonBody(request);
    const raw = body?.['url'];
    if (typeof raw !== 'string' || !raw.trim()) return fail(400, 'send a json body with a "url" string');

    if (!(await takeRateLimitToken(kv, `submit:${clientOf(request)}`, SUBMIT_LIMIT, SUBMIT_WINDOW_SECONDS))) {
      return fail(429, `at most ${SUBMIT_LIMIT} submissions per hour. try again later.`);
    }

    const outcome = await submitSource({ kv, rawUrl: raw, now });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    await dropCached(url.origin, { sourceUrl: outcome.record.url });
    return json({ source: outcome.record }, { status: outcome.created ? 201 : 200 });
  }

  if (url.pathname === '/api/installs' && request.method === 'POST') {
    if (!(await takeRateLimitToken(kv, `install:${clientOf(request)}`, INSTALL_LIMIT, INSTALL_WINDOW_SECONDS))) {
      return fail(429, `at most ${INSTALL_LIMIT} install reports per hour. try again later.`);
    }

    const outcome = await recordInstall({ kv, body: await readJsonBody(request), now });
    if (!outcome.ok) return fail(outcome.status, outcome.reason);
    return json({ installs: outcome.record.count }, { status: 202 });
  }

  if (url.pathname.startsWith('/api/admin/') || url.pathname.startsWith('/api/jam/')) {
    const caller = await principal(request, env);
    const handled = url.pathname.startsWith('/api/admin/')
      ? await handleAdmin(request, env, url, caller, now)
      : await handleJam(request, env, url, caller, now);
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
      const asset = await env.ASSETS.fetch(request);
      if (asset.status !== 404 || appIdFromPath(url.pathname) === null) return asset;
      return env.ASSETS.fetch(new Request(new URL(APP_DETAIL_SHELL, url.origin), request));
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
    const kv = kvOf(env);
    const now = new Date().toISOString();

    await rebuildInstalls(kv);
    await rebuildEntries(kv);
    await dropCached(SITE_ORIGIN, { routes: JAM_ROUTES });

    const stalest = (await rebuildSources(kv))
      .filter(isVisible)
      .sort((a, b) => a.last_checked_at.localeCompare(b.last_checked_at))
      .slice(0, RECHECK_BATCH);

    for (const record of stalest) {
      await recheckSource({ kv, record, now });
    }
  },
};
