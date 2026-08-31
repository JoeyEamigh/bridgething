import { beforeEach, describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL } from '@bridgething/catalog';
import { listInstalls, toInstallCounts } from './installs.ts';
import { ENTRY_SNAPSHOT_KEY } from './jam.ts';
import { JAM_CATEGORIES } from '../src/lib/jam.ts';
import { fakeKv, type FakeKv } from './kv-fake.ts';
import { readSnapshot } from './store.ts';
import type { Env } from './env.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const CLIENT = '203.0.113.7';
const ADMIN_TOKEN = 'admin-secret';
const CATALOG_URL = 'https://third.example/catalog.json';
const ICON_URL = 'https://third.example/icon.png';

const dropped: string[] = [];

(globalThis as unknown as { caches: unknown }).caches = {
  default: {
    match: () => Promise.resolve(undefined),
    put: () => Promise.resolve(),
    delete: (request: Request) => {
      dropped.push(request.url);
      return Promise.resolve(true);
    },
  },
};

const worker = (await import('./index.ts')).default;

let kv: FakeKv;

function env(): Env {
  return { SOURCES: kv as unknown as KVNamespace, ASSETS: {} as Fetcher, ADMIN_TOKEN };
}

function context(): ExecutionContext {
  return { waitUntil: () => undefined, passThroughOnException: () => undefined } as unknown as ExecutionContext;
}

function post(path: string, body: unknown, client = CLIENT): Promise<Response> {
  const request = new Request(`https://bridgething.com${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'cf-connecting-ip': client },
    body: JSON.stringify(body),
  });
  return worker.fetch(request, env(), context());
}

function beacon(sourceUrl = OFFICIAL_CATALOG_URL): Record<string, unknown> {
  return { app_id: CALENDAR_ID, source_url: sourceUrl, version: '1.0.0' };
}

beforeEach(() => {
  kv = fakeKv();
  dropped.length = 0;
});

describe('POST /api/installs', () => {
  test('an install is accepted and answered with the tally it produced', async () => {
    const response = await post('/api/installs', beacon());

    expect(response.status).toBe(202);
    expect(await response.json<{ installs: number }>()).toEqual({ installs: 1 });
  });

  test('a second install of the same app from the same source adds to the tally', async () => {
    await post('/api/installs', beacon());
    const response = await post('/api/installs', beacon());

    expect(await response.json<{ installs: number }>()).toEqual({ installs: 2 });
    expect(toInstallCounts(await listInstalls(kv))).toEqual([
      { app_id: CALENDAR_ID, source_url: OFFICIAL_CATALOG_URL, count: 2 },
    ]);
  });

  test('a source outside the directory is refused', async () => {
    const response = await post('/api/installs', beacon('https://nobody.example/catalog.json'));

    expect(response.status).toBe(404);
    expect(await listInstalls(kv)).toHaveLength(0);
  });

  test('a body that is not json is refused rather than counted', async () => {
    const request = new Request('https://bridgething.com/api/installs', {
      method: 'POST',
      headers: { 'content-type': 'application/json', 'cf-connecting-ip': CLIENT },
      body: 'not json',
    });

    expect((await worker.fetch(request, env(), context())).status).toBe(400);
  });

  test('one client cannot report installs without limit', async () => {
    const statuses: number[] = [];
    for (let i = 0; i < 41; i += 1) statuses.push((await post('/api/installs', beacon())).status);

    expect(statuses.filter(status => status === 202)).toHaveLength(40);
    expect(statuses.at(-1)).toBe(429);
  });

  test('the limit is per client, so one busy installer cannot silence everyone else', async () => {
    for (let i = 0; i < 40; i += 1) await post('/api/installs', beacon());

    expect((await post('/api/installs', beacon(), '198.51.100.4')).status).toBe(202);
  });

  test('reporting installs does not spend the budget for submitting sources', async () => {
    for (let i = 0; i < 40; i += 1) await post('/api/installs', beacon());

    const original = globalThis.fetch;
    globalThis.fetch = (() => Promise.reject(new TypeError('no network in tests'))) as unknown as typeof fetch;
    try {
      expect((await post('/api/sources', { url: 'https://listed.example/catalog.json' })).status).not.toBe(429);
    } finally {
      globalThis.fetch = original;
    }
  });

  test('the endpoint only takes posts', async () => {
    const request = new Request('https://bridgething.com/api/installs', { method: 'GET' });

    expect((await worker.fetch(request, env(), context())).status).toBe(404);
  });
});

function call(
  method: string,
  path: string,
  init: { body?: unknown; token?: string; client?: string } = {},
): Promise<Response> {
  const headers: Record<string, string> = {
    'content-type': 'application/json',
    'cf-connecting-ip': init.client ?? CLIENT,
  };
  if (init.token) headers['authorization'] = `Bearer ${init.token}`;

  const request = new Request(`https://bridgething.com${path}`, {
    method,
    headers,
    body: init.body === undefined ? undefined : JSON.stringify(init.body),
  });
  return worker.fetch(request, env(), context());
}

