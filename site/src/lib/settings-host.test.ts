import type { Catalog } from '@bridgething/catalog';
import {
  PICK_BUSY,
  UNKNOWN_VERB,
  openSettingsSession,
  type SettingsContext,
  type SettingsHost,
  type SettingsMessage,
  type Theme,
} from '@bridgething/client/settings-protocol';
import type { BlobErrorKind } from '@bridgething/client/settings';
import type { WebappInfo } from '@bridgething/companion-types';
import {
  serveSettingsFrame,
  warmSettingsState,
  type SettingsFrameEvent,
  type WarmSettingsState,
} from '@bridgething/ui';
import { describe, expect, test } from 'bun:test';

import type { BrowserBackend } from './browser-tier.ts';
import type { SourceFetchResult } from './catalog-source.ts';
import {
  DEFAULT_PAGE,
  NO_ACTIVATION,
  browserSettings,
  browserSettingsHost,
  chooseFile,
  hostedSettingsOrigin,
  type BrowserSettings,
} from './settings-host.ts';

const PAGE = { url: 'https://apps.example.com/weather/settings.html', size: 2048, sha256: 'ab'.repeat(32) };
const CATALOG_URL = 'https://apps.example.com/catalog.json';

function webapp(overrides: Partial<WebappInfo> = {}): WebappInfo {
  return {
    id: 'weather-id',
    name: 'Weather',
    source: 'installed',
    role: 'standard',
    version: '1.0.0',
    description: null,
    iconHash: null,
    settingsHash: PAGE.sha256,
    overlayHash: null,
    config: [{ type: 'boolean', data: { key: 'metric', label: 'Metric', default: true } }],
    permissions: [],
    provenance: CATALOG_URL,
    extension: null,
    ...overrides,
  } as WebappInfo;
}

function catalog(settings: typeof PAGE | null): Catalog {
  return {
    apps: [
      {
        id: 'WEATHER-ID',
        versions: [{ version: '1.0.0', download: { url: 'x', size: 1, sha256: 'cd'.repeat(32) }, settings }],
      },
    ],
  } as unknown as Catalog;
}

function serving(result: SourceFetchResult): (url: string) => Promise<SourceFetchResult> {
  return async () => result;
}

function recordingSession(): { session: BrowserBackend; writes: string[] } {
  const writes: string[] = [];
  const session = {
    webappConfig: async (id: string) => [{ key: 'metric', value: `false for ${id}` }],
    setWebappConfigField: async (id: string, key: string, value: string) => void writes.push(`${id} ${key}=${value}`),
  } as unknown as BrowserBackend;
  return { session, writes };
}

function page(host: SettingsHost, deliver: (message: SettingsMessage) => void) {
  return openSettingsSession<string>({ target: 'weather-id', bind: () => host, deliver });
}

function exchange(host: SettingsHost, verb: string, payload?: object) {
  return new Promise<SettingsMessage>(resolve => page(host, resolve).receive(JSON.stringify({ id: 7, verb, payload })));
}

const tick = (): Promise<void> => new Promise(resolve => setTimeout(resolve, 0));

const CONTEXT: SettingsContext = { webappId: 'weather-id', name: 'Weather', version: '1.0.0', deviceId: 'serial:' };

