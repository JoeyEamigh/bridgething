import { describe, expect, test } from 'bun:test';
import { isAdmin, tokenMatches } from './auth.ts';

function request(authorization?: string): Request {
  return new Request('https://bridgething.com/api/admin/sources', {
    headers: authorization === undefined ? {} : { authorization },
  });
}

describe('tokenMatches', () => {
  test('an unset admin token never matches, not even the empty string', () => {
    expect(tokenMatches('', '')).toBe(false);
  });

  test('a token matches only itself', () => {
    expect(tokenMatches('secret', 'secret')).toBe(true);
    expect(tokenMatches('secret', 'secreT')).toBe(false);
    expect(tokenMatches('secre', 'secret')).toBe(false);
  });
});

describe('isAdmin', () => {
  test('needs the admin token as a bearer', () => {
    expect(isAdmin(request('Bearer secret'), 'secret')).toBe(true);
    expect(isAdmin(request('secret'), 'secret')).toBe(false);
    expect(isAdmin(request(), 'secret')).toBe(false);
  });

  test('a worker with no admin token configured admits nobody', () => {
    expect(isAdmin(request('Bearer '), undefined)).toBe(false);
    expect(isAdmin(request('Bearer x'), '')).toBe(false);
  });
});
