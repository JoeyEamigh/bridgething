import { describe, expect, test } from 'bun:test';
import { fakeD1 } from './d1-fake.ts';
import { admitAll, catalog, LATER, NOW, origin, seedSource } from './fixtures.ts';
import { listSources, readSource, setSourceStatus, submitSource } from './sources.ts';

const CATALOG_URL = 'https://third.example/catalog.json';

describe('submitSource', () => {
  test('lists a valid source straight away', async () => {
    const db = fakeD1();
    const outcome = await submitSource({
      db,
      rawUrl: CATALOG_URL,
      now: NOW,
      admit: admitAll,
      fetchImpl: origin({ [CATALOG_URL]: { body: catalog('third party apps') } }).fetch,
    });

    expect(outcome.ok && outcome.created).toBe(true);
    expect(outcome.ok && outcome.record).toMatchObject({
      url: CATALOG_URL,
      name: 'third party apps',
      status: 'listed',
      reviewed_at: null,
      app_count: 1,
      last_check_ok: true,
      downloads_cors_ok: true,
    });
  });

  test('refuses a source that does not send permissive cors', async () => {
    const db = fakeD1();
    const outcome = await submitSource({
      db,
      rawUrl: CATALOG_URL,
      now: NOW,
      admit: admitAll,
      fetchImpl: origin({ [CATALOG_URL]: { cors: false } }).fetch,
    });

    expect(outcome).toMatchObject({ ok: false, status: 422 });
    expect(await readSource(db, CATALOG_URL)).toBeNull();
  });

  test('refuses a source that is not a valid catalog.v1', async () => {
    const db = fakeD1();
    const outcome = await submitSource({
      db,
      rawUrl: CATALOG_URL,
      now: NOW,
      admit: admitAll,
      fetchImpl: origin({ [CATALOG_URL]: { body: { schema: 'nope' } } }).fetch,
    });

    expect(outcome.ok).toBe(false);
    expect(!outcome.ok && outcome.reason).toContain('not a valid catalog.v1');
  });

  test('flags a source whose downloads are not browser-readable without refusing it', async () => {
    const db = fakeD1();
    const host = origin({ [CATALOG_URL]: {} });
    host.downloadsCors = false;

    const outcome = await submitSource({ db, rawUrl: CATALOG_URL, now: NOW, admit: admitAll, fetchImpl: host.fetch });

    expect(outcome.ok && outcome.record.downloads_cors_ok).toBe(false);
  });

  test('a resubmit refreshes metadata but keeps the status an admin granted', async () => {
    const db = fakeD1();
    await seedSource(db, CATALOG_URL, 'attested', catalog('old name'));

    const outcome = await submitSource({
      db,
      rawUrl: CATALOG_URL,
      now: LATER,
      admit: admitAll,
      fetchImpl: origin({ [CATALOG_URL]: { body: catalog('new name') } }).fetch,
    });

    expect(outcome.ok && outcome.created).toBe(false);
    expect(outcome.ok && outcome.record).toMatchObject({
      name: 'new name',
      status: 'attested',
      submitted_at: NOW,
      last_checked_at: LATER,
    });
  });

  test('a resubmit refetches unconditionally, so an author can force a new version through', async () => {
    const db = fakeD1();
    await seedSource(db, CATALOG_URL, 'listed');
    const host = origin({ [CATALOG_URL]: { etag: '"v1"' } });

    await submitSource({ db, rawUrl: CATALOG_URL, now: LATER, admit: admitAll, fetchImpl: host.fetch });

    expect(host.requests[0]!.headers.get('if-none-match')).toBeNull();
  });

  test('a rejected source cannot resubmit itself back into the directory', async () => {
    const db = fakeD1();
    await seedSource(db, CATALOG_URL, 'rejected');
    const host = origin({ [CATALOG_URL]: {} });

    const outcome = await submitSource({ db, rawUrl: CATALOG_URL, now: LATER, admit: admitAll, fetchImpl: host.fetch });

    expect(outcome).toMatchObject({ ok: false, status: 403 });
    expect(host.requests).toEqual([]);
  });

  test('a source that is down leaves the existing record untouched', async () => {
    const db = fakeD1();
    await seedSource(db, CATALOG_URL, 'listed');
    const before = await readSource(db, CATALOG_URL);

    const outcome = await submitSource({
      db,
      rawUrl: CATALOG_URL,
      now: LATER,
      admit: admitAll,
      fetchImpl: origin({ [CATALOG_URL]: { throws: true } }).fetch,
    });

    expect(outcome.ok).toBe(false);
    expect(await readSource(db, CATALOG_URL)).toEqual(before);
  });

  test('rejects a non-https url before any network call', async () => {
    const host = origin();
    const outcome = await submitSource({
      db: fakeD1(),
      rawUrl: 'http://third.example/catalog.json',
      now: NOW,
      admit: admitAll,
      fetchImpl: host.fetch,
    });

    expect(outcome).toMatchObject({ ok: false, status: 400 });
    expect(host.requests).toEqual([]);
  });

  test('the admission check sees whether the source is new and stops the probe when it refuses', async () => {
    const db = fakeD1();
    await seedSource(db, CATALOG_URL, 'listed');
    const host = origin({ [CATALOG_URL]: {} });
    const seen: (string | null)[] = [];

    const outcome = await submitSource({
      db,
      rawUrl: CATALOG_URL,
      now: LATER,
      admit: async existing => {
        seen.push(existing?.url ?? null);
        return { ok: false, status: 429, reason: 'slow down' };
      },
      fetchImpl: host.fetch,
    });

    expect(seen).toEqual([CATALOG_URL]);
    expect(outcome).toEqual({ ok: false, status: 429, reason: 'slow down' });
    expect(host.requests).toEqual([]);
  });
});

