import { describe, expect, test } from 'bun:test';
import type { AppEntry, Catalog } from '@bridgething/catalog';
import { fakeKv, withListLag, type FakeKv } from './kv-fake.ts';
import { installKeyFor } from './installs.ts';
import {
  entryKeyFor,
  entryView,
  jamGallery,
  jamReview,
  jamTally,
  listEntries,
  parseJamSubmission,
  patchJamEntry,
  putJamScore,
  readEntry,
  rebuildEntries,
  scoreKeyFor,
  submitJamEntry,
  type JamCategory,
  type JamSubmitOutcome,
} from './jam.ts';
import { setSourceStatus } from './sources.ts';
import { readSource } from './store.ts';

const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const OTHER_ID = '019e6701-13f8-71b5-ba04-85d326630e99';
const SHA = '0'.repeat(64);
const NOW = '2026-09-01T12:00:00.000Z';
const LATER = '2026-09-09T12:00:00.000Z';
const CATALOG_URL = 'https://third.example/catalog.json';
const DOWNLOAD_URL = 'https://third.example/r/app.zip';
const ICON_URL = 'https://third.example/icon.png';
const SHOT_URL = 'https://third.example/shot.png';
const REPO_URL = 'https://github.com/someone/thing';
const VIDEO_URL = 'https://youtu.be/abcdef';

function app(overrides: Partial<AppEntry> = {}): AppEntry {
  return {
    id: APP_ID,
    name: 'Thing',
    description: 'Does a thing.',
    author: 'somebody',
    icon: ICON_URL,
    screenshots: [SHOT_URL],
    homepage: null,
    source: REPO_URL,
    versions: [
      {
        version: '1.0.0',
        released_at: '2026-07-01T00:00:00Z',
        download: { url: DOWNLOAD_URL, size: 10, sha256: SHA },
        permissions: ['net.fetch'],
        min_libbridgething_version: '0.4.0',
        changelog: null,
      },
    ],
    ...overrides,
  };
}

function catalog(apps: AppEntry[] = [app()]): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-07-01T00:00:00Z',
    repo: { name: 'third party apps', description: 'some apps', homepage: null, icon: null },
    apps,
    recommended_sources: [],
  };
}

type StubOptions = { body?: unknown; iconStatus?: number; shotStatus?: number };

function stubFetch(options: StubOptions = {}): typeof fetch {
  const { body = catalog(), iconStatus = 200, shotStatus = 200 } = options;

  return (async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;

    if (url === ICON_URL) return new Response(iconStatus === 200 ? 'png' : null, { status: iconStatus });
    if (url === SHOT_URL) return new Response(shotStatus === 200 ? 'png' : null, { status: shotStatus });
    if (init?.method === 'HEAD')
      return new Response(null, { status: 200, headers: { 'access-control-allow-origin': '*' } });
    if (url !== CATALOG_URL) return new Response('not found', { status: 404 });

    return new Response(typeof body === 'string' ? body : JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json', 'access-control-allow-origin': '*' },
    });
  }) as typeof fetch;
}

async function enter(args: {
  kv: FakeKv;
  body: Record<string, unknown>;
  now: string;
  fetchImpl?: typeof fetch;
}): Promise<JamSubmitOutcome> {
  const parsed = parseJamSubmission(args.body);
  if (!parsed.ok) return parsed;
  return submitJamEntry({ kv: args.kv, submission: parsed.submission, now: args.now, fetchImpl: args.fetchImpl });
}

function entryBody(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    source_url: CATALOG_URL,
    app_id: APP_ID,
    category: 'utility',
    video_url: VIDEO_URL,
    discord: 'somebody',
    wishlist: 'let me read the phone clipboard',
    ...overrides,
  };
}

async function seeded(options: StubOptions = {}): Promise<{ kv: FakeKv; claim: string }> {
  const kv = fakeKv();
  const outcome = await enter({ kv, body: entryBody(), now: NOW, fetchImpl: stubFetch(options) });
  expect(outcome.ok).toBe(true);
  if (!outcome.ok || !outcome.created) throw new Error('the seed entry was not created');
  return { kv, claim: outcome.claim };
}

