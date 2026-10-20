import { beforeEach, describe, expect, test } from 'bun:test';

import type { SettingsContext, SettingsHost, WebappBlobEntry } from '../src/settings-protocol.js';
import type { AuthorizeErrorKind, BlobErrorKind } from '../src/settings.js';

import { bridge, deliver, page, resetBridge, session } from './host.js';

function contextFor(webappId: string): SettingsContext {
  return { webappId, name: webappId, version: '1.0.0', deviceId: 'd' };
}

function hostWith(verbs: Partial<SettingsHost>): SettingsHost {
  const refuse = () => Promise.reject(new Error('unused in this test'));
  return new Proxy({ done: () => undefined, ...verbs } as SettingsHost, {
    get: (target, verb) => (verb in target ? target[verb as keyof SettingsHost] : refuse),
  });
}

const { settings, AuthorizeError, BlobError, SettingsFetchError } = await import('../src/settings.js');

beforeEach(resetBridge);

describe('settings.fetch', () => {
  test('sends url, method, headers and a text body, and rebuilds the reply as a Response', async () => {
    bridge.handle = () => ({
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

    const request = bridge.seen[0];
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
    bridge.handle = () => ({
      status: 200,
      headers: [['content-type', 'application/octet-stream']],
      body: { kind: 'base64', data: 'AAECAA==' },
    });

    const response = await settings.fetch('https://api.example.test/blob', {
      method: 'PUT',
      headers: { 'content-type': 'application/octet-stream' },
      body: payload,
    });

    expect(bridge.seen[0].payload!.body).toEqual({ kind: 'base64', data: 'AAEC+vv8' });
    expect(new Uint8Array(await response.arrayBuffer())).toEqual(new Uint8Array([0, 1, 2, 0]));
  });

  test('forwards an explicit timeout and builds a null-body response for 204', async () => {
    bridge.handle = () => ({ status: 204, headers: [], body: { kind: 'text', data: '' } });

    const response = await settings.fetch('https://api.example.test/gone', { method: 'DELETE', timeoutMs: 30_000 });

    expect(bridge.seen[0].payload!.timeoutMs).toBe(30_000);
    expect(response.status).toBe(204);
    expect(response.body).toBeNull();
  });

  test('maps a host network failure to a typed error', async () => {
    bridge.handle = () => {
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
    expect(bridge.seen).toHaveLength(0);
  });

  test('refuses a request body over the one mebibyte cap', async () => {
    const oversized = new Uint8Array(1024 * 1024 + 1);

    const failure = await settings
      .fetch('https://api.example.test/big', { method: 'POST', body: oversized })
      .catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(SettingsFetchError);
    expect((failure as InstanceType<typeof SettingsFetchError>).kind).toBe('network');
    expect((failure as Error).message).toContain('over the 1048576 byte cap');
    expect(bridge.seen).toHaveLength(0);
  });
});

describe('settings.installFetch', () => {
  test('routes the global fetch through the host and restores the original', async () => {
    bridge.handle = () => ({
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
    bridge.handle = () => ({ url: 'bridgething://oauth/callback?code=xyz&state=s1' });

    const callback = await settings.auth.authorize('https://provider.test/authorize?client_id=1');

    expect(bridge.seen[0].verb).toBe('auth.authorize');
    expect(bridge.seen[0].payload).toEqual({ url: 'https://provider.test/authorize?client_id=1' });
    expect(callback.searchParams.get('code')).toBe('xyz');
    expect(callback.searchParams.get('state')).toBe('s1');
  });

  const failures: [AuthorizeErrorKind, string][] = [
    ['cancelled', 'cancelled'],
    ['busy', 'busy: an authorization is already in flight'],
    ['unsupported', 'unsupported'],
  ];

  test.each(failures)('surfaces %s as a typed error', async (kind, message) => {
    bridge.handle = () => {
      throw new Error(message);
    };

    const failure = await settings.auth.authorize('https://provider.test/authorize').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(AuthorizeError);
    expect((failure as InstanceType<typeof AuthorizeError>).kind).toBe(kind);
  });

  test('reads a host that does not know the verb as unsupported', async () => {
    bridge.handle = () => {
      throw new Error('unknown settings bridge verb: auth.authorize');
    };

    const failure = await settings.auth.authorize('https://provider.test/authorize').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(AuthorizeError);
    expect((failure as InstanceType<typeof AuthorizeError>).kind).toBe('unsupported');
  });
});

describe('settings.pick', () => {
  test('asks the host for a kind and a cover box and hands back what the user picked', async () => {
    const picked = {
      handle: 'h1',
      mime: 'image/gif',
      size: 4096,
      width: 800,
      height: 600,
      preview: 'data:image/jpeg;base64,AA==',
    };
    bridge.handle = () => picked;

    expect(await settings.pick({ kind: 'image', cover: [800, 480] })).toEqual(picked);
    expect(bridge.seen[0]).toMatchObject({ verb: 'pick', payload: { kind: 'image', cover: [800, 480] } });
  });

  test('resolves null when the user dismisses the picker', async () => {
    bridge.handle = () => null;

    expect(await settings.pick({ kind: 'any' })).toBeNull();
    expect(bridge.seen[0].payload).toEqual({ kind: 'any' });
  });

  test('reads a host that does not know the verb as unsupported', async () => {
    bridge.handle = () => {
      throw new Error('unknown settings bridge verb: pick');
    };

    const failure = await settings.pick({ kind: 'video' }).catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(BlobError);
    expect((failure as InstanceType<typeof BlobError>).kind).toBe('unsupported');
  });

  const failures: [BlobErrorKind, string][] = [
    ['busy', 'busy: a picker is already open'],
    ['no_gesture', 'no_gesture: a browser opens its file picker only in answer to a tap or click'],
    ['not_media', 'not_media: the picked file is not a video'],
    ['unsupported', 'unsupported: this companion cannot transcode video'],
    ['disconnected', 'not_connected: no link to the device'],
    ['failed', 'settings bridge pick cover edges must be whole numbers from 1 to 4294967295'],
  ];

  test.each(failures)('surfaces %s as a typed error', async (kind, message) => {
    bridge.handle = () => {
      throw new Error(message);
    };

    const failure = await settings.pick({ kind: 'video' }).catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(BlobError);
    expect((failure as InstanceType<typeof BlobError>).kind).toBe(kind);
    expect((failure as Error).message).toBe(message);
  });
});

describe('settings.blob', () => {
  test('streams page-made bytes to a handle in base64 chunks and puts it under a key', async () => {
    const received: Uint8Array[] = [];
    let size = 0;
    bridge.handle = request => {
      const payload = request.payload ?? {};
      if (request.verb === 'blob.create') return { handle: 'h7' };
      if (request.verb === 'blob.append') {
        const chunk = Uint8Array.from(atob(String(payload.data)), char => char.charCodeAt(0));
        received.push(chunk);
        size += chunk.length;
        return { handle: payload.handle, size };
      }
      if (request.verb === 'blob.put') return { key: payload.key, sha256: 'ab'.repeat(32), mime: 'image/png', size };
      throw new Error(`unexpected ${request.verb}`);
    };
    const body = Uint8Array.from({ length: 600 * 1024 }, (_, at) => at % 251);

    const handle = await settings.blob.create('image/png');
    const total = await settings.blob.append(handle, new Blob([body]));
    const stored = await settings.blob.put('canvas', handle);

    expect(bridge.seen.map(request => request.verb)).toEqual([
      'blob.create',
      'blob.append',
      'blob.append',
      'blob.append',
      'blob.put',
    ]);
    expect(bridge.seen[0].payload).toEqual({ mime: 'image/png' });
    expect(received.map(chunk => chunk.length)).toEqual([256 * 1024, 256 * 1024, 88 * 1024]);
    expect(Uint8Array.from(received.flatMap(chunk => [...chunk]))).toEqual(body);
    expect(total).toBe(body.length);
    expect(stored).toEqual({ key: 'canvas', sha256: 'ab'.repeat(32), mime: 'image/png', size: body.length });
  });

  test('takes an array buffer view and only sends its own bytes', async () => {
    const sent: string[] = [];
    bridge.handle = request => {
      sent.push(String(request.payload?.data));
      return { handle: 'h', size: 3 };
    };
    const backing = new Uint8Array([9, 1, 2, 3, 9]);

    await settings.blob.append('h', backing.subarray(1, 4));

    expect(sent).toEqual([btoa(String.fromCharCode(1, 2, 3))]);
  });

  test('an empty append still answers with the size the handle already holds', async () => {
    bridge.handle = request => ({ handle: request.payload?.handle, size: 4096 });

    const size = await settings.blob.append('h', new Uint8Array(0));

    expect(size).toBe(4096);
    expect(bridge.seen.map(request => [request.verb, request.payload])).toEqual([
      ['blob.append', { handle: 'h', data: '' }],
    ]);
  });

  test('lists and deletes the keys the device holds for this app', async () => {
    const entry = { key: 'bg', sha256: 'cd'.repeat(32), mime: 'video/mp4', size: 10 };
    bridge.handle = request => (request.verb === 'blob.list' ? [entry] : { key: 'bg' });

    expect(await settings.blob.list()).toEqual([entry]);
    await settings.blob.delete('bg');
    expect(bridge.seen.map(request => [request.verb, request.payload])).toEqual([
      ['blob.list', undefined],
      ['blob.delete', { key: 'bg' }],
    ]);
  });

  const failures: [BlobErrorKind, string][] = [
    ['too_large', 'too_large: bg is over the 33554432 byte cap'],
    ['quota_exceeded', 'quota_exceeded: bg would take the webapp over its 67108864 byte quota'],
    ['storage_full', 'storage_full: the device has no room for the 1024 bytes bg needs'],
    ['disconnected', 'link_lost: the link to the device dropped mid-upload'],
    ['disconnected', 'not_connected: no link to the device'],
    ['not_media', 'not_media: the picked file is not an image'],
    ['unsupported', 'unsupported: this companion cannot transcode video'],
    ['unknown_handle', 'unknown_handle: h is not a live handle'],
    ['busy', 'busy: a picker is already open'],
    ['failed', 'transfer_failed: digest mismatch'],
    ['failed', 'something nobody planned for'],
  ];

  test.each(failures)('surfaces %s as a typed error', async (kind, message) => {
    bridge.handle = () => {
      throw new Error(message);
    };

    const failure = await settings.blob.put('bg', 'h').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(BlobError);
    expect((failure as InstanceType<typeof BlobError>).kind).toBe(kind);
    expect((failure as Error).message).toBe(message);
  });

  test('reads a host that does not know the verbs as unsupported', async () => {
    bridge.handle = request => {
      throw new Error(`unknown settings bridge verb: ${request.verb}`);
    };

    const failure = await settings.blob.create('image/png').catch((err: unknown) => err);

    expect(failure).toBeInstanceOf(BlobError);
    expect((failure as InstanceType<typeof BlobError>).kind).toBe('unsupported');
  });

  test('a put hears the progress of its own upload and nothing else', async () => {
    let releaseOther: (value: WebappBlobEntry) => void = () => undefined;
    bridge.host = hostWith({
      'blob.put': ({ key }, report) => {
        if (key === 'other') return new Promise<WebappBlobEntry>(resolve => (releaseOther = resolve));
        report({ sent: 0, total: 10 });
        report({ sent: 4, total: 10 });
        report({ sent: 10, total: 10 });
        return Promise.resolve({ key, sha256: 'ab'.repeat(32), mime: 'video/mp4', size: 10 });
      },
    });
    const mine: Array<[number, number]> = [];
    const theirs: Array<[number, number]> = [];

    const other = settings.blob.put('other', 'h2', { onProgress: (sent, total) => theirs.push([sent, total]) });
    const stored = await settings.blob.put('bg', 'h1', { onProgress: (sent, total) => mine.push([sent, total]) });
    releaseOther({ key: 'other', sha256: 'cd'.repeat(32), mime: 'video/mp4', size: 1 });
    await other;

    expect(stored.key).toBe('bg');
    expect(mine).toEqual([
      [0, 10],
      [4, 10],
      [10, 10],
    ]);
    expect(theirs).toEqual([]);
  });

  test('a put waits on the host however long the link takes', async () => {
    let settle: (value: unknown) => void = () => undefined;
    bridge.handle = () => new Promise(resolve => (settle = resolve));
    const realTimeout = globalThis.setTimeout;
    const armed: number[] = [];
    globalThis.setTimeout = ((handler: () => void, ms?: number) => {
      armed.push(ms ?? 0);
      return realTimeout(handler, 0);
    }) as typeof setTimeout;

    const putting = settings.blob.put('bg', 'h');
    globalThis.setTimeout = realTimeout;
    await new Promise(resolve => realTimeout(resolve, 5));
    settle({ key: 'bg', sha256: 'ef'.repeat(32), mime: 'video/mp4', size: 1 });

    expect((await putting).key).toBe('bg');
    expect(armed).toEqual([]);
  });
});

describe('a page the host points at another app', () => {
  const listing = hostWith({
    context: () => Promise.resolve(contextFor('current')),
    'config.list': () => Promise.resolve([]),
  });

  test('stamps what it sends with the retarget it last heard of', async () => {
    bridge.host = listing;
    await settings.config.list();
    const before = bridge.seen.at(-1)!.epoch!;

    session.retarget('browser', contextFor('browser'));
    await settings.config.list();
    settings.done();

    expect(bridge.seen.at(-2)).toMatchObject({ verb: 'config.list', epoch: before + 1 });
    expect(bridge.seen.at(-1)).toMatchObject({ verb: 'done', epoch: before + 1 });
  });

  test('tells context listeners once per retarget and ignores one it already knows', async () => {
    bridge.host = listing;
    const heard: SettingsContext[] = [];
    const off = settings.onContextChanged(context => heard.push(context));

    session.retarget('browser', contextFor('browser'));
    await settings.config.list();
    const known = bridge.seen.at(-1)!.epoch!;
    deliver({ event: 'contextChanged', context: contextFor('late'), epoch: known });
    deliver({ event: 'contextChanged', context: contextFor('later'), epoch: known - 1 });
    off();

    expect(heard).toEqual([contextFor('browser')]);
  });

  test('keeps serving the app it shows until its next context reply tells it of a retarget it missed', async () => {
    bridge.host = listing;
    bridge.hosts.missed = hostWith({
      context: () => Promise.resolve(contextFor('browser')),
      'config.list': () => Promise.resolve([{ key: 'owner', value: 'browser' }]),
    });
    const heard: SettingsContext[] = [];
    const off = settings.onContextChanged(context => heard.push(context));
    await settings.config.list();
    const before = bridge.seen.at(-1)!.epoch!;

    const listening = page.__bridgethingSettingsDeliver;
    page.__bridgethingSettingsDeliver = undefined;
    session.retarget('missed', contextFor('browser'));
    page.__bridgethingSettingsDeliver = listening;

    const missed = await settings.config.list();
    const context = await settings.context();
    const entries = await settings.config.list();
    off();

    expect(missed).toEqual([]);
    expect(context).toEqual(contextFor('browser'));
    expect(heard).toEqual([contextFor('browser')]);
    expect(entries).toEqual([{ key: 'owner', value: 'browser' }]);
    expect(bridge.seen.at(-1)!.epoch).toBe(before + 1);
  });

  test('a pinned config keeps writing to the app it was pinned on after a retarget', async () => {
    const written: Array<[string, string]> = [];
    bridge.host = hostWith({
      'config.set': ({ key, value }) => {
        written.push([`old ${key}`, value]);
        return Promise.resolve({ key, value });
      },
    });
    const pinned = settings.config.pin();
    const target = `pinned-${bridge.seen.length}`;
    bridge.hosts[target] = hostWith({
      'config.set': ({ key, value }) => {
        written.push([`new ${key}`, value]);
        return Promise.resolve({ key, value });
      },
    });

    session.retarget(target, contextFor(target));
    await pinned.set('zip', '48201');
    await settings.config.set('zip', '10001');
    pinned.release();

    expect(written).toEqual([
      ['old zip', '48201'],
      ['new zip', '10001'],
    ]);
  });

  test('tells the host to let go of the epoch it moved off when nothing is pinned to it', async () => {
    bridge.host = listing;
    await settings.config.list();
    const before = bridge.seen.at(-1)!.epoch!;

    session.retarget('browser', contextFor('browser'));
    await settings.config.list();
    const releases = bridge.seen.filter(request => request.verb === 'release');

    expect(releases.at(-1)).toMatchObject({ epoch: before });
    expect(bridge.seen.at(-1)).toMatchObject({ verb: 'config.list', epoch: before + 1 });
  });

  test('keeps a pinned epoch until its pin is released, then lets it go once', async () => {
    bridge.host = listing;
    await settings.config.list();
    const before = bridge.seen.at(-1)!.epoch!;
    const pinned = settings.config.pin();
    const twice = settings.config.pin();
    const released = () => bridge.seen.filter(request => request.verb === 'release' && request.epoch === before);

    session.retarget('browser', contextFor('browser'));
    await settings.config.list();
    expect(released()).toEqual([]);

    pinned.release();
    pinned.release();
    expect(released()).toEqual([]);

    twice.release();
    await settings.config.list();
    expect(released()).toHaveLength(1);
    expect(await settings.config.list()).toEqual([]);
  });

  test('releasing a pin on the epoch still on screen tells the host nothing', async () => {
    bridge.host = listing;
    await settings.config.list();
    const seen = bridge.seen.length;

    settings.config.pin().release();

    expect(bridge.seen.slice(seen).filter(request => request.verb === 'release')).toEqual([]);
  });

  test('tells context listeners which app the page showed before the retarget', async () => {
    bridge.host = listing;
    const heard: Array<[string, string | null]> = [];
    const off = settings.onContextChanged((context, previous) =>
      heard.push([context.webappId, previous?.webappId ?? null]),
    );

    session.retarget('weather', contextFor('weather'));
    session.notify({ event: 'refresh', context: { ...contextFor('weather'), version: '1.1.0' } });
    session.retarget('browser', contextFor('browser'));
    off();

    expect(heard.map(([context]) => context)).toEqual(['weather', 'browser']);
    expect(heard[1]).toEqual(['browser', 'weather']);
  });

  test('tells refresh listeners when the host shows the page again', () => {
    const heard: SettingsContext[] = [];
    const off = settings.onRefresh(context => heard.push(context));

    session.notify({ event: 'refresh', context: contextFor('weather') });
    off();
    session.notify({ event: 'refresh', context: contextFor('weather') });

    expect(heard).toEqual([contextFor('weather')]);
  });
});