describe('browserSettingsHost', () => {
  test('the verbs a browser cannot serve answer as unknown, which the page sdk degrades on', async () => {
    const { session } = recordingSession();
    const host = browserSettingsHost(
      session,
      () => webapp(),
      () => CONTEXT,
      () => {},
    );

    for (const verb of ['fetch', 'auth.authorize']) {
      const reply = await exchange(host, verb, { url: 'https://ha.local/api' });
      expect(reply).toEqual({ id: 7, ok: false, error: `${UNKNOWN_VERB}: ${verb}` });
    }
  });

  test('context and the declared fields come from the app the dialog opened', async () => {
    const { session } = recordingSession();
    const app = webapp();
    const host = browserSettingsHost(
      session,
      () => app,
      () => CONTEXT,
      () => {},
    );

    expect(await exchange(host, 'context')).toEqual({ id: 7, ok: true, value: CONTEXT, epoch: 0 });
    expect(await exchange(host, 'config.fields')).toEqual({ id: 7, ok: true, value: app.config });
  });

  test('config verbs reach the device for this app', async () => {
    const { session, writes } = recordingSession();
    const host = browserSettingsHost(
      session,
      () => webapp(),
      () => CONTEXT,
      () => {},
    );

    expect(await exchange(host, 'config.set', { key: 'metric', value: 'false' })).toEqual({
      id: 7,
      ok: true,
      value: { key: 'metric', value: 'false' },
    });
    expect(writes).toEqual(['weather-id metric=false']);
    expect(await exchange(host, 'config.list')).toEqual({
      id: 7,
      ok: true,
      value: [{ key: 'metric', value: 'false for weather-id' }],
    });
  });

  test('a pick streams the chosen file into a handle untouched, and a dismissed pick is null', async () => {
    const appended: string[] = [];
    const session = {
      createBlobHandle: async (mime: string) => `handle for ${mime}`,
      appendBlobHandle: async (_handle: string, base64: string) => {
        appended.push(base64);
        return appended.reduce((bytes, chunk) => bytes + atob(chunk).length, 0);
      },
    } as unknown as BrowserBackend;
    const body = Uint8Array.from({ length: 300 * 1024 }, (_, at) => at % 251);
    const chosen: Array<File | null> = [new File([body], 'loop.gif', { type: 'image/gif' }), null];
    const kinds: string[] = [];
    const host = browserSettingsHost(
      session,
      () => webapp(),
      () => CONTEXT,
      () => {},
      async kind => {
        kinds.push(kind);
        return chosen.shift() ?? null;
      },
    );

    expect(await exchange(host, 'pick', { kind: 'image', cover: [800, 480] })).toEqual({
      id: 7,
      ok: true,
      value: { handle: 'handle for image/gif', mime: 'image/gif', size: body.length },
    });
    expect(appended.length).toBe(2);
    expect(Uint8Array.from(appended.flatMap(chunk => [...atob(chunk)].map(c => c.charCodeAt(0))))).toEqual(body);
    expect(await exchange(host, 'pick', { kind: 'video' })).toEqual({ id: 7, ok: true, value: null });
    expect(kinds).toEqual(['image', 'video']);
  });

  test('a pick while another is open is busy on every bound page, and the first still lands', async () => {
    const { session } = recordingSession();
    const opened: Array<(file: File | null) => void> = [];
    const choose = (): Promise<File | null> => new Promise(resolve => opened.push(resolve));
    const bind = () =>
      browserSettingsHost(
        session,
        () => webapp(),
        () => CONTEXT,
        () => {},
        choose,
      );
    const host = bind();

    const first = exchange(host, 'pick', { kind: 'image' });
    await tick();
    expect(await exchange(host, 'pick', { kind: 'video' })).toEqual({ id: 7, ok: false, error: PICK_BUSY });
    expect(await exchange(bind(), 'pick', { kind: 'any' })).toEqual({ id: 7, ok: false, error: PICK_BUSY });
    expect(opened.length).toBe(1);

    opened[0](null);
    expect(await first).toEqual({ id: 7, ok: true, value: null });

    const next = exchange(host, 'pick', { kind: 'any' });
    await tick();
    expect(opened.length).toBe(2);
    opened[1](null);
    expect(await next).toEqual({ id: 7, ok: true, value: null });
  });

  test('a picker that fails to open frees the slot for the next pick', async () => {
    const { session } = recordingSession();
    const outcomes: Array<() => Promise<File | null>> = [
      () => Promise.reject(new Error(NO_ACTIVATION)),
      async () => null,
    ];
    const host = browserSettingsHost(
      session,
      () => webapp(),
      () => CONTEXT,
      () => {},
      () => outcomes.shift()!(),
    );

    expect(await exchange(host, 'pick', { kind: 'image' })).toEqual({ id: 7, ok: false, error: NO_ACTIVATION });
    expect(await exchange(host, 'pick', { kind: 'image' })).toEqual({ id: 7, ok: true, value: null });
  });

  test('the file picker refuses up front when the browser would silently not open it', async () => {
    const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator');
    Object.defineProperty(globalThis, 'navigator', {
      configurable: true,
      value: { userActivation: { isActive: false } },
    });
    try {
      await expect(chooseFile('image')).rejects.toThrow(NO_ACTIVATION);
      expect(NO_ACTIVATION.split(':', 1)[0]).toBe('no_gesture' satisfies BlobErrorKind);
    } finally {
      if (original) Object.defineProperty(globalThis, 'navigator', original);
    }
  });

  test('a put lands on this app and its progress reaches the page under the request id', async () => {
    const puts: string[] = [];
    const session = {
      putWebappBlob: async (
        id: string,
        key: string,
        handle: string,
        onProgress?: (sent: number, total: number) => void,
      ) => {
        puts.push(`${id} ${key} ${handle}`);
        onProgress?.(0, 2);
        onProgress?.(2, 2);
        return { key, sha256: 'ef'.repeat(32), mime: 'image/gif', size: 2 };
      },
    } as unknown as BrowserBackend;
    const host = browserSettingsHost(
      session,
      () => webapp(),
      () => CONTEXT,
      () => {},
    );
    const delivered: SettingsMessage[] = [];

    page(host, m => delivered.push(m)).receive(
      JSON.stringify({ id: 3, verb: 'blob.put', payload: { key: 'bg', handle: 'h' } }),
    );
    await tick();

    expect(puts).toEqual(['weather-id bg h']);
    expect(delivered).toEqual([
      { event: 'progress', id: 3, sent: 0, total: 2 },
      { event: 'progress', id: 3, sent: 2, total: 2 },
      { id: 3, ok: true, value: { key: 'bg', sha256: 'ef'.repeat(32), mime: 'image/gif', size: 2 } },
    ]);
  });

  test('done closes the dialog', () => {
    const { session } = recordingSession();
    let closed = 0;
    const host = browserSettingsHost(
      session,
      () => webapp(),
      () => CONTEXT,
      () => (closed += 1),
    );

    page(host, () => {}).receive(JSON.stringify({ id: 1, verb: 'done' }));

    expect(closed).toBe(1);
  });

  test('the declared fields follow the app as an update changes them', async () => {
    const { session } = recordingSession();
    let app = webapp();
    const host = browserSettingsHost(
      session,
      () => app,
      () => CONTEXT,
      () => {},
    );

    app = webapp({ config: [{ type: 'string', data: { key: 'zip', label: 'ZIP' } }] as WebappInfo['config'] });

    expect(await exchange(host, 'config.fields')).toEqual({ id: 7, ok: true, value: app.config });
  });
});