const CATALOG = {
  schema: 'catalog.v1',
  updated_at: '2026-07-01T00:00:00Z',
  repo: { name: 'third party apps', description: 'some apps', homepage: null, icon: null },
  apps: [
    {
      id: CALENDAR_ID,
      name: 'Thing',
      description: 'Does a thing.',
      author: 'somebody',
      icon: ICON_URL,
      screenshots: ['https://third.example/shot.png'],
      homepage: null,
      source: 'https://github.com/someone/thing',
      versions: [
        {
          version: '1.0.0',
          released_at: '2026-07-01T00:00:00Z',
          download: { url: 'https://third.example/r/app.zip', size: 10, sha256: '0'.repeat(64) },
          permissions: [],
          min_libbridgething_version: '0.4.0',
          changelog: null,
        },
      ],
    },
  ],
  recommended_sources: [],
};

async function onStubbedNetwork<T>(run: () => Promise<T>): Promise<T> {
  const original = globalThis.fetch;
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;
    if (url === ICON_URL) return new Response('png', { status: 200 });
    if (init?.method === 'HEAD') {
      return new Response(null, { status: 200, headers: { 'access-control-allow-origin': '*' } });
    }
    if (url !== CATALOG_URL) return new Response('not found', { status: 404 });
    return new Response(JSON.stringify(CATALOG), {
      status: 200,
      headers: { 'content-type': 'application/json', 'access-control-allow-origin': '*' },
    });
  }) as unknown as typeof fetch;

  try {
    return await run();
  } finally {
    globalThis.fetch = original;
  }
}

function entryBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    source_url: CATALOG_URL,
    app_id: CALENDAR_ID,
    category: 'utility',
    video_url: 'https://youtu.be/abcdef',
    discord: 'somebody',
    wishlist: 'phone clipboard',
    ...overrides,
  };
}

async function enterJam(): Promise<string> {
  const response = await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
  expect(response.status).toBe(201);
  return (await response.json<{ claim: string }>()).claim;
}

async function registerSource(client = CLIENT): Promise<void> {
  const response = await onStubbedNetwork(() => call('POST', '/api/sources', { body: { url: CATALOG_URL }, client }));
  expect(response.status).toBe(201);
}

async function mintJudge(handle = 'espeon'): Promise<string> {
  const response = await call('POST', '/api/admin/judges', { body: { handle }, token: ADMIN_TOKEN });
  expect(response.status).toBe(201);
  return (await response.json<{ token: string }>()).token;
}

describe('GET /oauth/callback', () => {
  test('renders the bounce page without touching the asset store', async () => {
    const response = await worker.fetch(
      new Request('https://bridgething.com/oauth/callback?code=abc&state=xyz'),
      env(),
      context(),
    );

    expect(response.status).toBe(200);
    expect(response.headers.get('content-type')).toContain('text/html');
    expect(await response.text()).toContain('<a href="bridgething://oauth/callback?code=abc&amp;state=xyz">');
  });
});

