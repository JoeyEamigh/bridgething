import { describe, expect, test } from 'bun:test';
import { fakeD1 } from './d1-fake.ts';
import { pruneTokens, takeToken, type Limit } from './limits.ts';

const LIMIT: Limit = { bucket: 'test', limit: 2, windowSeconds: 3600, refusal: 'no' };
const OTHER: Limit = { ...LIMIT, bucket: 'other' };
const AT = '2026-07-24T12:10:00.000Z';
const NEXT_HOUR = '2026-07-24T13:00:00.000Z';

describe('takeToken', () => {
  test('allows up to the limit then refuses', async () => {
    const db = fakeD1();
    const taken = [];
    for (let n = 0; n < 3; n += 1) taken.push(await takeToken(db, LIMIT, 'client', AT));

    expect(taken).toEqual([true, true, false]);
  });

  test('counts each key separately', async () => {
    const db = fakeD1();
    await takeToken(db, LIMIT, 'a', AT);
    await takeToken(db, LIMIT, 'a', AT);

    expect(await takeToken(db, LIMIT, 'b', AT)).toBe(true);
  });

  test('counts each bucket separately', async () => {
    const db = fakeD1();
    await takeToken(db, LIMIT, 'a', AT);
    await takeToken(db, LIMIT, 'a', AT);

    expect(await takeToken(db, OTHER, 'a', AT)).toBe(true);
  });

  test('a new window starts the count over', async () => {
    const db = fakeD1();
    await takeToken(db, LIMIT, 'a', AT);
    await takeToken(db, LIMIT, 'a', AT);

    expect(await takeToken(db, LIMIT, 'a', NEXT_HOUR)).toBe(true);
    expect(await takeToken(db, LIMIT, 'a', NEXT_HOUR)).toBe(true);
    expect(await takeToken(db, LIMIT, 'a', NEXT_HOUR)).toBe(false);
  });
});

describe('pruneTokens', () => {
  test('drops windows long past and keeps the live ones', async () => {
    const db = fakeD1();
    await takeToken(db, LIMIT, 'old', '2026-07-20T00:00:00.000Z');
    await takeToken(db, LIMIT, 'live', AT);

    await pruneTokens(db, AT);

    expect((await db.prepare('SELECT key FROM rate_limits').all()).results).toEqual([{ key: 'live' }]);
  });
});