type Dialog = {
  settings: BrowserSettings;
  state(): WarmSettingsState<string, Blob>;
  app: { current: WebappInfo };
  writes: string[];
  resources: Array<{ id: string; origin: unknown }>;
  markup: { fail: Error | null };
  closed: { count: number };
  frame(): Frame;
};

type Frame = {
  posted: SettingsMessage[];
  stop(): void;
  send(id: number, verb: string, payload?: object, epoch?: number): void;
};

const THEME = { scheme: 'dark' } as Theme;

function dialog(opened: WebappInfo): Dialog {
  let state = warmSettingsState<string, Blob>(DEFAULT_PAGE);
  const app = { current: opened };
  const writes: string[] = [];
  const resources: Dialog['resources'] = [];
  const markup: Dialog['markup'] = { fail: null };
  const closed = { count: 0 };
  const listeners = new Set<(event: SettingsFrameEvent) => void>();
  const hub = {
    addEventListener: (_type: 'message', listener: (event: SettingsFrameEvent) => void) => void listeners.add(listener),
    removeEventListener: (_type: 'message', listener: (event: SettingsFrameEvent) => void) =>
      void listeners.delete(listener),
  };
  const session = {
    webappConfig: async (id: string) => [{ key: 'metric', value: `false for ${id}` }],
    setWebappConfigField: async (id: string, key: string, value: string) => void writes.push(`${id} ${key}=${value}`),
    webappResource: async (id: string, _kind: string, origin: unknown) => {
      if (markup.fail) throw markup.fail;
      resources.push({ id, origin });
      return { digest: 'd', mime: 'text/html', bytes: new TextEncoder().encode(`${id} page`) };
    },
  } as unknown as BrowserBackend;

  const settings = browserSettings({
    session,
    store: {
      get: () => state,
      set: patch => {
        state = { ...state, ...patch };
      },
    },
    webapp: () => app.current,
    deviceId: () => 'serial:',
    theme: () => THEME,
    done: () => void (closed.count += 1),
    explain: reason => (reason instanceof Error ? reason.message : String(reason)),
    fetchCatalog: serving({ ok: true, catalog: catalog(PAGE) }),
  });

  return {
    settings,
    state: () => state,
    app,
    writes,
    resources,
    markup,
    closed,
    frame() {
      const posted: SettingsMessage[] = [];
      const port = { postMessage: (raw: string) => void posted.push(JSON.parse(raw) as SettingsMessage) };
      const { revision } = state.loaded;
      const stop = serveSettingsFrame(hub, port, settings, revision);
      settings.settle(revision);
      return {
        posted,
        stop,
        send: (id, verb, payload, epoch) => {
          for (const listener of [...listeners])
            listener({ source: port, data: JSON.stringify({ id, verb, payload, epoch }) });
        },
      };
    },
  };
}