describe('principals over http', () => {
  test('an unauthenticated caller is nobody', async () => {
    expect((await call('GET', '/api/admin/me')).status).toBe(401);
  });

  test('the admin token identifies as admin', async () => {
    const response = await call('GET', '/api/admin/me', { token: ADMIN_TOKEN });

    expect(response.status).toBe(200);
    expect(await response.json<unknown>()).toEqual({ principal: { role: 'admin' } });
  });

  test('a judge token identifies as that judge', async () => {
    const token = await mintJudge('68p');
    const response = await call('GET', '/api/admin/me', { token });

    expect(await response.json<unknown>()).toEqual({ principal: { role: 'judge', handle: '68p' } });
  });
});

describe('/api/admin/judges', () => {
  test('a judge cannot mint another judge', async () => {
    const token = await mintJudge();
    expect((await call('POST', '/api/admin/judges', { body: { handle: 'x' }, token })).status).toBe(401);
  });

  test('the token comes back once and never again', async () => {
    const token = await mintJudge('lmore377');
    const listed = await call('GET', '/api/admin/judges', { token: ADMIN_TOKEN });
    const text = await listed.text();

    expect(text).not.toContain(token);
    expect(JSON.parse(text)).toEqual({ judges: [{ handle: 'lmore377', created_at: expect.any(String) }] });
  });

  test('revoking a judge takes their token with it', async () => {
    const token = await mintJudge();
    const revoked = await call('DELETE', '/api/admin/judges', { body: { handle: 'espeon' }, token: ADMIN_TOKEN });

    expect(revoked.status).toBe(200);
    expect((await call('GET', '/api/admin/me', { token })).status).toBe(401);
  });
});

describe('/api/jam/entries', () => {
  test('a submission registers the source and pins the entry', async () => {
    const response = await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));

    expect(response.status).toBe(201);
    const body = await response.json<{ entry: { app_id: string; status: string } }>();
    expect(body.entry.app_id).toBe(CALENDAR_ID);
    expect(body.entry.status).toBe('submitted');
  });

  test('a resubmission with the claim token answers 200 rather than creating a second entry', async () => {
    const claim = await enterJam();
    const again = await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody({ claim }) }));

    expect(again.status).toBe(200);
  });

  test('the claim token comes back once, on the response that created the entry', async () => {
    const created = await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
    const body = await created.json<{ claim?: string; entry: { claim_hash: string } }>();

    expect(created.status).toBe(201);
    expect(body.claim).toMatch(/^[0-9a-f]{64}$/);

    const again = await onStubbedNetwork(() =>
      call('POST', '/api/jam/entries', { body: entryBody({ claim: body.claim }) }),
    );
    expect(await again.json<{ claim?: string }>()).not.toHaveProperty('claim');
  });

  test('a stranger cannot overwrite an entry they do not hold the claim token for', async () => {
    await enterJam();

    const stolen = await onStubbedNetwork(() =>
      call('POST', '/api/jam/entries', { body: entryBody({ discord: 'attacker' }), client: '198.51.100.4' }),
    );
    expect(stolen.status).toBe(403);

    const wrong = await onStubbedNetwork(() =>
      call('POST', '/api/jam/entries', {
        body: entryBody({ discord: 'attacker', claim: 'f'.repeat(64) }),
        client: '198.51.100.5',
      }),
    );
    expect(wrong.status).toBe(403);

    const review = await onStubbedNetwork(() => call('GET', '/api/jam/review', { token: ADMIN_TOKEN }));
    expect((await review.json<{ entries: { discord: string }[] }>()).entries[0]?.discord).toBe('somebody');
  });

  test('the claim token rides in an authorization header just as well as in the body', async () => {
    const claim = await enterJam();
    const again = await onStubbedNetwork(() =>
      call('POST', '/api/jam/entries', { body: entryBody({ category: 'cursed' }), token: claim }),
    );

    expect(again.status).toBe(200);
    expect(await again.json<{ entry: { category: string } }>()).toMatchObject({ entry: { category: 'cursed' } });
  });

  test('one client cannot flood the jam', async () => {
    const statuses = await onStubbedNetwork(async () => {
      const seen: number[] = [];
      for (let i = 0; i < 6; i += 1) seen.push((await call('POST', '/api/jam/entries', { body: entryBody() })).status);
      return seen;
    });

    expect(statuses.at(-1)).toBe(429);
  });

  test('a submission that fails local validation does not spend a rate limit token', async () => {
    for (let i = 0; i < 6; i += 1) {
      const rejected = await call('POST', '/api/jam/entries', { body: entryBody({ category: 'voice' }) });
      expect(rejected.status).toBe(400);
    }

    const accepted = await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
    expect(accepted.status).toBe(201);
  });

  test('the gallery is public and strips the contact fields', async () => {
    await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
    const response = await onStubbedNetwork(() => call('GET', '/api/jam/entries.json'));

    expect(response.status).toBe(200);
    const text = await response.text();
    expect(text).toContain(CALENDAR_ID);
    expect(text).not.toContain('phone clipboard');
  });

  test('only an admin may patch an entry', async () => {
    await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
    const token = await mintJudge();

    expect(
      (await call('PATCH', '/api/jam/entries', { body: { app_id: CALENDAR_ID, status: 'verified' }, token })).status,
    ).toBe(401);

    const promoted = await call('PATCH', '/api/jam/entries', {
      body: { app_id: CALENDAR_ID, promote: true },
      token: ADMIN_TOKEN,
    });
    expect(promoted.status).toBe(200);
    expect(await promoted.json<{ promoted: boolean }>()).toMatchObject({ promoted: true });
  });
});

