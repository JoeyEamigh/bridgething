import { describe, expect, test } from 'bun:test';
import type { Env } from './env.ts';
import { fakeKv, type FakeKv } from './kv-fake.ts';
import {
  createJudge,
  hashToken,
  judgeKeyFor,
  listJudges,
  normalizeHandle,
  principal,
  principalLabel,
  revokeJudge,
  scoringHandle,
  tokenMatches,
} from './principal.ts';

const ADMIN_TOKEN = 'admin-secret';
const NOW = '2026-08-01T00:00:00.000Z';

function env(kv: FakeKv): Env {
  return { SOURCES: kv as unknown as KVNamespace, ASSETS: {} as Fetcher, ADMIN_TOKEN };
}

function bearing(token: string | null): Request {
  return new Request('https://bridgething.com/api/jam/review', {
    headers: token === null ? {} : { authorization: `Bearer ${token}` },
  });
}

describe('tokenMatches', () => {
  test('an unset admin token never matches, not even the empty string', () => {
    expect(tokenMatches('', '')).toBe(false);
    expect(tokenMatches('anything', '')).toBe(false);
  });

  test('a token matches only itself', () => {
    expect(tokenMatches(ADMIN_TOKEN, ADMIN_TOKEN)).toBe(true);
    expect(tokenMatches('admin-secre', ADMIN_TOKEN)).toBe(false);
    expect(tokenMatches('admin-secretx', ADMIN_TOKEN)).toBe(false);
  });
});

describe('normalizeHandle', () => {
  test('drops a leading at sign and keeps display case', () => {
    expect(normalizeHandle('@ItsRiprod')).toBe('ItsRiprod');
  });

  test('refuses anything that would break a score key', () => {
    expect(normalizeHandle('has:colon')).toBeNull();
    expect(normalizeHandle('has space')).toBeNull();
    expect(normalizeHandle('')).toBeNull();
    expect(normalizeHandle('@')).toBeNull();
    expect(normalizeHandle('x'.repeat(41))).toBeNull();
  });
});

describe('principal', () => {
  test('no authorization header resolves to nobody', async () => {
    expect(await principal(bearing(null), env(fakeKv()))).toBeNull();
  });

  test('the admin token resolves to admin', async () => {
    expect(await principal(bearing(ADMIN_TOKEN), env(fakeKv()))).toEqual({ role: 'admin' });
  });

  test('an unknown token resolves to nobody', async () => {
    expect(await principal(bearing('nope'), env(fakeKv()))).toBeNull();
  });

  test('a judge token resolves to that judge handle', async () => {
    const kv = fakeKv();
    const created = await createJudge({ kv, rawHandle: '@espeon', now: NOW });
    expect(created.ok).toBe(true);
    if (!created.ok) return;

    expect(await principal(bearing(created.token), env(kv))).toEqual({ role: 'judge', handle: 'espeon' });
  });

  test('a revoked judge token stops resolving', async () => {
    const kv = fakeKv();
    const created = await createJudge({ kv, rawHandle: 'espeon', now: NOW });
    if (!created.ok) return;

    await revokeJudge({ kv, rawHandle: 'espeon' });
    expect(await principal(bearing(created.token), env(kv))).toBeNull();
  });

  test('labels and scoring handles follow the role', () => {
    expect(principalLabel({ role: 'admin' })).toBe('admin');
    expect(principalLabel({ role: 'judge', handle: '68p' })).toBe('68p');
    expect(scoringHandle({ role: 'admin' })).toBeNull();
    expect(scoringHandle({ role: 'judge', handle: '68p' })).toBe('68p');
  });
});

describe('judges', () => {
  test('only the hash of the token is stored', async () => {
    const kv = fakeKv();
    const created = await createJudge({ kv, rawHandle: 'lmore377', now: NOW });
    expect(created.ok).toBe(true);
    if (!created.ok) return;

    const stored = kv.snapshot();
    expect(Object.keys(stored)).toEqual([judgeKeyFor(await hashToken(created.token))]);
    expect(JSON.stringify(stored)).not.toContain(created.token);
    expect(created.token).toHaveLength(64);
  });

  test('two judges never get the same token', async () => {
    const kv = fakeKv();
    const first = await createJudge({ kv, rawHandle: 'a', now: NOW });
    const second = await createJudge({ kv, rawHandle: 'b', now: NOW });
    expect(first.ok && second.ok && first.token !== second.token).toBe(true);
  });

  test('a handle cannot be taken twice, whatever its case', async () => {
    const kv = fakeKv();
    await createJudge({ kv, rawHandle: 'ItsRiprod', now: NOW });
    const again = await createJudge({ kv, rawHandle: 'itsriprod', now: NOW });

    expect(again.ok).toBe(false);
    if (again.ok) return;
    expect(again.status).toBe(409);
  });

  test('a handle that is not a handle is refused', async () => {
    const outcome = await createJudge({ kv: fakeKv(), rawHandle: 'not a handle', now: NOW });
    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(400);
  });

  test('the list comes back sorted by handle', async () => {
    const kv = fakeKv();
    await createJudge({ kv, rawHandle: 'zed', now: NOW });
    await createJudge({ kv, rawHandle: 'anna', now: NOW });

    expect((await listJudges(kv)).map(judge => judge.handle)).toEqual(['anna', 'zed']);
  });

  test('revoking somebody who is not a judge is a 404', async () => {
    const outcome = await revokeJudge({ kv: fakeKv(), rawHandle: 'ghost' });
    expect(outcome.ok).toBe(false);
    if (outcome.ok) return;
    expect(outcome.status).toBe(404);
  });
});
