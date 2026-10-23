import { describe, expect, test } from 'bun:test';
import { OFFICIAL_CATALOG_URL } from '@bridgething/catalog';
import { readCatalog, refreshCatalogs } from './catalogs.ts';
import { fakeD1 } from './d1-fake.ts';
import {
  app,
  APP_ID,
  catalog,
  LATER,
  NOW,
  origin,
  OTHER_APP_ID,
  seedOfficial,
  seedSource,
  seedUnreadable,
} from './fixtures.ts';
import { readSource } from './sources.ts';

const LISTED = 'https://listed.example/catalog.json';
const REJECTED = 'https://rejected.example/catalog.json';

describe('refreshCatalogs', () => {
  test('fetches the official catalog the first time round', async () => {
    const db = fakeD1();
    const host = origin({ [OFFICIAL_CATALOG_URL]: { body: catalog('bridgething apps') } });

    await refreshCatalogs({ db, now: NOW, fetchImpl: host.fetch });

    expect((await readCatalog(db, OFFICIAL_CATALOG_URL))?.name).toBe('bridgething apps');
  });

  test('an official catalog that is down on the first round writes nothing', async () => {
    const db = fakeD1();

    await refreshCatalogs({ db, now: NOW, fetchImpl: origin().fetch });

    expect(await readCatalog(db, OFFICIAL_CATALOG_URL)).toBeNull();
  });

  test('asks with the stored etag and keeps the body when the source answers 304', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    const host = origin({ [OFFICIAL_CATALOG_URL]: { etag: '"v1"', body: catalog('bridgething apps') } });
    await refreshCatalogs({ db, now: NOW, fetchImpl: host.fetch });
    const before = await readCatalog(db, OFFICIAL_CATALOG_URL);

    await refreshCatalogs({ db, now: LATER, fetchImpl: host.fetch });

    expect(host.requests.at(-1)!.headers.get('if-none-match')).toBe('"v1"');
    expect(await readCatalog(db, OFFICIAL_CATALOG_URL)).toEqual({ ...before!, checked_at: LATER });
  });

  test('a changed catalog replaces the body and the metadata derived from it', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed', catalog('one app'));
    const grown = catalog('two apps', [app(APP_ID, 'thing'), app(OTHER_APP_ID, 'other')]);

    await refreshCatalogs({ db, now: LATER, fetchImpl: origin({ [LISTED]: { body: grown } }).fetch });

    const held = await readCatalog(db, LISTED);
    expect(held).toMatchObject({ name: 'two apps', app_count: 2, checked_at: LATER, check_ok: 1 });
    expect(JSON.parse(held!.body!)).toEqual(grown);
  });

  test('an unchanged body skips the download probe', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed', catalog('same'));
    const host = origin({ [LISTED]: { body: catalog('same') } });

    await refreshCatalogs({ db, now: LATER, fetchImpl: host.fetch });

    expect(host.requests.filter(request => request.method === 'HEAD')).toEqual([]);
  });

  test('a source that went dark keeps its last good catalog and records why', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed', catalog('still here'));

    await refreshCatalogs({ db, now: LATER, fetchImpl: origin({ [LISTED]: { status: 500 } }).fetch });

    expect(await readCatalog(db, LISTED)).toMatchObject({ name: 'still here', check_ok: 0, checked_at: LATER });
    expect(await readSource(db, LISTED)).toMatchObject({
      status: 'listed',
      last_check_ok: false,
      last_check_error: `${LISTED} returned 500`,
    });
  });

  test('clears a prior failure when the source comes back', async () => {
    const db = fakeD1();
    await seedSource(db, LISTED, 'listed');
    await refreshCatalogs({ db, now: NOW, fetchImpl: origin({ [LISTED]: { throws: true } }).fetch });

    await refreshCatalogs({ db, now: LATER, fetchImpl: origin({ [LISTED]: {} }).fetch });

    expect(await readSource(db, LISTED)).toMatchObject({ last_check_ok: true, last_check_error: null });
  });

  test('a rejected source is never fetched again', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await seedSource(db, REJECTED, 'rejected');
    const host = origin();

    await refreshCatalogs({ db, now: LATER, fetchImpl: host.fetch });

    expect(host.requests.map(request => request.url)).toEqual([OFFICIAL_CATALOG_URL]);
  });

  test('a round fetches only its budget, stalest first', async () => {
    const db = fakeD1();
    await seedOfficial(db);
    await seedSource(db, 'https://a.example/catalog.json', 'listed', catalog('a'), '2026-07-03T00:00:00.000Z');
    await seedSource(db, 'https://b.example/catalog.json', 'listed', catalog('b'), '2026-07-01T00:00:00.000Z');
    await seedSource(db, 'https://c.example/catalog.json', 'quarantined', catalog('c'), '2026-07-02T00:00:00.000Z');
    const host = origin();

    await refreshCatalogs({ db, now: LATER, budget: 2, fetchImpl: host.fetch });

    expect(host.requests.map(request => request.url)).toEqual([
      'https://b.example/catalog.json',
      'https://c.example/catalog.json',
    ]);
  });

  test('a source that has never been readable keeps being asked and recovers once it is fixed', async () => {
    const db = fakeD1();
    await seedUnreadable(db, LISTED, 'listed', 'not a valid catalog.v1');

    await refreshCatalogs({ db, now: NOW, fetchImpl: origin({ [LISTED]: { status: 404 } }).fetch });
    expect(await readSource(db, LISTED)).toMatchObject({
      last_check_ok: false,
      last_check_error: `${LISTED} returned 404`,
    });

    await refreshCatalogs({ db, now: LATER, fetchImpl: origin({ [LISTED]: { body: catalog('fixed') } }).fetch });
    expect(await readCatalog(db, LISTED)).toMatchObject({ name: 'fixed', check_ok: 1 });
  });
});
