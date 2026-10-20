import { beforeEach, describe, expect, test } from 'bun:test';

import {
  DEFAULT_SETTINGS_KEY,
  MAX_COVER_EDGE,
  NO_SETTINGS_TARGET,
  UNKNOWN_SETTINGS_EPOCH,
  UNKNOWN_VERB,
  decodeSettingsMessage,
  openSettingsSession,
  settingsPageKey,
  settingsSwitch,
  type SettingsContext,
  type SettingsHost,
  type SettingsMessage,
  type WebappBlobEntry,
} from '../src/settings-protocol.js';

import { bridge, resetBridge } from './host.js';

const { settings } = await import('../src/settings.js');

type Calls = Array<[string, unknown]>;

function recordingHost(calls: Calls): SettingsHost {
  const record =
    <T>(verb: string, value: T) =>
    (payload: unknown) => {
      calls.push([verb, payload]);
      return Promise.resolve(value);
    };
  return {
    context: record('context', { webappId: 'w', name: 'weather', version: '1.0.0', deviceId: 'd' }),
    'config.fields': record('config.fields', []),
    'config.list': record('config.list', [{ key: 'zip', value: '48201' }]),
    'config.set': record('config.set', { key: 'zip', value: '10001' }),
    'config.delete': record('config.delete', { key: 'zip', value: null }),
    'doc.get': record('doc.get', { key: 'layout', value: null }),
    'doc.list': record('doc.list', []),
    'doc.set': record('doc.set', { key: 'layout', value: '[]' }),
    'doc.delete': record('doc.delete', { key: 'layout', value: null }),
    fetch: record('fetch', { status: 200, headers: [], body: { kind: 'text', data: '' } }),
    'auth.authorize': record('auth.authorize', { url: 'bridgething://oauth/callback' }),
    pick: record('pick', null),
    'blob.create': record('blob.create', { handle: 'h' }),
    'blob.append': record('blob.append', { handle: 'h', size: 3 }),
    'blob.put': record('blob.put', { key: 'bg', sha256: 'ab'.repeat(32), mime: 'image/gif', size: 3 }),
    'blob.delete': record('blob.delete', { key: 'bg' }),
    'blob.list': record('blob.list', []),
    done: () => calls.push(['done', undefined]),
  };
}

function tick(): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, 0));
}

async function serve(raw: unknown, host: SettingsHost): Promise<SettingsMessage[]> {
  const delivered: SettingsMessage[] = [];
  openSettingsSession({ target: 'w', bind: () => host, deliver: message => delivered.push(message) }).receive(raw);
  await tick();
  return delivered;
}

function contextFor(webappId: string): SettingsContext {
  return { webappId, name: webappId, version: '1.0.0', deviceId: 'd' };
}

function appHost(webappId: string, calls: Calls = []): SettingsHost {
  return {
    ...recordingHost(calls),
    context: () => Promise.resolve(contextFor(webappId)),
    'config.list': () => Promise.resolve([{ key: 'owner', value: webappId }]),
    'config.set': ({ key, value }) => {
      calls.push([`${webappId} config.set`, { key, value }]);
      return Promise.resolve({ key, value });
    },
    done: () => calls.push([`${webappId} done`, undefined]),
  };
}

function request(id: number, verb: string, payload?: unknown, epoch?: number): string {
  return JSON.stringify({ id, verb, payload, epoch });
}

beforeEach(resetBridge);