async function score(kv: FakeKv, args: { appId?: string; handle: string; category: JamCategory; score: number }) {
  const outcome = await putJamScore({
    kv,
    body: { app_id: args.appId ?? APP_ID, category: args.category, score: args.score, note: `${args.handle} says hi` },
    handle: args.handle,
    now: NOW,
  });
  expect(outcome.ok).toBe(true);
}

describe('parseJamSubmission', () => {
  test('normalizes the source url and the app id before anything touches the network', () => {
    const parsed = parseJamSubmission(
      entryBody({ source_url: 'third.example/catalog.json', app_id: APP_ID.toUpperCase() }),
    );

    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.submission.sourceUrl).toBe(CATALOG_URL);
    expect(parsed.submission.appId).toBe(APP_ID);
  });

  test('every local rule fails before the entry ever reaches the kv', () => {
    const bad: Record<string, unknown>[] = [
      entryBody({ source_url: 'http://third.example/catalog.json' }),
      entryBody({ app_id: '' }),
      entryBody({ category: 'voice' }),
      entryBody({ video_url: 'http://youtu.be/abcdef' }),
      entryBody({ discord: '   ' }),
      entryBody({ wishlist: 42 }),
    ];

    for (const body of bad) {
      const parsed = parseJamSubmission(body);
      expect(parsed.ok).toBe(false);
      if (!parsed.ok) expect(parsed.status).toBe(400);
    }
  });

  test('the wishlist is optional and capped', () => {
    const parsed = parseJamSubmission(entryBody({ wishlist: 'x'.repeat(5000) }));

    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.submission.wishlist).toHaveLength(1200);
  });
});

