import { describe, expect, test } from 'bun:test';
import { fakeD1, type FakeD1 } from './d1-fake.ts';
import { catalog, seedSource, seedUnreadable } from './fixtures.ts';
import { relayCatalog } from './relay.ts';

const LISTED = 'https://listed.example/catalog.json';
const ATTESTED = 'https://attested.example/catalog.json';
const QUARANTINED = 'https://unreviewed.example/catalog.json';
const REJECTED = 'https://rejected.example/catalog.json';

async function seeded(): Promise<FakeD1> {
  const db = fakeD1();
  await seedSource(db, LISTED, 'listed', catalog('listed'));
  await seedSource(db, ATTESTED, 'attested', catalog('attested'));
  await seedSource(db, QUARANTINED, 'quarantined');
  await seedSource(db, REJECTED, 'rejected');
  return db;
}

describe('relayCatalog', () => {
  test('relays the stored catalog of a listed or attested source', async () => {
    const db = await seeded();

    for (const [url, name] of [
      [LISTED, 'listed'],
      [ATTESTED, 'attested'],
    ] as const) {
      const outcome = await relayCatalog({ db, rawUrl: url });
      expect(outcome.ok && JSON.parse(outcome.body)).toEqual(catalog(name));
    }
  });

  test('refuses a quarantined or rejected source, which has not earned this origin', async () => {
    const db = await seeded();

    expect(await relayCatalog({ db, rawUrl: QUARANTINED })).toMatchObject({ ok: false, status: 404 });
    expect(await relayCatalog({ db, rawUrl: REJECTED })).toMatchObject({ ok: false, status: 404 });
  });

  test('a listed source with no readable catalog has nothing to relay', async () => {
    const db = await seeded();
    await seedUnreadable(db, 'https://broken.example/catalog.json', 'listed', 'down');

    expect(await relayCatalog({ db, rawUrl: 'https://broken.example/catalog.json' })).toMatchObject({ status: 404 });
  });

  test('refuses a url nobody submitted, so this is not an open fetch relay', async () => {
    expect(await relayCatalog({ db: await seeded(), rawUrl: 'https://nobody.example/catalog.json' })).toMatchObject({
      ok: false,
      status: 404,
    });
  });

  test('refuses a non-https url', async () => {
    expect(await relayCatalog({ db: await seeded(), rawUrl: 'http://listed.example/catalog.json' })).toMatchObject({
      ok: false,
      status: 400,
    });
  });

  test('matches the allowlist on the normalized url, not the raw one', async () => {
    expect((await relayCatalog({ db: await seeded(), rawUrl: `${LISTED}#frag` })).ok).toBe(true);
  });

  test('requires ?url=', async () => {
    expect(await relayCatalog({ db: await seeded(), rawUrl: null })).toMatchObject({ ok: false, status: 400 });
  });
});