describe('a settings session', () => {
  test('routes a verb to the host with a decoded payload and replies with its value', async () => {
    const calls: Calls = [];
    const replies = await serve(
      JSON.stringify({ id: 4, verb: 'config.set', payload: { key: 'zip', value: '10001', extra: true } }),
      recordingHost(calls),
    );

    expect(calls).toEqual([['config.set', { key: 'zip', value: '10001' }]]);
    expect(replies).toEqual([{ id: 4, ok: true, value: { key: 'zip', value: '10001' } }]);
  });

  test('refuses a payload missing a required field without calling the host', async () => {
    const calls: Calls = [];
    const replies = await serve(
      JSON.stringify({ id: 5, verb: 'doc.set', payload: { key: 'layout' } }),
      recordingHost(calls),
    );

    expect(calls).toEqual([]);
    expect(replies).toEqual([{ id: 5, ok: false, error: 'settings bridge payload needs a string value' }]);
  });

  test('answers an unknown verb with the error the page reads as a missing verb', async () => {
    const replies = await serve(JSON.stringify({ id: 6, verb: 'camera.capture' }), recordingHost([]));

    expect(replies).toEqual([{ id: 6, ok: false, error: `${UNKNOWN_VERB}: camera.capture` }]);
  });

  test('decodes a pick and refuses a kind or cover box it cannot honor', async () => {
    const calls: Calls = [];
    const host = recordingHost(calls);
    const sent = (payload: unknown, id: number) => serve(JSON.stringify({ id, verb: 'pick', payload }), host);

    await sent({ kind: 'image', cover: [800, 480], extra: 1 }, 1);
    await sent({ kind: 'any', cover: null }, 2);
    const refused = [
      ...(await sent({ kind: 'camera' }, 3)),
      ...(await sent({ kind: 'video', cover: [800] }, 4)),
      ...(await sent({ kind: 'video', cover: [0, 480] }, 5)),
      ...(await sent({ kind: 'video', cover: [12.5, 480] }, 6)),
    ];

    expect(calls).toEqual([
      ['pick', { kind: 'image', cover: [800, 480] }],
      ['pick', { kind: 'any' }],
    ]);
    expect(refused.map(reply => ('ok' in reply ? [reply.id, reply.ok] : null))).toEqual([
      [3, false],
      [4, false],
      [5, false],
      [6, false],
    ]);
  });

  test('takes a cover edge up to the largest a u32 holds and refuses anything wider by name', async () => {
    const calls: Calls = [];
    const host = recordingHost(calls);
    const sent = (cover: unknown, id: number) =>
      serve(JSON.stringify({ id, verb: 'pick', payload: { kind: 'image', cover } }), host);

    await sent([MAX_COVER_EDGE, MAX_COVER_EDGE], 1);
    const refused = [...(await sent([MAX_COVER_EDGE + 1, 480], 2)), ...(await sent([800, Number.MAX_SAFE_INTEGER], 3))];

    expect(MAX_COVER_EDGE).toBe(4294967295);
    expect(calls).toEqual([['pick', { kind: 'image', cover: [MAX_COVER_EDGE, MAX_COVER_EDGE] }]]);
    expect(refused).toEqual([
      { id: 2, ok: false, error: 'settings bridge pick cover edges must be whole numbers from 1 to 4294967295' },
      { id: 3, ok: false, error: 'settings bridge pick cover edges must be whole numbers from 1 to 4294967295' },
    ]);
  });

  test('passes blob payloads through with only the fields each verb takes', async () => {
    const calls: Calls = [];
    const host = recordingHost(calls);

    await serve(JSON.stringify({ id: 1, verb: 'blob.create', payload: { mime: 'image/png', size: 9 } }), host);
    await serve(JSON.stringify({ id: 2, verb: 'blob.append', payload: { handle: 'h', data: 'AAE=' } }), host);
    await serve(JSON.stringify({ id: 3, verb: 'blob.put', payload: { key: 'bg', handle: 'h' } }), host);
    await serve(JSON.stringify({ id: 4, verb: 'blob.delete', payload: { key: 'bg' } }), host);
    await serve(JSON.stringify({ id: 5, verb: 'blob.list' }), host);
    const refused = await serve(JSON.stringify({ id: 6, verb: 'blob.append', payload: { handle: 'h' } }), host);

    expect(calls).toEqual([
      ['blob.create', { mime: 'image/png' }],
      ['blob.append', { handle: 'h', data: 'AAE=' }],
      ['blob.put', { key: 'bg', handle: 'h' }],
      ['blob.delete', { key: 'bg' }],
      ['blob.list', undefined],
    ]);
    expect(refused).toEqual([{ id: 6, ok: false, error: 'settings bridge payload needs a string data' }]);
  });

  test('a verb reports progress tagged with its request id before it replies', async () => {
    const host = {
      ...recordingHost([]),
      'blob.put': (_payload: unknown, report: (progress: { sent: number; total: number }) => void) => {
        report({ sent: 0, total: 8 });
        report({ sent: 8, total: 8 });
        return Promise.resolve({ key: 'bg', sha256: 'ab'.repeat(32), mime: 'image/gif', size: 8 });
      },
    };

    const delivered = await serve(
      JSON.stringify({ id: 9, verb: 'blob.put', payload: { key: 'bg', handle: 'h' } }),
      host,
    );

    expect(delivered).toEqual([
      { event: 'progress', id: 9, sent: 0, total: 8 },
      { event: 'progress', id: 9, sent: 8, total: 8 },
      { id: 9, ok: true, value: { key: 'bg', sha256: 'ab'.repeat(32), mime: 'image/gif', size: 8 } },
    ]);
  });

  test('treats done as a fire-and-forget close', async () => {
    const calls: Calls = [];
    const replies = await serve(JSON.stringify({ id: 7, verb: 'done' }), recordingHost(calls));

    expect(calls).toEqual([['done', undefined]]);
    expect(replies).toEqual([]);
  });

  test('ignores anything that is not a request', async () => {
    const calls: Calls = [];
    const host = recordingHost(calls);

    const replies = [
      ...(await serve('not json', host)),
      ...(await serve(JSON.stringify({ verb: 'context' }), host)),
      ...(await serve(JSON.stringify({ id: 'x', verb: 'context' }), host)),
      ...(await serve({ id: 1, verb: 'context' }, host)),
    ];

    expect(calls).toEqual([]);
    expect(replies).toEqual([]);
  });

  test('reports a host failure through the supplied explanation', async () => {
    const host = { ...recordingHost([]), 'config.list': () => Promise.reject(new Error('gone')) };
    const delivered: SettingsMessage[] = [];
    openSettingsSession({
      target: 'w',
      bind: () => host,
      deliver: message => delivered.push(message),
      explain: () => 'explained',
    }).receive(JSON.stringify({ id: 8, verb: 'config.list' }));
    await tick();

    expect(delivered).toEqual([{ id: 8, ok: false, error: 'explained' }]);
  });

  test('keeps a reply and its progress away from the document that replaced the one that asked', async () => {
    let finish: (entry: WebappBlobEntry) => void = () => undefined;
    let tell: (progress: { sent: number; total: number }) => void = () => undefined;
    const slow: SettingsHost = {
      ...recordingHost([]),
      'blob.put': (_payload, report) => {
        tell = report;
        return new Promise(resolve => (finish = resolve));
      },
    };
    const first: SettingsMessage[] = [];
    const second: SettingsMessage[] = [];
    const before = openSettingsSession({ target: 'w', bind: () => slow, deliver: message => first.push(message) });

    before.receive(request(4, 'blob.put', { key: 'bg', handle: 'h1' }));
    await tick();
    before.close();
    const after = openSettingsSession({
      target: 'w',
      bind: () => recordingHost([]),
      deliver: message => second.push(message),
    });
    tell({ sent: 4, total: 8 });
    finish({ key: 'bg', sha256: 'ab'.repeat(32), mime: 'video/mp4', size: 8 });
    await tick();

    expect(before.closed).toBe(true);
    expect(after.closed).toBe(false);
    expect(first).toEqual([]);
    expect(second).toEqual([]);
  });

  test('drops a request, a done and a notice that reach a closed session', async () => {
    const calls: Calls = [];
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'w',
      bind: () => recordingHost(calls),
      deliver: message => delivered.push(message),
    });

    session.close();
    session.receive(request(1, 'config.set', { key: 'zip', value: '10001' }));
    session.receive(request(2, 'done'));
    session.notify({ event: 'docChanged', key: 'layout', value: '[]' });
    session.retarget('other', contextFor('other'));
    await tick();

    expect(calls).toEqual([]);
    expect(delivered).toEqual([]);
    expect(session.target).toBe('w');
  });

  test('serves a request against the app it arrived for after the page is pointed at another', async () => {
    let release: () => void = () => undefined;
    const calls: Calls = [];
    const hosts: Record<string, SettingsHost> = {
      weather: {
        ...appHost('weather', calls),
        'config.list': () => new Promise(resolve => (release = () => resolve([{ key: 'owner', value: 'weather' }]))),
      },
      browser: appHost('browser', calls),
    };
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'weather',
      bind: target => hosts[target],
      deliver: message => delivered.push(message),
    });

    session.receive(request(1, 'config.list'));
    session.retarget('browser', contextFor('browser'));
    release();
    await tick();

    expect(session.target).toBe('browser');
    expect(delivered).toEqual([
      { event: 'contextChanged', context: contextFor('browser'), epoch: 1 },
      { id: 1, ok: true, value: [{ key: 'owner', value: 'weather' }] },
    ]);
  });

  test('serves a request against the app its epoch named, and honors done only for the app on screen', async () => {
    const calls: Calls = [];
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'weather',
      bind: target => appHost(target, calls),
      deliver: message => delivered.push(message),
    });

    session.retarget('browser', contextFor('browser'));
    session.receive(request(2, 'config.set', { key: 'zip', value: '48201' }, 0));
    session.receive(request(3, 'config.set', { key: 'zip', value: '10001' }));
    session.receive(request(4, 'done', undefined, 0));
    session.receive(request(5, 'config.set', { key: 'home', value: 'about:blank' }, 1));
    session.receive(request(6, 'done', undefined, 1));
    await tick();

    expect(calls).toEqual([
      ['weather config.set', { key: 'zip', value: '48201' }],
      ['weather config.set', { key: 'zip', value: '10001' }],
      ['browser config.set', { key: 'home', value: 'about:blank' }],
      ['browser done', undefined],
    ]);
    expect(delivered).toEqual([
      { event: 'contextChanged', context: contextFor('browser'), epoch: 1 },
      { id: 2, ok: true, value: { key: 'zip', value: '48201' } },
      { id: 3, ok: true, value: { key: 'zip', value: '10001' } },
      { id: 5, ok: true, value: { key: 'home', value: 'about:blank' } },
    ]);
  });

  test('refuses a request that names an epoch the page was never given', async () => {
    const calls: Calls = [];
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'weather',
      bind: target => appHost(target, calls),
      deliver: message => delivered.push(message),
    });

    session.receive(request(1, 'config.set', { key: 'zip', value: '48201' }, 1));
    session.receive(request(2, 'config.set', { key: 'zip', value: '48201' }, -1));
    session.receive(request(3, 'config.set', { key: 'zip', value: '48201' }, 0.5));
    session.receive(request(4, 'done', undefined, 1));
    await tick();

    expect(calls).toEqual([]);
    expect(delivered).toEqual([
      { id: 1, ok: false, error: UNKNOWN_SETTINGS_EPOCH },
      { id: 2, ok: false, error: UNKNOWN_SETTINGS_EPOCH },
      { id: 3, ok: false, error: UNKNOWN_SETTINGS_EPOCH },
    ]);
  });

  test('lets go of an earlier epoch the page releases and refuses anything sent on it afterwards', async () => {
    const calls: Calls = [];
    const delivered: SettingsMessage[] = [];
    const bound: string[] = [];
    const session = openSettingsSession({
      target: 'weather',
      bind: target => {
        bound.push(target);
        return appHost(target, calls);
      },
      deliver: message => delivered.push(message),
    });
    session.retarget('browser', contextFor('browser'));
    session.retarget('dash', contextFor('dash'));

    session.receive(request(1, 'config.set', { key: 'zip', value: '48201' }, 0));
    session.receive(request(2, 'release', undefined, 0));
    session.receive(request(3, 'config.set', { key: 'zip', value: '10001' }, 0));
    session.receive(request(4, 'release', undefined, 2));
    session.receive(request(5, 'config.set', { key: 'home', value: 'about:blank' }, 1));
    session.receive(request(6, 'config.set', { key: 'tile', value: 'wide' }, 2));
    session.receive(request(7, 'release', undefined, 1));
    session.receive(request(8, 'config.set', { key: 'home', value: 'about:blank' }, 1));
    await tick();

    expect(bound).toEqual(['weather', 'browser', 'dash']);
    expect(calls).toEqual([
      ['weather config.set', { key: 'zip', value: '48201' }],
      ['browser config.set', { key: 'home', value: 'about:blank' }],
      ['dash config.set', { key: 'tile', value: 'wide' }],
    ]);
    const replies = delivered.filter(message => 'id' in message).sort((a, b) => a.id - b.id);
    expect(replies).toEqual([
      { id: 1, ok: true, value: { key: 'zip', value: '48201' } },
      { id: 3, ok: false, error: UNKNOWN_SETTINGS_EPOCH },
      { id: 5, ok: true, value: { key: 'home', value: 'about:blank' } },
      { id: 6, ok: true, value: { key: 'tile', value: 'wide' } },
      { id: 8, ok: false, error: UNKNOWN_SETTINGS_EPOCH },
    ]);
  });

  test('serves a request stamped with an epoch that held no app as having no app', async () => {
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession<string>({
      target: null,
      bind: target => appHost(target),
      deliver: message => delivered.push(message),
    });

    session.retarget('weather', contextFor('weather'));
    session.receive(request(1, 'config.list', undefined, 0));
    await tick();

    expect(delivered).toEqual([
      { event: 'contextChanged', context: contextFor('weather'), epoch: 1 },
      { id: 1, ok: false, error: NO_SETTINGS_TARGET },
    ]);
  });

  test('answers context for the app it holds now and says which retarget that is', async () => {
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'weather',
      bind: target => appHost(target),
      deliver: message => delivered.push(message),
    });

    session.receive(request(1, 'context'));
    await tick();
    session.retarget('browser', contextFor('browser'));
    session.receive(request(2, 'context', undefined, 0));
    await tick();

    expect(delivered).toEqual([
      { id: 1, ok: true, value: contextFor('weather'), epoch: 0 },
      { event: 'contextChanged', context: contextFor('browser'), epoch: 1 },
      { id: 2, ok: true, value: contextFor('browser'), epoch: 1 },
    ]);
  });

  test('answers a context request that straddles a retarget with the app the page ended on', async () => {
    let release: () => void = () => undefined;
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'weather',
      bind: target =>
        target === 'weather'
          ? {
              ...appHost(target),
              context: () => new Promise(resolve => (release = () => resolve(contextFor('weather')))),
            }
          : appHost(target),
      deliver: message => delivered.push(message),
    });

    session.receive(request(1, 'context'));
    session.retarget('browser', contextFor('browser'));
    release();
    await tick();

    expect(delivered).toEqual([
      { event: 'contextChanged', context: contextFor('browser'), epoch: 1 },
      { id: 1, ok: true, value: contextFor('browser'), epoch: 1 },
    ]);
  });

  test('serves a page that never stamps its requests for as long as it is never retargeted', async () => {
    const calls: Calls = [];
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'ha',
      bind: target => appHost(target, calls),
      deliver: message => delivered.push(message),
    });

    session.receive(JSON.stringify({ id: 1, verb: 'config.set', payload: { key: 'token', value: 't' } }));
    session.receive(JSON.stringify({ id: 2, verb: 'done' }));
    await tick();

    expect(calls).toEqual([
      ['ha config.set', { key: 'token', value: 't' }],
      ['ha done', undefined],
    ]);
    expect(delivered).toEqual([{ id: 1, ok: true, value: { key: 'token', value: 't' } }]);
  });

  test('refuses every verb until it is given an app, then serves that app', async () => {
    const bound: string[] = [];
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession<string>({
      target: null,
      bind: target => {
        bound.push(target);
        return appHost(target);
      },
      deliver: message => delivered.push(message),
    });

    session.receive(request(1, 'context'));
    session.receive(request(2, 'config.list'));
    session.receive(request(3, 'done'));
    await tick();
    session.retarget('weather', contextFor('weather'));
    session.receive(request(4, 'config.list', undefined, 1));
    await tick();

    expect(bound).toEqual(['weather']);
    expect(delivered).toEqual([
      { id: 1, ok: false, error: NO_SETTINGS_TARGET },
      { id: 2, ok: false, error: NO_SETTINGS_TARGET },
      { event: 'contextChanged', context: contextFor('weather'), epoch: 1 },
      { id: 4, ok: true, value: [{ key: 'owner', value: 'weather' }] },
    ]);
  });

  test('passes host notices to its own document', () => {
    const delivered: SettingsMessage[] = [];
    const session = openSettingsSession({
      target: 'w',
      bind: () => recordingHost([]),
      deliver: message => delivered.push(message),
    });

    session.notify({ event: 'docChanged', key: 'layout', value: null });
    session.notify({ event: 'refresh', context: contextFor('w') });

    expect(delivered).toEqual([
      { event: 'docChanged', key: 'layout', value: null },
      { event: 'refresh', context: contextFor('w') },
    ]);
  });
});

