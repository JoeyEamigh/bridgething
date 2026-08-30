import { describe, expect, test } from 'bun:test';
import type { Catalog } from '@bridgething/catalog';
import { fakeKv } from './kv-fake.ts';
import { recheckSource, setSourceStatus, submitSource } from './sources.ts';
import { readSource } from './store.ts';

const APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const SHA = '0'.repeat(64);
const NOW = '2026-07-24T12:00:00.000Z';
const LATER = '2026-07-25T12:00:00.000Z';
const CATALOG_URL = 'https://third.example/catalog.json';
const DOWNLOAD_URL = 'https://third.example/r/app.zip';

function catalog(overrides: Partial<Catalog> = {}): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-07-01T00:00:00Z',
    repo: { name: 'third party apps', description: 'some apps', homepage: null, icon: null },
    apps: [
      {
        id: APP_ID,
        name: 'Thing',
        description: 'Does a thing.',
        author: 'somebody',
        icon: null,
        homepage: null,
        source: null,
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
      },
    ],
    recommended_sources: [],
    ...overrides,
  };
}

type StubOptions = {
  body?: unknown;
  status?: number;
  cors?: boolean;
  downloadCors?: boolean;
  throws?: boolean;
};

function stubFetch(options: StubOptions = {}): typeof fetch {
  const { body = catalog(), status = 200, cors = true, downloadCors = true, throws = false } = options;

  return (async (input: string | URL | Request, init?: RequestInit) => {
    if (throws) throw new TypeError('network down');

    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url;

    if (init?.method === 'HEAD') {
      return new Response(null, {
        status: 200,
        headers: downloadCors ? { 'access-control-allow-origin': '*' } : {},
      });
    }

    if (url !== CATALOG_URL) return new Response('not found', { status: 404 });

    const text = typeof body === 'string' ? body : JSON.stringify(body);
    return new Response(text, {
      status,
      headers: {
        'content-type': 'application/json',
        ...(cors ? { 'access-control-allow-origin': '*' } : {}),
      },
    });
  }) as typeof fetch;
}