describe('setSourceStatus', () => {
  test('404s an url nobody submitted', async () => {
    const outcome = await setSourceStatus({
      db: fakeD1(),
      rawUrl: CATALOG_URL,
      status: 'listed',
      reviewedBy: 'admin',
      now: NOW,
    });

    expect(outcome).toMatchObject({ ok: false, status: 404 });
  });

  test('stamps the review time and keeps the note when none is given', async () => {
    const db = fakeD1();
    await seedSource(db, CATALOG_URL, 'listed');
    await setSourceStatus({ db, rawUrl: CATALOG_URL, status: 'listed', reviewedBy: 'admin', note: 'fine', now: NOW });

    const outcome = await setSourceStatus({
      db,
      rawUrl: CATALOG_URL,
      status: 'attested',
      reviewedBy: 'admin',
      now: LATER,
    });

    expect(outcome.ok && outcome.record).toMatchObject({
      status: 'attested',
      reviewed_at: LATER,
      reviewed_by: 'admin',
      note: 'fine',
    });
  });

  test('a null note clears it', async () => {
    const db = fakeD1();
    await seedSource(db, CATALOG_URL, 'listed');
    await setSourceStatus({ db, rawUrl: CATALOG_URL, status: 'listed', reviewedBy: 'admin', note: 'fine', now: NOW });

    const outcome = await setSourceStatus({
      db,
      rawUrl: CATALOG_URL,
      status: 'listed',
      reviewedBy: 'admin',
      note: null,
      now: LATER,
    });

    expect(outcome.ok && outcome.record.note).toBeNull();
  });
});

describe('listSources', () => {
  test('reads the whole directory with one query', async () => {
    const db = fakeD1();
    await seedSource(db, 'https://a.example/catalog.json', 'listed');
    await seedSource(db, 'https://b.example/catalog.json', 'quarantined');
    db.statements.length = 0;

    expect((await listSources(db)).map(record => record.url).sort()).toEqual([
      'https://a.example/catalog.json',
      'https://b.example/catalog.json',
    ]);
    expect(db.statements).toHaveLength(1);
  });
});