describe('decodeSettingsMessage', () => {
  test('reads replies and events and drops everything else', () => {
    expect(decodeSettingsMessage('{"id":1,"ok":true,"value":3}')).toEqual({ id: 1, ok: true, value: 3 });
    expect(decodeSettingsMessage('{"event":"docChanged","key":"k","value":null}')).toEqual({
      event: 'docChanged',
      key: 'k',
      value: null,
    });
    expect(decodeSettingsMessage('nope')).toBeNull();
    expect(decodeSettingsMessage('3')).toBeNull();
    expect(decodeSettingsMessage({ id: 1 })).toBeNull();
  });
});

describe('the page sdk against a protocol host', () => {
  test('round-trips every verb shape the sdk sends', async () => {
    const calls: Calls = [];
    bridge.host = recordingHost(calls);

    expect(await settings.context()).toEqual({ webappId: 'w', name: 'weather', version: '1.0.0', deviceId: 'd' });
    expect(await settings.config.list()).toEqual([{ key: 'zip', value: '48201' }]);
    await settings.config.set('zip', '10001');
    await settings.config.delete('zip');
    await settings.doc.get('layout');
    await settings.doc.set('layout', '[]');
    await settings.doc.delete('layout');
    settings.done();

    expect(calls.map(([verb]) => verb)).toEqual([
      'context',
      'config.list',
      'config.set',
      'config.delete',
      'doc.get',
      'doc.set',
      'doc.delete',
    ]);
    expect(bridge.dones).toBe(1);
  });
});