describe('submitSource', () => {
  test('auto-lists a valid source into quarantine', async () => {
    const kv = fakeKv();
    const outcome = await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.created).toBe(true);
    expect(outcome.record.status).toBe('quarantined');
    expect(outcome.record.name).toBe('third party apps');
    expect(outcome.record.app_count).toBe(1);
    expect(outcome.record.downloads_cors_ok).toBe(true);
  });

  test('refuses a source that does not send permissive cors', async () => {
    const kv = fakeKv();
    const outcome = await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch({ cors: false }) });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('Access-Control-Allow-Origin');
    expect(await readSource(kv, CATALOG_URL)).toBeNull();
  });

  test('refuses a source that is not a valid catalog.v1', async () => {
    const kv = fakeKv();
    const outcome = await submitSource({
      kv,
      rawUrl: CATALOG_URL,
      now: NOW,
      fetchImpl: stubFetch({ body: { schema: 'catalog.v1' } }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(422);
    expect(outcome.reason).toContain('not a valid catalog.v1');
  });

  test('flags a source whose downloads are not browser-readable without refusing it', async () => {
    const kv = fakeKv();
    const outcome = await submitSource({
      kv,
      rawUrl: CATALOG_URL,
      now: NOW,
      fetchImpl: stubFetch({ downloadCors: false }),
    });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.record.downloads_cors_ok).toBe(false);
    expect(outcome.record.status).toBe('quarantined');
  });

  test('a resubmit refreshes metadata but keeps the status an admin granted', async () => {
    const kv = fakeKv();
    await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch() });
    await setSourceStatus({ kv, rawUrl: CATALOG_URL, status: 'attested', reviewedBy: 'admin', now: NOW });

    const renamed = catalog({ repo: { name: 'renamed', description: 'x', homepage: null, icon: null } });
    const outcome = await submitSource({
      kv,
      rawUrl: CATALOG_URL,
      now: LATER,
      fetchImpl: stubFetch({ body: renamed }),
    });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.created).toBe(false);
    expect(outcome.record.status).toBe('attested');
    expect(outcome.record.name).toBe('renamed');
    expect(outcome.record.submitted_at).toBe(NOW);
  });

  test('a rejected source cannot resubmit itself back into the directory', async () => {
    const kv = fakeKv();
    await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch() });
    await setSourceStatus({ kv, rawUrl: CATALOG_URL, status: 'rejected', reviewedBy: 'admin', now: NOW });

    const outcome = await submitSource({ kv, rawUrl: CATALOG_URL, now: LATER, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(403);
    expect((await readSource(kv, CATALOG_URL))?.status).toBe('rejected');
  });

  test('a source that is down leaves the existing record untouched', async () => {
    const kv = fakeKv();
    await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch() });
    await setSourceStatus({ kv, rawUrl: CATALOG_URL, status: 'listed', reviewedBy: 'admin', now: NOW });

    const outcome = await submitSource({
      kv,
      rawUrl: CATALOG_URL,
      now: LATER,
      fetchImpl: stubFetch({ throws: true }),
    });

    expect(outcome.ok).toBe(false);
    const stored = await readSource(kv, CATALOG_URL);
    expect(stored?.status).toBe('listed');
    expect(stored?.last_check_ok).toBe(true);
  });

  test('rejects a non-https url before any network call', async () => {
    const kv = fakeKv();
    const outcome = await submitSource({
      kv,
      rawUrl: 'http://third.example/catalog.json',
      now: NOW,
      fetchImpl: stubFetch({ throws: true }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
  });
});

describe('recheckSource', () => {
  test('records the failure without demoting a source that went dark', async () => {
    const kv = fakeKv();
    await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch() });
    await setSourceStatus({ kv, rawUrl: CATALOG_URL, status: 'attested', reviewedBy: 'admin', now: NOW });

    const record = (await readSource(kv, CATALOG_URL))!;
    const updated = await recheckSource({ kv, record, now: LATER, fetchImpl: stubFetch({ throws: true }) });

    expect(updated.status).toBe('attested');
    expect(updated.last_check_ok).toBe(false);
    expect(updated.last_check_error).toContain('could not reach');
    expect(updated.last_checked_at).toBe(LATER);
  });

  test('clears a prior failure when the source comes back', async () => {
    const kv = fakeKv();
    await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch() });

    const record = (await readSource(kv, CATALOG_URL))!;
    const down = await recheckSource({ kv, record, now: LATER, fetchImpl: stubFetch({ throws: true }) });
    const back = await recheckSource({ kv, record: down, now: LATER, fetchImpl: stubFetch() });

    expect(back.last_check_ok).toBe(true);
    expect(back.last_check_error).toBeNull();
  });
});

describe('setSourceStatus', () => {
  test('404s an url nobody submitted', async () => {
    const outcome = await setSourceStatus({
      kv: fakeKv(),
      rawUrl: 'https://nobody.example/c.json',
      status: 'listed',
      reviewedBy: 'admin',
      now: NOW,
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(404);
  });

  test('stamps the review time and keeps the note when none is given', async () => {
    const kv = fakeKv();
    await submitSource({ kv, rawUrl: CATALOG_URL, now: NOW, fetchImpl: stubFetch() });
    await setSourceStatus({
      kv,
      rawUrl: CATALOG_URL,
      status: 'listed',
      reviewedBy: 'admin',
      note: 'looks fine',
      now: NOW,
    });

    const outcome = await setSourceStatus({
      kv,
      rawUrl: CATALOG_URL,
      status: 'attested',
      reviewedBy: 'admin',
      now: LATER,
    });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect(outcome.record.note).toBe('looks fine');
    expect(outcome.record.reviewed_at).toBe(LATER);
  });
});