describe('submitJamEntry', () => {
  test('registers the source and pins the entry', async () => {
    const kv = fakeKv();
    const outcome = await enter({ kv, body: entryBody(), now: NOW, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.created).toBe(true);
    expect(outcome.entry.status).toBe('submitted');
    expect(outcome.entry.source_url).toBe(CATALOG_URL);
    expect((await readSource(kv, CATALOG_URL))?.status).toBe('quarantined');
  });

  test('an app id the catalog does not list is refused', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody({ app_id: OTHER_ID }),
      now: NOW,
      fetchImpl: stubFetch(),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('does not list an app');
  });

  test('an app with no icon is refused', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ body: catalog([app({ icon: null })]) }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('no icon');
  });

  test('an icon nobody can fetch is refused', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ iconStatus: 404 }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('could not be fetched');
  });

  test('an app whose repo url is not https is refused', async () => {
    for (const source of ['http://github.com/someone/thing', 'javascript:alert(1)']) {
      const outcome = await enter({
        kv: fakeKv(),
        body: entryBody(),
        now: NOW,
        fetchImpl: stubFetch({ body: catalog([app({ source })]) }),
      });

      expect(outcome.ok).toBe(false);
      if (outcome.ok) return;
      expect(outcome.status).toBe(422);
      expect(outcome.reason).toContain('must be https');
    }
  });

  test('an app with no screenshots is refused with the fix in the message', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ body: catalog([app({ screenshots: undefined })]) }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('has no screenshots');
    expect(outcome.reason).toContain('800x480');
  });

  test('a screenshot the directory cannot fetch is refused and names the url', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ shotStatus: 404 }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain(SHOT_URL);
  });

  test('a non-https screenshot is refused before it is fetched', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ body: catalog([app({ screenshots: ['http://third.example/shot.png'] })]) }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.reason).toContain('must be https');
  });

  test('an app with no repo is refused because jam entries are open source', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ body: catalog([app({ source: null })]) }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('open source');
  });

  test('an entry that ships a native extension keeps its github repo', async () => {
    const withExtension = app({
      versions: [{ ...app().versions[0]!, extension: { desktop: true, permissions: ['all'] } }],
    });
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ body: catalog([withExtension]) }),
    });

    expect(outcome.ok).toBe(true);
  });

  test('an extension whose repo is not github is refused by the catalog validator, not by the jam route', async () => {
    const withExtension = app({
      source: 'https://gitlab.example/someone/thing',
      versions: [{ ...app().versions[0]!, extension: { desktop: true, permissions: ['all'] } }],
    });
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody(),
      now: NOW,
      fetchImpl: stubFetch({ body: catalog([withExtension]) }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('is not a valid catalog.v1');
    expect(outcome.reason).toContain('github.com repo url');
  });

  test('a video that is not https is refused', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody({ video_url: 'http://youtu.be/abcdef' }),
      now: NOW,
      fetchImpl: stubFetch(),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
  });

  test('a category outside the five is refused before any network call', async () => {
    const outcome = await enter({
      kv: fakeKv(),
      body: entryBody({ category: 'voice' }),
      now: NOW,
      fetchImpl: (() => Promise.reject(new TypeError('no network'))) as unknown as typeof fetch,
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
    expect(outcome.reason).toContain('launcher');
  });

  test('a missing discord handle is refused because prizes are handed out there', async () => {
    const body = entryBody();
    delete body['discord'];
    const outcome = await enter({ kv: fakeKv(), body, now: NOW, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
  });

  test('resubmitting the same app with its claim token updates the fields and keeps the first submission time', async () => {
    const { kv, claim } = await seeded();
    const outcome = await enter({
      kv,
      body: entryBody({ claim, category: 'cursed', video_url: 'https://youtu.be/second' }),
      now: LATER,
      fetchImpl: stubFetch(),
    });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.created).toBe(false);
    expect(outcome.entry.category).toBe('cursed');
    expect(outcome.entry.submitted_at).toBe(NOW);
    expect(outcome.entry.updated_at).toBe(LATER);
  });

  test('a resubmit does not undo a status the admin granted', async () => {
    const { kv, claim } = await seeded();
    await patchJamEntry({ kv, body: { app_id: APP_ID, status: 'verified' }, reviewedBy: 'admin', now: NOW });

    const outcome = await enter({ kv, body: entryBody({ claim }), now: LATER, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.entry.status).toBe('verified');
  });

  test('creating an entry mints a claim token once and stores only its hash', async () => {
    const kv = fakeKv();
    const outcome = await enter({ kv, body: entryBody(), now: NOW, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok || !outcome.created) throw new Error('the entry was not created');
    expect(outcome.claim).toMatch(/^[0-9a-f]{64}$/);

    const held = await readEntry(kv, APP_ID);
    expect(held?.claim_hash).toMatch(/^[0-9a-f]{64}$/);
    expect(held?.claim_hash).not.toBe(outcome.claim);
    expect(JSON.stringify(kv.snapshot())).not.toContain(outcome.claim);
  });

  test('a resubmit with no claim token cannot overwrite somebody else entry', async () => {
    const { kv } = await seeded();
    const outcome = await enter({
      kv,
      body: entryBody({ discord: 'attacker', video_url: 'https://evil.example/clip' }),
      now: LATER,
      fetchImpl: stubFetch(),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(403);
    expect(outcome.reason).toContain('claim token');
    expect((await readEntry(kv, APP_ID))?.discord).toBe('somebody');
  });

  test('a resubmit with the wrong claim token is refused too', async () => {
    const { kv } = await seeded();
    const outcome = await enter({
      kv,
      body: entryBody({ claim: 'f'.repeat(64), discord: 'attacker' }),
      now: LATER,
      fetchImpl: stubFetch(),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(403);
    expect((await readEntry(kv, APP_ID))?.discord).toBe('somebody');
  });

  test('a wrong claim token is refused before the source is ever probed', async () => {
    const { kv } = await seeded();
    const outcome = await enter({
      kv,
      body: entryBody({ claim: 'f'.repeat(64) }),
      now: LATER,
      fetchImpl: (() => Promise.reject(new TypeError('no network'))) as unknown as typeof fetch,
    });

    expect(outcome.ok).toBe(false);
  });

  test('a resubmit keeps the same claim token rather than minting a second one', async () => {
    const { kv, claim } = await seeded();
    const before = (await readEntry(kv, APP_ID))?.claim_hash;

    const outcome = await enter({ kv, body: entryBody({ claim }), now: LATER, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.created).toBe(false);
    expect((await readEntry(kv, APP_ID))?.claim_hash).toBe(before!);
  });

  test('an admin patch needs no claim token and leaves the owner in charge', async () => {
    const { kv, claim } = await seeded();

    const patched = await patchJamEntry({
      kv,
      body: { app_id: APP_ID, status: 'verified' },
      reviewedBy: 'admin',
      now: LATER,
    });
    expect(patched.ok).toBe(true);

    const outcome = await enter({
      kv,
      body: entryBody({ claim, category: 'cursed' }),
      now: LATER,
      fetchImpl: stubFetch(),
    });
    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.entry.category).toBe('cursed');
    expect(outcome.entry.status).toBe('verified');
  });

  test('a disqualified entry cannot resubmit itself back into the jam', async () => {
    const { kv } = await seeded();
    await patchJamEntry({ kv, body: { app_id: APP_ID, status: 'disqualified' }, reviewedBy: 'admin', now: NOW });

    const outcome = await enter({ kv, body: entryBody(), now: LATER, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(403);
  });
});

describe('the submission window', () => {
  const timeline = {
    opensAt: '2026-09-01T00:00:00.000Z',
    closesAt: '2026-09-15T00:00:00.000Z',
    resultsAt: null,
  };

  async function attempt(now: string) {
    const kv = fakeKv();
    const parsed = parseJamSubmission(entryBody());
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) throw new Error(parsed.reason);
    return submitJamEntry({ kv, submission: parsed.submission, now, timeline, fetchImpl: stubFetch() });
  }

  test('an entry before the jam opens is refused', async () => {
    const outcome = await attempt('2026-08-20T00:00:00.000Z');

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(403);
    expect(outcome.reason).toContain('opens');
  });

  test('an entry after the jam closes is refused', async () => {
    const outcome = await attempt('2026-09-20T00:00:00.000Z');

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(403);
    expect(outcome.reason).toContain('closed');
  });

  test('an entry inside the window is taken', async () => {
    expect((await attempt('2026-09-05T00:00:00.000Z')).ok).toBe(true);
  });

  test('a closed window is refused before the source is ever probed', async () => {
    const kv = fakeKv();
    const parsed = parseJamSubmission(entryBody());
    if (!parsed.ok) throw new Error(parsed.reason);

    const outcome = await submitJamEntry({
      kv,
      submission: parsed.submission,
      now: '2026-08-20T00:00:00.000Z',
      timeline,
      fetchImpl: (() => Promise.reject(new TypeError('no network'))) as unknown as typeof fetch,
    });

    expect(outcome.ok).toBe(false);
    expect(Object.keys(kv.snapshot())).toHaveLength(0);
  });

  test('the shipped timeline leaves the jam open', async () => {
    const kv = fakeKv();
    const parsed = parseJamSubmission(entryBody());
    if (!parsed.ok) throw new Error(parsed.reason);

    const outcome = await submitJamEntry({ kv, submission: parsed.submission, now: NOW, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(true);
  });
});

describe('jamGallery', () => {
  test('joins the catalog fields and never leaks contact or wishlist', async () => {
    const { kv } = await seeded();
    const gallery = await jamGallery({ kv, fetchImpl: stubFetch() });

    expect(gallery).toHaveLength(1);
    expect(gallery[0]).toEqual({
      app_id: APP_ID,
      source_url: CATALOG_URL,
      category: 'utility',
      video_url: VIDEO_URL,
      status: 'submitted',
      submitted_at: NOW,
      name: 'Thing',
      description: 'Does a thing.',
      author: 'somebody',
      icon: ICON_URL,
      screenshot: SHOT_URL,
      repo: REPO_URL,
    });
    expect(JSON.stringify(gallery)).not.toContain('let me read the phone clipboard');
    expect(JSON.stringify(gallery)).not.toContain('somebody says');
  });

  test('the claim hash never reaches the gallery or the judges', async () => {
    const { kv } = await seeded();
    const held = await readEntry(kv, APP_ID);

    expect(JSON.stringify(await jamGallery({ kv, fetchImpl: stubFetch() }))).not.toContain(held!.claim_hash);
    expect(JSON.stringify(await jamReview({ kv, handle: 'espeon', fetchImpl: stubFetch() }))).not.toContain(
      held!.claim_hash,
    );
  });

  test('a disqualified entry drops out of the public gallery', async () => {
    const { kv } = await seeded();
    await patchJamEntry({ kv, body: { app_id: APP_ID, status: 'disqualified' }, reviewedBy: 'admin', now: NOW });

    expect(await jamGallery({ kv, fetchImpl: stubFetch() })).toHaveLength(0);
  });

  test('a source that went dark still lists the entry, with the catalog fields blank', async () => {
    const { kv } = await seeded();
    const dark = (() => Promise.reject(new TypeError('network down'))) as unknown as typeof fetch;

    const gallery = await jamGallery({ kv, fetchImpl: dark });
    expect(gallery).toHaveLength(1);
    expect(gallery[0]?.name).toBeNull();
    expect(gallery[0]?.video_url).toBe(VIDEO_URL);
  });
});

describe('jamReview', () => {
  test('shows the judge the contact fields, install count, source status and their own scores', async () => {
    const { kv } = await seeded();
    await kv.put(
      installKeyFor(APP_ID, CATALOG_URL),
      JSON.stringify({ app_id: APP_ID, source_url: CATALOG_URL, count: 7, last_at: NOW, last_version: '1.0.0' }),
    );
    await score(kv, { handle: 'espeon', category: 'utility', score: 4 });
    await score(kv, { handle: '68p', category: 'utility', score: 1 });

    const [entry] = await jamReview({ kv, handle: 'espeon', fetchImpl: stubFetch() });

    expect(entry?.discord).toBe('somebody');
    expect(entry?.wishlist).toBe('let me read the phone clipboard');
    expect(entry?.installs).toBe(7);
    expect(entry?.source_status).toBe('quarantined');
    expect(entry?.scores).toEqual([{ category: 'utility', score: 4, note: 'espeon says hi' }]);
  });

  test('an admin sees the entries with nobody else scores attached', async () => {
    const { kv } = await seeded();
    await score(kv, { handle: 'espeon', category: 'utility', score: 4 });

    const [entry] = await jamReview({ kv, handle: null, fetchImpl: stubFetch() });
    expect(entry?.scores).toEqual([]);
  });

  test('a disqualified entry stays visible to review so it can be undone', async () => {
    const { kv } = await seeded();
    await patchJamEntry({ kv, body: { app_id: APP_ID, status: 'disqualified' }, reviewedBy: 'admin', now: NOW });

    expect(await jamReview({ kv, handle: null, fetchImpl: stubFetch() })).toHaveLength(1);
  });
});

describe('putJamScore', () => {
  test('a score is pinned per entry, judge, and category', async () => {
    const { kv } = await seeded();
    await score(kv, { handle: 'espeon', category: 'utility', score: 5 });

    expect(kv.snapshot()[scoreKeyFor(APP_ID, 'espeon', 'utility')]).toContain('"score":5');
  });

  test('rescoring the same category replaces the old score', async () => {
    const { kv } = await seeded();
    await score(kv, { handle: 'espeon', category: 'utility', score: 5 });
    await score(kv, { handle: 'espeon', category: 'utility', score: 2 });

    const tally = await jamTally({ kv, fetchImpl: stubFetch() });
    expect(tally.find(row => row.category === 'utility')?.entries[0]?.scores).toEqual([
      { handle: 'espeon', score: 2, note: 'espeon says hi' },
    ]);
  });

  test('a score outside one to five is refused', async () => {
    const { kv } = await seeded();
    for (const bad of [0, 6, 3.5]) {
      const outcome = await putJamScore({
        kv,
        body: { app_id: APP_ID, category: 'utility', score: bad },
        handle: 'espeon',
        now: NOW,
      });
      expect(outcome.ok).toBe(false);
      if (!outcome.ok) expect(outcome.status).toBe(400);
    }
  });

  test('scoring an app nobody entered is a 404', async () => {
    const outcome = await putJamScore({
      kv: fakeKv(),
      body: { app_id: OTHER_ID, category: 'utility', score: 3 },
      handle: 'espeon',
      now: NOW,
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(404);
  });
});

describe('jamTally', () => {
  test('means the scores per category and breaks them down per judge', async () => {
    const { kv } = await seeded();
    await score(kv, { handle: 'espeon', category: 'utility', score: 5 });
    await score(kv, { handle: '68p', category: 'utility', score: 2 });

    const utility = (await jamTally({ kv, fetchImpl: stubFetch() })).find(row => row.category === 'utility');

    expect(utility?.entries).toHaveLength(1);
    expect(utility?.entries[0]?.mean).toBe(3.5);
    expect(utility?.entries[0]?.count).toBe(2);
    expect(utility?.entries[0]?.scores).toEqual([
      { handle: '68p', score: 2, note: '68p says hi' },
      { handle: 'espeon', score: 5, note: 'espeon says hi' },
    ]);
  });

  test('an entry the panel nominated elsewhere shows up in that category too', async () => {
    const { kv } = await seeded();
    await score(kv, { handle: 'espeon', category: 'cursed', score: 5 });

    const tally = await jamTally({ kv, fetchImpl: stubFetch() });
    const cursed = tally.find(row => row.category === 'cursed');

    expect(cursed?.entries[0]?.app_id).toBe(APP_ID);
    expect(cursed?.entries[0]?.primary).toBe(false);
    expect(cursed?.entries[0]?.mean).toBe(5);
  });

  test('an unscored entry still lists under its own category, ranked last', async () => {
    const { kv } = await seeded();
    const second = app({ id: OTHER_ID, name: 'Other' });
    await enter({
      kv,
      body: entryBody({ app_id: OTHER_ID }),
      now: LATER,
      fetchImpl: stubFetch({ body: catalog([app(), second]) }),
    });
    await score(kv, { appId: OTHER_ID, handle: 'espeon', category: 'utility', score: 3 });

    const utility = (await jamTally({ kv, fetchImpl: stubFetch({ body: catalog([app(), second]) }) })).find(
      row => row.category === 'utility',
    );

    expect(utility?.entries.map(entry => entry.app_id)).toEqual([OTHER_ID, APP_ID]);
    expect(utility?.entries[1]?.mean).toBeNull();
    expect(utility?.entries[1]?.count).toBe(0);
  });

  test('a disqualified entry and its scores fall out of the tally', async () => {
    const { kv } = await seeded();
    await score(kv, { handle: 'espeon', category: 'utility', score: 5 });
    await patchJamEntry({ kv, body: { app_id: APP_ID, status: 'disqualified' }, reviewedBy: 'admin', now: NOW });

    const utility = (await jamTally({ kv, fetchImpl: stubFetch() })).find(row => row.category === 'utility');
    expect(utility?.entries).toHaveLength(0);
  });

  test('every category comes back even when nothing scored in it', async () => {
    const tally = await jamTally({ kv: fakeKv(), fetchImpl: stubFetch() });
    expect(tally.map(row => row.category)).toEqual(['launcher', 'music', 'utility', 'desk', 'cursed']);
  });
});

describe('patchJamEntry', () => {
  test('an admin can set the status', async () => {
    const { kv } = await seeded();
    const outcome = await patchJamEntry({
      kv,
      body: { app_id: APP_ID, status: 'verified' },
      reviewedBy: 'admin',
      now: LATER,
    });

    expect(outcome.ok).toBe(true);
    expect((await readEntry(kv, APP_ID))?.status).toBe('verified');
  });

  test('promote leaves a source that is already attested alone', async () => {
    const { kv } = await seeded();
    await setSourceStatus({ kv, rawUrl: CATALOG_URL, status: 'attested', reviewedBy: 'espeon', now: NOW });

    const outcome = await patchJamEntry({
      kv,
      body: { app_id: APP_ID, promote: true },
      reviewedBy: 'admin',
      now: LATER,
    });

    expect(outcome.ok).toBe(true);
    const source = await readSource(kv, CATALOG_URL);
    expect(source?.status).toBe('attested');
    expect(source?.reviewed_by).toBe('espeon');
  });

  test('promote lists the source the entry came from and records who did it', async () => {
    const { kv } = await seeded();
    const outcome = await patchJamEntry({
      kv,
      body: { app_id: APP_ID, promote: true },
      reviewedBy: 'admin',
      now: LATER,
    });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.promoted).toBe(true);

    const source = await readSource(kv, CATALOG_URL);
    expect(source?.status).toBe('listed');
    expect(source?.reviewed_by).toBe('admin');
  });

  test('a patch that asks for nothing is refused', async () => {
    const { kv } = await seeded();
    const outcome = await patchJamEntry({ kv, body: { app_id: APP_ID }, reviewedBy: 'admin', now: NOW });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
  });

  test('patching an app nobody entered is a 404', async () => {
    const outcome = await patchJamEntry({
      kv: fakeKv(),
      body: { app_id: OTHER_ID, status: 'verified' },
      reviewedBy: 'admin',
      now: NOW,
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(404);
  });
});

describe('source moderation attribution', () => {
  test('a judge handle rides along on the source record', async () => {
    const { kv } = await seeded();
    await setSourceStatus({ kv, rawUrl: CATALOG_URL, status: 'attested', reviewedBy: 'espeon', now: LATER });

    expect((await readSource(kv, CATALOG_URL))?.reviewed_by).toBe('espeon');
  });
});

describe('the entry snapshot', () => {
  test('a fresh entry is in the gallery even before kv list can enumerate it', async () => {
    const { kv } = await seeded();
    expect(await listEntries(kv)).toHaveLength(1);

    const lagging = withListLag(kv);
    const outcome = await enter({
      kv: lagging,
      body: entryBody({ app_id: OTHER_ID }),
      now: LATER,
      fetchImpl: stubFetch({ body: catalog([app(), app({ id: OTHER_ID })]) }),
    });
    expect(outcome.ok).toBe(true);

    expect((await listEntries(lagging)).map(entry => entry.app_id).sort()).toEqual([APP_ID, OTHER_ID]);
  });

  test('a rebuild keeps a fresh entry the snapshot holds but kv list cannot enumerate yet', async () => {
    const { kv } = await seeded();
    const lagging = withListLag(kv);
    const outcome = await enter({
      kv: lagging,
      body: entryBody({ app_id: OTHER_ID }),
      now: LATER,
      fetchImpl: stubFetch({ body: catalog([app(), app({ id: OTHER_ID })]) }),
    });
    expect(outcome.ok).toBe(true);

    expect((await rebuildEntries(lagging)).map(entry => entry.app_id).sort()).toEqual([APP_ID, OTHER_ID]);
    expect((await listEntries(lagging)).map(entry => entry.app_id).sort()).toEqual([APP_ID, OTHER_ID]);
  });

  test('a rebuild drops a snapshot entry whose backing record is gone', async () => {
    const { kv } = await seeded();
    await kv.delete(entryKeyFor(APP_ID));

    expect(await rebuildEntries(kv)).toEqual([]);
  });

  test('the first entry ever submitted is in the gallery with no snapshot to merge into', async () => {
    const kv = withListLag(fakeKv());
    const outcome = await enter({ kv, body: entryBody(), now: NOW, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(true);

    expect((await listEntries(kv)).map(entry => entry.app_id)).toEqual([APP_ID]);
    expect((await jamGallery({ kv, fetchImpl: stubFetch() })).map(listing => listing.app_id)).toEqual([APP_ID]);
  });
});

describe('entryView', () => {
  test('drops the claim hash the entry is authenticated by', async () => {
    const { kv } = await seeded();
    const entry = await readEntry(kv, APP_ID);

    expect(entry?.claim_hash).toMatch(/^[0-9a-f]{64}$/);
    expect(entryView(entry!)).not.toHaveProperty('claim_hash');
    expect(entryView(entry!).app_id).toBe(APP_ID);
  });
});