describe('settingsSwitch', () => {
  const weather = settingsPageKey('weather', null);
  const ha = settingsPageKey('ha', 'h1');

  test('keys every config-only app to the one default page and a custom page to its app and hash', () => {
    expect(weather).toBe(DEFAULT_SETTINGS_KEY);
    expect(settingsPageKey('browser', undefined)).toBe(DEFAULT_SETTINGS_KEY);
    expect(ha).not.toBe(settingsPageKey('ha', 'h2'));
    expect(ha).not.toBe(settingsPageKey('calendar', 'h1'));
  });

  test('stays on the page already showing the app as it last described it', () => {
    expect(settingsSwitch({ target: 'weather', key: weather, stamp: 'v1' }, 'weather', weather, 'v1')).toBe('stay');
    expect(settingsSwitch({ target: 'ha', key: ha, stamp: 'v1' }, 'ha', ha, 'v1')).toBe('stay');
  });

  test('refreshes the page in place when its app changed underneath it', () => {
    expect(settingsSwitch({ target: 'weather', key: weather, stamp: 'v1' }, 'weather', weather, 'v2')).toBe('refresh');
    expect(settingsSwitch({ target: 'ha', key: ha, stamp: 'v1' }, 'ha', ha, 'v2')).toBe('refresh');
  });

  test('retargets the default page in place for another config-only app', () => {
    expect(settingsSwitch({ target: 'weather', key: weather, stamp: 'v1' }, 'browser', weather, 'v1')).toBe('retarget');
    expect(settingsSwitch({ target: null, key: DEFAULT_SETTINGS_KEY, stamp: null }, 'browser', weather, 'v1')).toBe(
      'retarget',
    );
  });

  test('loads whenever the page itself changes', () => {
    expect(settingsSwitch({ target: 'weather', key: weather, stamp: 'v1' }, 'ha', ha, 'v1')).toBe('load');
    expect(settingsSwitch({ target: 'ha', key: ha, stamp: 'v1' }, 'weather', weather, 'v1')).toBe('load');
    expect(settingsSwitch({ target: 'ha', key: ha, stamp: 'v1' }, 'ha', settingsPageKey('ha', 'h2'), 'v2')).toBe(
      'load',
    );
  });
});