describe('/api/jam/catalog', () => {
  test('serves a quarantined source the public relay refuses', async () => {
    await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));

    const relayed = await onStubbedNetwork(() => call('GET', `/api/catalog?url=${encodeURIComponent(CATALOG_URL)}`));
    expect(relayed.status).toBe(404);

    const picker = await onStubbedNetwork(() => call('GET', `/api/jam/catalog?url=${encodeURIComponent(CATALOG_URL)}`));
    expect(picker.status).toBe(200);
    expect(await picker.json<{ catalog: { apps: unknown[] } }>()).toMatchObject({ url: CATALOG_URL });
  });
});

describe('/api/jam/catalog rate limit', () => {
  test('one client cannot use the picker relay as a free fetch proxy', async () => {
    const path = `/api/jam/catalog?url=${encodeURIComponent(CATALOG_URL)}`;
    await registerSource();

    const statuses = await onStubbedNetwork(async () => {
      const seen: number[] = [];
      for (let i = 0; i < 61; i += 1) seen.push((await call('GET', path)).status);
      return seen;
    });

    expect(statuses.filter(status => status === 200)).toHaveLength(60);
    expect(statuses.at(-1)).toBe(429);
  });

  test('the limit is per client, so one busy picker cannot lock everyone out', async () => {
    const path = `/api/jam/catalog?url=${encodeURIComponent(CATALOG_URL)}`;
    await registerSource();

    await onStubbedNetwork(async () => {
      for (let i = 0; i < 60; i += 1) await call('GET', path);
    });

    const other = await onStubbedNetwork(() => call('GET', path, { client: '198.51.100.9' }));
    expect(other.status).toBe(200);
  });

  test('picker lookups do not spend the budget for entering the jam', async () => {
    const path = `/api/jam/catalog?url=${encodeURIComponent(CATALOG_URL)}`;
    await registerSource();

    const entered = await onStubbedNetwork(async () => {
      for (let i = 0; i < 60; i += 1) await call('GET', path);
      return call('POST', '/api/jam/entries', { body: entryBody() });
    });

    expect(entered.status).toBe(201);
  });
});

