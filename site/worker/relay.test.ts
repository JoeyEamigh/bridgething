import { describe, expect, test } from 'bun:test';
import type { Catalog } from '@bridgething/catalog';
import type { SourceRecord, SourceStatus } from './directory.ts';
import { fakeKv } from './kv-fake.ts';
import { relayCatalog } from './relay.ts';
import { writeSource } from './store.ts';

const SOURCE_URL = 'https://third.example/catalog.json';

function catalog(): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-07-01T00:00:00Z',
    repo: { name: 'third party apps', description: 'some apps', homepage: null, icon: null },
    apps: [],
    recommended_sources: [],
  };
}

function record(status: SourceStatus, url = SOURCE_URL): SourceRecord {
  return {
    url,
    name: 'third party apps',
    description: null,
    homepage: null,
    icon: null,
    status,
    submitted_at: '2026-07-01T00:00:00.000Z',
    reviewed_at: null,
    reviewed_by: null,
    app_count: 0,
    last_checked_at: '2026-07-20T00:00:00.000Z',
    last_check_ok: true,
    last_check_error: null,
    downloads_cors_ok: true,
    note: null,
  };
}

function stubFetch(options: { body?: unknown; status?: number; throws?: boolean } = {}): typeof fetch {
  const { body = catalog(), status = 200, throws = false } = options;
  return (async () => {
    if (throws) throw new TypeError('network down');
    const text = typeof body === 'string' ? body : JSON.stringify(body);
    return new Response(text, { status, headers: { 'content-type': 'application/json' } });
  }) as unknown as typeof fetch;
}

async function kvWith(status: SourceStatus) {
  const kv = fakeKv();
  await writeSource(kv, record(status));
  return kv;
}

describe('relayCatalog', () => {
  test('relays a listed source', async () => {
    const outcome = await relayCatalog({ kv: await kvWith('listed'), rawUrl: SOURCE_URL, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(true);
    if (!outcome.ok) return;
    expect((outcome.catalog as Catalog).repo.name).toBe('third party apps');
  });

  test('relays an attested source', async () => {
    const outcome = await relayCatalog({ kv: await kvWith('attested'), rawUrl: SOURCE_URL, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(true);
  });

  test('refuses a quarantined source, which has not earned this origin', async () => {
    const outcome = await relayCatalog({ kv: await kvWith('quarantined'), rawUrl: SOURCE_URL, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(404);
    expect(outcome.reason).toContain('fetch it directly');
  });

  test('refuses a rejected source', async () => {
    const outcome = await relayCatalog({ kv: await kvWith('rejected'), rawUrl: SOURCE_URL, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(false);
  });

  test('refuses a url nobody submitted, so this is not an open fetch relay', async () => {
    const outcome = await relayCatalog({
      kv: await kvWith('attested'),
      rawUrl: 'https://attacker.example/anything.json',
      fetchImpl: stubFetch(),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(404);
  });

  test('refuses a non-https url before any network call', async () => {
    const outcome = await relayCatalog({
      kv: await kvWith('listed'),
      rawUrl: 'http://third.example/catalog.json',
      fetchImpl: stubFetch({ throws: true }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
  });

  test('matches the allowlist on the normalized url, not the raw one', async () => {
    const outcome = await relayCatalog({
      kv: await kvWith('listed'),
      rawUrl: `${SOURCE_URL}#fragment`,
      fetchImpl: stubFetch(),
    });

    expect(outcome.ok).toBe(true);
  });

  test('requires ?url=', async () => {
    const outcome = await relayCatalog({ kv: await kvWith('listed'), rawUrl: null, fetchImpl: stubFetch() });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
  });

  test('will not relay bytes that are not a catalog.v1 document', async () => {
    const outcome = await relayCatalog({
      kv: await kvWith('listed'),
      rawUrl: SOURCE_URL,
      fetchImpl: stubFetch({ body: '<html>gotcha</html>' }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(502);
  });

  test('reports an upstream outage as a gateway failure', async () => {
    const outcome = await relayCatalog({
      kv: await kvWith('listed'),
      rawUrl: SOURCE_URL,
      fetchImpl: stubFetch({ throws: true }),
    });

    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(502);
  });

  test('does not require the source to send cors, since we are not a browser', async () => {
    const outcome = await relayCatalog({ kv: await kvWith('listed'), rawUrl: SOURCE_URL, fetchImpl: stubFetch() });
    expect(outcome.ok).toBe(true);
  });
});
