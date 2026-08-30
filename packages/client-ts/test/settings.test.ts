import { beforeEach, describe, expect, test } from 'bun:test';

import type { AuthorizeErrorKind } from '../src/settings.js';

type HostRequest = { id: number; verb: string; payload?: Record<string, unknown> };
type Handler = (request: HostRequest) => unknown;

let handle: Handler = () => {
  throw new Error('the fake host has no handler for this test');
};
const seen: HostRequest[] = [];

const host = {
  addEventListener: () => {},
  ReactNativeWebView: {
    postMessage(json: string) {
      const request = JSON.parse(json) as HostRequest;
      seen.push(request);
      void Promise.resolve()
        .then(() => handle(request))
        .then(
          value => answer({ id: request.id, ok: true, value }),
          err => answer({ id: request.id, ok: false, error: err instanceof Error ? err.message : String(err) }),
        );
    },
  },
} as unknown as Window & typeof globalThis;

function answer(payload: unknown): void {
  host.__bridgethingSettingsDeliver?.(JSON.stringify(payload));
}

globalThis.window = host;

const { settings, AuthorizeError, SettingsFetchError } = await import('../src/settings.js');

beforeEach(() => {
  seen.length = 0;
  handle = () => {
    throw new Error('the fake host has no handler for this test');
  };
});

describe('settings.fetch', () => {
  test('sends url, method, headers and a text body, and rebuilds the reply as a Response', async () => {
    handle = () => ({
      status: 201,
      headers: [
        ['content-type', 'application/json'],
        ['x-request-id', 'abc'],
      ],
      body: { kind: 'text', data: '{"id":7}' },
    });

    const response = await settings.fetch('https://api.example.test/things', {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: 'Bearer t' },
      body: '{"name":"thing"}',
    });

    const request = seen[0];
    expect(request.verb).toBe('fetch');
    expect(request.payload).toMatchObject({
      url: 'https://api.example.test/things',
      method: 'POST',
      body: { kind: 'text', data: '{"name":"thing"}' },
    });
    expect(request.payload!.headers).toContainEqual(['authorization', 'Bearer t']);

    expect(response.status).toBe(201);
    expect(response.headers.get('x-request-id')).toBe('abc');
    expect(await response.json()).toEqual({ id: 7 });
  });

  test('carries a binary request body as base64 and decodes a base64 reply', async () => {
    const payload = new Uint8Array([0, 1, 2, 250, 251, 252]);
    handle = () => ({
      status: 200,
      headers: [['content-type', 'application/octet-stream']],
      body: { kind: 'base64', data: 'AAECAA==' },
    });

    const response = await settings.fetch('https://api.example.test/blob', {
      method: 'PUT',
      headers: { 'content-type': 'application/octet-stream' },
      body: payload,
    });

    expect(seen[0].payload!.body).toEqual({ kind: 'base64', data: 'AAEC+vv8' });
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(new Uint8Array([0, 1, 2, 0]));
  });

  test('forwards an explicit timeout and builds a null-body response for 204', async () => {
    handle = () => ({ status: 204, headers: [], body: { kind: 'text', data: '' } });

    const response = await settings.fetch('https://api.example.test/gone', { method: 'DELETE', timeoutMs: 30_000 });

    expect(seen[0].payload!.timeoutMs).toBe(30_000);
    expect(response.status).toBe(204);
    expect(response.body).toBeNull();
  });

  test('maps a host network failure to a typed error', async () => {
    handle = () => {
      throw new Error('network: no route to host');
    };

    const failure = await settings.fetch('https://api.example.test/down').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(SettingsFetchError);
    expect((failure as InstanceType<typeof SettingsFetchError>).kind).toBe('network');
  });

  test('rejects an unparseable url before it reaches the host', async () => {
    const failure = await settings.fetch('not a url').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(SettingsFetchError);
    expect((failure as InstanceType<typeof SettingsFetchError>).kind).toBe('invalid_url');
    expect(seen).toHaveLength(0);
  });

  test('refuses a request body over the one mebibyte cap', async () => {
    const oversized = new Uint8Array(1024 * 1024 + 1);

    const failure = await settings
      .fetch('https://api.example.test/big', { method: 'POST', body: oversized })
      .catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(SettingsFetchError);
    expect((failure as InstanceType<typeof SettingsFetchError>).kind).toBe('network');
    expect((failure as Error).message).toContain('over the 1048576 byte cap');
    expect(seen).toHaveLength(0);
  });
});

describe('settings.installFetch', () => {
  test('routes the global fetch through the host and restores the original', async () => {
    handle = () => ({
      status: 200,
      headers: [['content-type', 'text/plain']],
      body: { kind: 'text', data: 'through' },
    });
    const original = globalThis.fetch;

    const restore = settings.installFetch();
    expect(globalThis.fetch).not.toBe(original);
    expect(await (await fetch('https://api.example.test/hello')).text()).toBe('through');

    restore();
    expect(globalThis.fetch).toBe(original);
  });
});

describe('settings.auth.authorize', () => {
  test('resolves to the callback url the host returns', async () => {
    handle = () => ({ url: 'bridgething://oauth/callback?code=xyz&state=s1' });

    const callback = await settings.auth.authorize('https://provider.test/authorize?client_id=1');

    expect(seen[0].verb).toBe('auth.authorize');
    expect(seen[0].payload).toEqual({ url: 'https://provider.test/authorize?client_id=1' });
    expect(callback.searchParams.get('code')).toBe('xyz');
    expect(callback.searchParams.get('state')).toBe('s1');
  });

  const failures: [AuthorizeErrorKind, string][] = [
    ['cancelled', 'cancelled'],
    ['busy', 'busy: an authorization is already in flight'],
    ['unsupported', 'unsupported'],
  ];

  test.each(failures)('surfaces %s as a typed error', async (kind, message) => {
    handle = () => {
      throw new Error(message);
    };

    const failure = await settings.auth.authorize('https://provider.test/authorize').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(AuthorizeError);
    expect((failure as InstanceType<typeof AuthorizeError>).kind).toBe(kind);
  });

  test('reads a host that does not know the verb as unsupported', async () => {
    handle = () => {
      throw new Error('unknown settings bridge verb: auth.authorize');
    };

    const failure = await settings.auth.authorize('https://provider.test/authorize').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(AuthorizeError);
    expect((failure as InstanceType<typeof AuthorizeError>).kind).toBe('unsupported');
  });
});