describe('/api/jam/review and scoring', () => {
  test('review needs a judge or admin token', async () => {
    expect((await call('GET', '/api/jam/review')).status).toBe(401);
    expect((await call('GET', '/api/jam/review', { token: ADMIN_TOKEN })).status).toBe(200);
  });

  test('a judge scores an entry and an admin cannot', async () => {
    await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
    const token = await mintJudge();

    const body = { app_id: CALENDAR_ID, category: 'utility', score: 4, note: 'good' };
    expect((await call('PUT', '/api/jam/scores', { body, token: ADMIN_TOKEN })).status).toBe(403);
    expect((await call('PUT', '/api/jam/scores', { body, token })).status).toBe(200);

    const review = await onStubbedNetwork(() => call('GET', '/api/jam/review', { token }));
    const entries = (await review.json<{ entries: { scores: unknown[] }[] }>()).entries;
    expect(entries[0]?.scores).toEqual([{ category: 'utility', score: 4, note: 'good' }]);
  });

  test('the tally is admin only', async () => {
    const token = await mintJudge();

    expect((await call('GET', '/api/jam/tally', { token })).status).toBe(401);
    const tally = await call('GET', '/api/jam/tally', { token: ADMIN_TOKEN });
    expect(tally.status).toBe(200);
    expect((await tally.json<{ tally: unknown[] }>()).tally).toHaveLength(JAM_CATEGORIES.length);
  });
});

describe('/api/jam/catalog is not an open relay', () => {
  test('a url the directory has never seen is refused even though it serves a catalog', async () => {
    const response = await onStubbedNetwork(() =>
      call('GET', `/api/jam/catalog?url=${encodeURIComponent(CATALOG_URL)}`),
    );

    expect(response.status).toBe(404);
    expect((await response.json<{ error: string }>()).error).toContain('not in the directory');
  });

  test('an upstream failure comes back generic rather than as a status oracle', async () => {
    await registerSource();

    const original = globalThis.fetch;
    globalThis.fetch = (async () =>
      new Response('nope', { status: 503, statusText: 'Service Unavailable' })) as unknown as typeof fetch;

    try {
      const response = await call('GET', `/api/jam/catalog?url=${encodeURIComponent(CATALOG_URL)}`);

      expect(response.status).toBe(502);
      expect(await response.json<{ error: string }>()).toEqual({ error: 'could not read that catalog' });
    } finally {
      globalThis.fetch = original;
    }
  });
});

describe('the claim hash never leaves the worker', () => {
  test('the create and resubmit responses carry the token but never its hash', async () => {
    const created = await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
    const text = await created.text();

    expect(created.headers.get('cache-control')).toBe('no-store');
    expect(text).not.toContain('claim_hash');

    const claim = JSON.parse(text).claim as string;
    const again = await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody({ claim }) }));

    expect(again.status).toBe(200);
    expect(await again.text()).not.toContain('claim_hash');
  });

  test('the admin patch response does not carry it either', async () => {
    await enterJam();
    const patched = await call('PATCH', '/api/jam/entries', {
      body: { app_id: CALENDAR_ID, status: 'verified' },
      token: ADMIN_TOKEN,
    });

    expect(patched.status).toBe(200);
    expect(await patched.text()).not.toContain('claim_hash');
  });
});

describe('the scheduled rebuild', () => {
  test('repairs an entry snapshot that lost the entries and drops the cached gallery', async () => {
    await onStubbedNetwork(() => call('POST', '/api/jam/entries', { body: entryBody() }));
    await kv.put(ENTRY_SNAPSHOT_KEY, '[]');
    dropped.length = 0;

    await onStubbedNetwork(() => worker.scheduled({} as ScheduledController, env()));

    expect((await readSnapshot(kv, ENTRY_SNAPSHOT_KEY))?.items).toHaveLength(1);
    expect(dropped).toContain('https://bridgething.com/api/jam/entries.json');
  });
});