describe('browserSettings', () => {
  test('a config-only app opens on the built-in page without asking the device for one', async () => {
    const d = dialog(webapp({ settingsHash: null }));

    d.settings.select('weather-id');
    d.settings.reveal();
    const frame = d.frame();
    frame.send(1, 'context');
    frame.send(2, 'config.set', { key: 'metric', value: 'false' });
    await tick();

    expect(d.state().loaded).toMatchObject({ target: 'weather-id', page: DEFAULT_PAGE, revision: 0 });
    expect(d.settings.ready(d.state())).toBe(true);
    expect(d.resources).toEqual([]);
    expect(d.writes).toEqual(['weather-id metric=false']);
    expect(frame.posted).toContainEqual({
      id: 1,
      ok: true,
      epoch: 0,
      value: { webappId: 'weather-id', name: 'Weather', version: '1.0.0', deviceId: 'serial:', theme: THEME },
    });
  });

  test('a custom page comes from the hosted copy the catalog names, and a failed fetch can be retried', async () => {
    const d = dialog(webapp());
    d.markup.fail = new Error('the link dropped');

    d.settings.select('weather-id');
    await tick();
    expect(d.state().failure).toBe('the link dropped');

    d.markup.fail = null;
    d.settings.retry();
    await tick();

    expect(d.state().failure).toBeNull();
    expect(d.resources).toEqual([{ id: 'weather-id', origin: { ...PAGE, mime: 'text/html; charset=utf-8' } }]);
    expect(await d.state().loaded.page.text()).toBe('weather-id page');
    expect(d.settings.ready(d.state())).toBe(false);
    d.frame();
    expect(d.settings.ready(d.state())).toBe(true);
  });

  test('the frame of a page an update replaced stops acting, and the new frame takes over', async () => {
    const d = dialog(webapp());
    d.settings.select('weather-id');
    await tick();
    const old = d.frame();

    d.app.current = webapp({ settingsHash: 'cd'.repeat(32), version: '1.1.0' });
    d.settings.select('weather-id');
    await tick();
    expect(d.state().loaded.revision).toBe(2);

    const fresh = d.frame();
    old.send(1, 'config.set', { key: 'metric', value: 'old' });
    fresh.send(1, 'config.set', { key: 'metric', value: 'new' });
    await tick();

    expect(d.writes).toEqual(['weather-id metric=new']);
    expect(old.posted).toEqual([]);
    expect(fresh.posted).toEqual([{ id: 1, ok: true, value: { key: 'metric', value: 'new' } }]);
  });

  test('an update that keeps the page tells it to refresh', () => {
    const d = dialog(webapp({ settingsHash: null }));
    d.settings.select('weather-id');
    const frame = d.frame();

    d.app.current = webapp({ settingsHash: null, version: '1.1.0' });
    d.settings.select('weather-id');
    d.settings.select('weather-id');

    expect(frame.posted).toEqual([
      { event: 'refresh', context: expect.objectContaining({ webappId: 'weather-id', version: '1.1.0' }) },
    ]);
  });

  test('done closes the dialog while it is up, and a doc change reaches the page for this app only', () => {
    const d = dialog(webapp({ settingsHash: null }));
    d.settings.select('weather-id');
    const frame = d.frame();

    frame.send(1, 'done');
    expect(d.closed.count).toBe(0);
    d.settings.reveal();
    frame.send(2, 'done');
    expect(d.closed.count).toBe(1);

    d.settings.docChanged('other-id', 'saved', 'x');
    d.settings.docChanged('weather-id', 'saved', 'y');
    expect(frame.posted).toEqual([{ event: 'docChanged', key: 'saved', value: 'y' }]);
  });
});

describe('hostedSettingsOrigin', () => {
  test('the catalog copy whose digest is the one the device reports becomes the origin', async () => {
    const origin = await hostedSettingsOrigin(webapp(), serving({ ok: true, catalog: catalog(PAGE) }));

    expect(origin).toEqual({ ...PAGE, mime: 'text/html; charset=utf-8' });
  });

  test('a catalog without that page, an unreachable catalog, or no provenance all fall back to the link', async () => {
    const elsewhere = { ...PAGE, sha256: 'ef'.repeat(32) };
    const blocked: SourceFetchResult = { ok: false, kind: 'blocked', message: 'cors' };

    expect(await hostedSettingsOrigin(webapp(), serving({ ok: true, catalog: catalog(elsewhere) }))).toBeNull();
    expect(await hostedSettingsOrigin(webapp(), serving(blocked))).toBeNull();
    expect(await hostedSettingsOrigin(webapp(), () => Promise.reject(new Error('offline')))).toBeNull();
    expect(await hostedSettingsOrigin(webapp({ provenance: null }), serving(blocked))).toBeNull();
  });

  test('a config-only app never asks the catalog for a page', async () => {
    let asked = 0;
    const origin = await hostedSettingsOrigin(webapp({ settingsHash: null }), async () => {
      asked += 1;
      return { ok: true, catalog: catalog(PAGE) };
    });

    expect(origin).toBeNull();
    expect(asked).toBe(0);
  });
});
