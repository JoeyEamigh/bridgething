import {
  NO_SETTINGS_TARGET,
  type SettingsHost,
  type SettingsMessage,
  type SettingsSession,
} from '@bridgething/client/settings-protocol';
import type { ConfigEntry, DocEntry, WebappBlob } from '@bridgething/companion-types';
import { describe, expect, setSystemTime, test } from 'bun:test';

import {
  CRASH_LOOP_WINDOW_MS,
  SETTINGS_PAGE_CRASHED,
  deviceSettingsVerbs,
  hasSettings,
  serveSettingsFrame,
  warmSettings,
  warmSettingsState,
  type SettingsDevice,
  type SettingsFrameEvent,
  type WarmSettings,
  type WarmSettingsState,
} from './settings-host.ts';

type Store = { config: Map<string, string>; doc: Map<string, string>; defaults: Map<string, string> };

function deviceOver(stores: Record<string, Store>): SettingsDevice {
  const of = (id: string): Store => {
    const store = stores[id];
    if (!store) throw new Error(`no app ${id}`);
    return store;
  };
  const entries = (map: Map<string, string>): ConfigEntry[] & DocEntry[] =>
    [...map].map(([key, value]) => ({ key, value }));
  const device: Partial<SettingsDevice> = {
    webappConfig: async id => entries(of(id).config),
    setWebappConfigField: async (id, key, value) => void of(id).config.set(key, value),
    deleteWebappConfigField: async (id, key) => {
      const store = of(id);
      const fallback = store.defaults.get(key);
      if (fallback === undefined) store.config.delete(key);
      else store.config.set(key, fallback);
    },
    webappDoc: async id => entries(of(id).doc),
    webappDocEntry: async (id, key) => of(id).doc.get(key) ?? null,
    setWebappDoc: async (id, key, value) => void of(id).doc.set(key, value),
    deleteWebappDoc: async (id, key) => void of(id).doc.delete(key),
  };
  return device as SettingsDevice;
}

function store(): Store {
  return { config: new Map(), doc: new Map(), defaults: new Map() };
}

const quiet = (): void => {};
const tick = (): Promise<void> => new Promise(resolve => setTimeout(resolve, 0));

describe('deviceSettingsVerbs', () => {
  test('every verb lands on the app the verbs were made for, whatever is made afterwards', async () => {
    const stores = { weather: store(), dash: store() };
    const device = deviceOver(stores);
    const weather = deviceSettingsVerbs(device, 'weather');
    const dash = deviceSettingsVerbs(device, 'dash');

    await weather['config.set']({ key: 'units', value: 'metric' }, quiet);
    await dash['doc.set']({ key: 'layout', value: '[1,2]' }, quiet);
    await weather['doc.set']({ key: 'saved', value: 'oslo' }, quiet);

    expect(stores.weather.config.get('units')).toBe('metric');
    expect(stores.dash.doc.get('layout')).toBe('[1,2]');
    expect([...stores.weather.doc]).toEqual([['saved', 'oslo']]);
    expect(await dash['config.list'](undefined, quiet)).toEqual([]);
    expect(await dash['doc.list'](undefined, quiet)).toEqual([{ key: 'layout', value: '[1,2]' }]);
  });

  test('deleting a config key answers with what the device holds afterwards', async () => {
    const weather = store();
    weather.defaults.set('units', 'imperial');
    weather.config.set('units', 'metric').set('city', 'oslo');
    const verbs = deviceSettingsVerbs(deviceOver({ weather }), 'weather');

    expect(await verbs['config.delete']({ key: 'units' }, quiet)).toEqual({ key: 'units', value: 'imperial' });
    expect(await verbs['config.delete']({ key: 'city' }, quiet)).toEqual({ key: 'city', value: null });
  });

  test('a doc read of a missing key is null, and a delete answers null', async () => {
    const dash = store();
    dash.doc.set('layout', '[]');
    const verbs = deviceSettingsVerbs(deviceOver({ dash }), 'dash');

    expect(await verbs['doc.get']({ key: 'missing' }, quiet)).toEqual({ key: 'missing', value: null });
    expect(await verbs['doc.delete']({ key: 'layout' }, quiet)).toEqual({ key: 'layout', value: null });
    expect(dash.doc.has('layout')).toBe(false);
  });
});

describe('hasSettings', () => {
  const field = { type: 'boolean', data: { key: 'metric', label: 'Metric', default: true } } as const;

  test('a page or a declared field earns the settings button, and neither means no settings', () => {
    expect(hasSettings({ config: [], settingsHash: null })).toBe(false);
    expect(hasSettings({ config: [] })).toBe(false);
    expect(hasSettings({ config: [], settingsHash: 'abc' })).toBe(true);
    expect(hasSettings({ config: [field], settingsHash: null })).toBe(true);
  });
});

describe('deviceSettingsVerbs blobs', () => {
  function blobDevice(): { device: SettingsDevice; stored: Map<string, WebappBlob[]>; held: Map<string, string[]> } {
    const stored = new Map<string, WebappBlob[]>();
    const held = new Map<string, string[]>();
    const device: Partial<SettingsDevice> = {
      createBlobHandle: async mime => {
        const handle = `h${held.size}:${mime}`;
        held.set(handle, []);
        return handle;
      },
      appendBlobHandle: async (handle, base64) => {
        const chunks = held.get(handle) ?? [];
        chunks.push(base64);
        held.set(handle, chunks);
        return chunks.join('').length;
      },
      putWebappBlob: async (id, key, handle, onProgress) => {
        onProgress?.(0, 2);
        onProgress?.(2, 2);
        const blob = { key, sha256: handle, mime: handle.split(':')[1]!, size: (held.get(handle) ?? []).length };
        stored.set(id, [...(stored.get(id) ?? []).filter(entry => entry.key !== key), blob]);
        return blob;
      },
      deleteWebappBlob: async (id, key) =>
        void stored.set(
          id,
          (stored.get(id) ?? []).filter(entry => entry.key !== key),
        ),
      webappBlobs: async id => stored.get(id) ?? [],
    };
    return { device: device as SettingsDevice, stored, held };
  }

  test('a handle built in chunks is put, listed and deleted on the app the verbs were made for', async () => {
    const { device, stored } = blobDevice();
    const verbs = deviceSettingsVerbs(device, 'riprod');
    const other = deviceSettingsVerbs(device, 'other');

    const { handle } = await verbs['blob.create']({ mime: 'image/gif' }, quiet);
    expect(await verbs['blob.append']({ handle, data: 'R0lG' }, quiet)).toEqual({ handle, size: 4 });
    expect(await verbs['blob.append']({ handle, data: 'ODlh' }, quiet)).toEqual({ handle, size: 8 });
    expect(await verbs['blob.append']({ handle, data: '' }, quiet)).toEqual({ handle, size: 8 });
    const ticks: Array<{ sent: number; total: number }> = [];
    const blob = await verbs['blob.put']({ key: 'background', handle }, progress => ticks.push(progress));
    expect(ticks).toEqual([
      { sent: 0, total: 2 },
      { sent: 2, total: 2 },
    ]);

    expect(blob).toEqual({ key: 'background', sha256: handle, mime: 'image/gif', size: 3 });
    expect(await verbs['blob.list'](undefined, quiet)).toEqual([blob]);
    expect(await other['blob.list'](undefined, quiet)).toEqual([]);
    expect(await verbs['blob.delete']({ key: 'background' }, quiet)).toEqual({ key: 'background' });
    expect(stored.get('riprod')).toEqual([]);
  });
});

type App = { settingsHash: string | null; stamp: string };
type Put = { target: string; report: (progress: { sent: number; total: number }) => void; finish: () => void };
type Fetch = { target: string; resolve: (page: string) => void; reject: (err: Error) => void };

type Page = {
  session: SettingsSession<string> | null;
  inbox: SettingsMessage[];
  send(verb: string, payload?: object, epoch?: number): number;
  ask(verb: string, payload?: object, epoch?: number): Promise<SettingsMessage>;
};

type Rig = {
  settings: WarmSettings<string, string>;
  state(): WarmSettingsState<string, string>;
  apps: Map<string, App>;
  fetches: Fetch[];
  puts: Put[];
  writes: string[];
  closed: string[];
  open(revision?: number): Page;
  show(): Page;
};

const DEFAULT_PAGE = 'default page';

function rig(): Rig {
  let state = warmSettingsState<string, string>(DEFAULT_PAGE);
  const apps = new Map<string, App>([
    ['weather', { settingsHash: null, stamp: 'weather 1.0.0' }],
    ['browser', { settingsHash: null, stamp: 'browser 1.0.0' }],
    ['ha', { settingsHash: 'ha-1', stamp: 'ha 1.0.0' }],
    ['calendar', { settingsHash: 'cal-1', stamp: 'calendar 1.0.0' }],
  ]);
  const fetches: Fetch[] = [];
  const puts: Put[] = [];
  const writes: string[] = [];
  const closed: string[] = [];
  let nextId = 1;

  const settings = warmSettings<string, string>({
    store: {
      get: () => state,
      set: patch => {
        state = { ...state, ...patch };
      },
    },
    defaultPage: DEFAULT_PAGE,
    ident: target => target,
    app: target => apps.get(target) ?? null,
    context: target => ({ webappId: target, name: target, version: apps.get(target)?.stamp ?? '', deviceId: 'thing' }),
    fetch: target => new Promise<string>((resolve, reject) => fetches.push({ target, resolve, reject })),
    bind: target => {
      const host: Partial<SettingsHost> = {
        context: async () => ({ webappId: target, name: target, version: '', deviceId: 'thing' }),
        'config.list': async () => [{ key: 'app', value: target }],
        'config.set': async ({ key, value }) => {
          writes.push(`${target} ${key}=${value}`);
          return { key, value };
        },
        'blob.put': ({ key }, report) =>
          new Promise(resolve =>
            puts.push({ target, report, finish: () => resolve({ key, sha256: target, mime: 'image/gif', size: 1 }) }),
          ),
        done: () => void closed.push(target),
      };
      return host as SettingsHost;
    },
    explain: err => (err instanceof Error ? err.message : String(err)),
  });

  const open = (revision = state.loaded.revision): Page => {
    const inbox: SettingsMessage[] = [];
    const session = settings.attach(revision, message => inbox.push(message));
    const send = (verb: string, payload?: object, epoch?: number): number => {
      const id = nextId++;
      session?.receive(JSON.stringify({ id, verb, payload, epoch }));
      return id;
    };
    return {
      session,
      inbox,
      send,
      async ask(verb, payload, epoch) {
        const id = send(verb, payload, epoch);
        await tick();
        const reply = inbox.find(message => 'ok' in message && message.id === id);
        if (!reply) throw new Error(`no reply to ${verb}`);
        return reply;
      },
    };
  };

  return {
    settings,
    state: () => state,
    apps,
    fetches,
    puts,
    writes,
    closed,
    open,
    show() {
      const page = open();
      settings.settle(state.loaded.revision);
      return page;
    },
  };
}

function events(page: Page): string[] {
  return page.inbox.flatMap(message => ('event' in message ? [message.event] : []));
}

describe('warm settings', () => {
  test('holds the default page for no app, and that page refuses to act', async () => {
    const r = rig();
    const page = r.show();

    expect(r.state().loaded).toEqual({ target: null, key: 'default', stamp: null, page: DEFAULT_PAGE, revision: 0 });
    expect(r.settings.ready(r.state())).toBe(false);
    expect(await page.ask('config.list')).toMatchObject({ ok: false, error: NO_SETTINGS_TARGET });
  });

  test('moves the default page between config-only apps by context, never by reload', async () => {
    const r = rig();
    const page = r.show();

    r.settings.select('weather');
    expect(r.settings.ready(r.state())).toBe(true);
    expect(await page.ask('config.list', undefined, 1)).toMatchObject({ value: [{ value: 'weather' }] });

    r.settings.select('browser');

    expect(r.state().loaded.revision).toBe(0);
    expect(page.inbox.filter(message => 'event' in message)).toEqual([
      { event: 'contextChanged', context: expect.objectContaining({ webappId: 'weather' }), epoch: 1 },
      { event: 'contextChanged', context: expect.objectContaining({ webappId: 'browser' }), epoch: 2 },
    ]);
    expect(await page.ask('config.list', undefined, 2)).toMatchObject({ value: [{ value: 'browser' }] });
  });

  test('a request the page sent for an app before it was pointed at another lands on the app it was sent for', async () => {
    const r = rig();
    const page = r.show();
    r.settings.select('weather');
    r.settings.select('browser');

    const late = await page.ask('config.set', { key: 'zip', value: '90210' }, 1);
    const current = await page.ask('config.set', { key: 'home', value: 'about:blank' }, 2);

    expect(late).toMatchObject({ ok: true });
    expect(current).toMatchObject({ ok: true });
    expect(r.writes).toEqual(['weather zip=90210', 'browser home=about:blank']);
  });

  test('a document that is still running after its page was replaced is no longer served', async () => {
    const r = rig();
    const old = r.show();
    r.settings.select('weather');

    r.settings.select('ha');
    expect(r.settings.ready(r.state())).toBe(false);
    expect(await old.ask('config.list', undefined, 1)).toMatchObject({ value: [{ value: 'weather' }] });

    r.fetches[0]!.resolve('ha page');
    await tick();
    expect(r.state().loaded).toMatchObject({ target: 'ha', page: 'ha page', revision: 1 });

    const before = old.inbox.length;
    old.send('config.set', { key: 'zip', value: '90210' }, 1);
    old.send('config.list', undefined, 1);
    await tick();

    expect(r.writes).toEqual([]);
    expect(old.inbox.length).toBe(before);
    expect(old.session?.closed).toBe(true);

    const fresh = r.show();
    expect(r.settings.ready(r.state())).toBe(true);
    expect(await fresh.ask('config.list')).toMatchObject({ value: [{ value: 'ha' }] });
  });

  test('replies and progress for a replaced document reach nobody', async () => {
    const r = rig();
    r.settings.select('ha');
    r.fetches[0]!.resolve('ha page');
    await tick();
    const old = r.show();
    const put = old.send('blob.put', { key: 'background', handle: 'h1' });
    await tick();
    r.puts[0]!.report({ sent: 1, total: 4 });
    expect(old.inbox).toContainEqual({ event: 'progress', id: put, sent: 1, total: 4 });

    r.settings.select('calendar');
    r.fetches[1]!.resolve('calendar page');
    await tick();
    const fresh = r.show();
    const delivered = old.inbox.length;

    r.puts[0]!.report({ sent: 4, total: 4 });
    r.puts[0]!.finish();
    await tick();

    expect(old.inbox.length).toBe(delivered);
    expect(fresh.inbox).toEqual([]);
  });

  test('a document for a page that is no longer loaded is never attached', async () => {
    const r = rig();
    r.settings.select('ha');
    r.fetches[0]!.resolve('ha page');
    await tick();

    const stale = r.open(0);
    stale.send('config.set', { key: 'zip', value: '90210' });
    await tick();

    expect(stale.session).toBeNull();
    expect(r.writes).toEqual([]);
  });

  test('a second document in the same load takes over and the first stops being served', async () => {
    const r = rig();
    const first = r.show();
    r.settings.select('weather');
    expect(r.settings.ready(r.state())).toBe(true);

    const second = r.open();
    expect(r.settings.ready(r.state())).toBe(false);
    r.settings.settle(r.state().loaded.revision);

    first.send('config.set', { key: 'zip', value: '90210' }, 1);
    await tick();

    expect(first.session?.closed).toBe(true);
    expect(r.writes).toEqual([]);
    expect(await second.ask('config.list')).toMatchObject({ value: [{ value: 'weather' }] });
    expect(r.settings.ready(r.state())).toBe(true);
  });

  test('a page counts as ready only once its own document has finished loading', async () => {
    const r = rig();
    r.show();
    r.settings.select('ha');
    r.fetches[0]!.resolve('ha page');
    await tick();

    expect(r.settings.ready(r.state())).toBe(false);
    r.settings.settle(0);
    expect(r.settings.ready(r.state())).toBe(false);
    r.open();
    r.settings.settle(1);
    expect(r.settings.ready(r.state())).toBe(true);
  });

  test('one fetch serves a custom page however often it is selected while the fetch runs', async () => {
    const r = rig();
    r.show();

    r.settings.select('ha');
    r.settings.select('calendar');
    r.settings.select('ha');
    r.settings.select('ha');
    expect(r.fetches.map(fetch => fetch.target)).toEqual(['ha', 'calendar']);

    r.fetches[0]!.resolve('ha page');
    await tick();
    expect(r.state().loaded).toMatchObject({ target: 'ha', page: 'ha page', revision: 1 });

    r.fetches[1]!.resolve('calendar page');
    await tick();
    expect(r.state().loaded).toMatchObject({ target: 'ha', page: 'ha page', revision: 1 });
  });

  test('reloads the default page after a custom one, and a second app with an identical page still reloads', async () => {
    const r = rig();
    r.settings.select('ha');
    r.fetches[0]!.resolve('same page');
    await tick();

    r.settings.select('calendar');
    r.fetches[1]!.resolve('same page');
    await tick();
    expect(r.state().loaded).toMatchObject({ target: 'calendar', revision: 2 });

    r.settings.select('weather');
    expect(r.state().loaded).toMatchObject({ target: 'weather', page: DEFAULT_PAGE, revision: 3 });
  });

  test('drops a custom page that lands after the user moved on', async () => {
    const r = rig();
    r.show();
    r.settings.select('ha');
    r.settings.select('weather');

    r.fetches[0]!.resolve('ha page');
    await tick();

    expect(r.state().loaded).toMatchObject({ target: 'weather', page: DEFAULT_PAGE, revision: 0 });
    expect(r.settings.ready(r.state())).toBe(true);
  });

  test('shows why a page failed and fetches again only on retry', async () => {
    const r = rig();
    r.settings.select('ha');
    r.fetches[0]!.reject(new Error('the car thing went away'));
    await tick();

    expect(r.state().failure).toBe('the car thing went away');
    r.settings.select('ha');
    expect(r.fetches).toHaveLength(1);

    r.settings.retry();
    expect(r.state().failure).toBeNull();
    r.fetches[1]!.resolve('ha page');
    await tick();
    r.show();

    expect(r.settings.ready(r.state())).toBe(true);
  });

  test('an app that leaves the device stops being wanted, and comes back when it returns', () => {
    const r = rig();
    r.show();
    r.settings.select('weather');
    const weather = r.apps.get('weather')!;

    r.apps.delete('weather');
    r.settings.select('weather');
    expect(r.state()).toMatchObject({ selected: 'weather', wanted: null });
    expect(r.settings.ready(r.state())).toBe(false);

    r.apps.set('weather', weather);
    r.settings.select('weather');
    expect(r.settings.ready(r.state())).toBe(true);
  });

  test('tells the page to refresh when its app changes underneath it', () => {
    const r = rig();
    const page = r.show();
    r.settings.select('weather');

    r.settings.select('weather');
    expect(events(page)).toEqual(['contextChanged']);

    r.apps.set('weather', { settingsHash: null, stamp: 'weather 1.1.0' });
    r.settings.select('weather');
    r.settings.select('weather');

    expect(events(page)).toEqual(['contextChanged', 'refresh']);
    expect(page.inbox.at(-1)).toEqual({
      event: 'refresh',
      context: expect.objectContaining({ webappId: 'weather', version: 'weather 1.1.0' }),
    });
    expect(r.state().loaded).toMatchObject({ stamp: 'weather 1.1.0', revision: 0 });
  });

  test('tells the page to refresh each time it is shown again, and not the first time', () => {
    const r = rig();
    const page = r.show();
    r.settings.select('weather');

    r.settings.reveal();
    r.settings.reveal();
    expect(events(page)).toEqual(['contextChanged']);

    r.settings.hide();
    r.settings.reveal();
    expect(events(page)).toEqual(['contextChanged', 'refresh']);

    r.settings.hide();
    r.settings.select('browser');
    r.settings.reveal();
    expect(events(page)).toEqual(['contextChanged', 'refresh', 'contextChanged']);
  });

  test('a page that loads while the frame is already up is refreshed the next time it is shown', async () => {
    const r = rig();
    r.settings.select('ha');
    r.settings.reveal();
    r.fetches[0]!.resolve('ha page');
    await tick();
    const page = r.show();

    r.settings.hide();
    r.settings.reveal();

    expect(events(page)).toEqual(['refresh']);
  });

  test('honors done only from the page that is on screen', async () => {
    const r = rig();
    const page = r.show();
    r.settings.select('weather');

    page.send('done', undefined, 1);
    expect(r.closed).toEqual([]);

    r.settings.reveal();
    page.send('done', undefined, 1);
    expect(r.closed).toEqual(['weather']);

    r.settings.select('ha');
    page.send('done', undefined, 1);
    r.settings.hide();
    page.send('done', undefined, 1);
    expect(r.closed).toEqual(['weather']);
  });

  test('forwards a doc change to the document bound to that app only, and a theme change to whoever is loaded', () => {
    const r = rig();
    const page = r.show();
    r.settings.select('weather');
    const theme = { scheme: 'dark' } as Parameters<typeof r.settings.themeChanged>[0];

    r.settings.docChanged('browser', 'saved', 'x');
    r.settings.docChanged('weather', 'saved', 'y');
    r.settings.themeChanged(theme);

    expect(page.inbox.slice(1)).toEqual([
      { event: 'docChanged', key: 'saved', value: 'y' },
      { event: 'themeChanged', theme },
    ]);
  });

  test('a host that dies out of sight comes back on the default page and reloads on the next open', async () => {
    const r = rig();
    r.settings.select('ha');
    r.fetches[0]!.resolve('ha page');
    await tick();
    const dead = r.show();

    r.settings.lost();

    expect(dead.session?.closed).toBe(true);
    expect(r.state()).toMatchObject({
      selected: 'ha',
      failure: null,
      loaded: { target: null, key: 'default', page: DEFAULT_PAGE, revision: 2 },
    });
    expect(r.settings.ready(r.state())).toBe(false);
    expect(r.fetches).toHaveLength(1);

    r.settings.select('ha');
    r.fetches[1]!.resolve('ha page');
    await tick();
    r.show();
    expect(r.settings.ready(r.state())).toBe(true);
  });

  test('a host that dies on screen brings its app straight back', async () => {
    const r = rig();
    r.show();
    r.settings.select('weather');
    r.settings.reveal();

    r.settings.lost();

    expect(r.state()).toMatchObject({ failure: null, loaded: { target: 'weather', key: 'default', revision: 1 } });
    expect(r.settings.ready(r.state())).toBe(false);
    const page = r.show();
    expect(r.settings.ready(r.state())).toBe(true);
    expect(await page.ask('config.list')).toMatchObject({ value: [{ value: 'weather' }] });
  });

  test('a page that kills the host before it finishes loading is reported, not loaded again', async () => {
    const r = rig();
    r.settings.select('ha');
    r.settings.reveal();
    r.fetches[0]!.resolve('ha page');
    await tick();
    r.open();

    r.settings.lost();

    expect(r.state()).toMatchObject({
      failure: SETTINGS_PAGE_CRASHED,
      loaded: { target: null, key: 'default', revision: 2 },
    });
    expect(r.fetches).toHaveLength(1);

    r.settings.retry();
    expect(r.fetches).toHaveLength(2);
  });

  test('a page that kills the host on screen after it loaded is brought back once, then reported', async () => {
    const r = rig();
    r.settings.select('ha');
    r.settings.reveal();
    r.fetches[0]!.resolve('ha page');
    await tick();
    r.show();

    r.settings.lost();
    expect(r.state().failure).toBeNull();
    expect(r.fetches).toHaveLength(2);
    r.fetches[1]!.resolve('ha page');
    await tick();
    r.show();
    expect(r.settings.ready(r.state())).toBe(true);

    r.settings.lost();

    expect(r.state()).toMatchObject({ failure: SETTINGS_PAGE_CRASHED, loaded: { target: null, key: 'default' } });
    expect(r.fetches).toHaveLength(2);

    r.settings.retry();
    expect(r.fetches).toHaveLength(3);
    r.fetches[2]!.resolve('ha page');
    await tick();
    r.show();
    r.settings.lost();
    expect(r.state().failure).toBeNull();
    expect(r.fetches).toHaveLength(4);
  });

  test('a page lost out of sight never counts against it', async () => {
    const r = rig();
    r.settings.select('ha');
    r.fetches[0]!.resolve('ha page');
    await tick();
    r.show();
    r.settings.lost();
    r.settings.select('ha');
    r.fetches[1]!.resolve('ha page');
    await tick();
    r.show();
    r.settings.reveal();

    r.settings.lost();

    expect(r.state().failure).toBeNull();
    expect(r.fetches).toHaveLength(3);
  });

  test('a crash long after the reload that followed the last one reloads again instead of reporting', async () => {
    const start = Date.now();
    try {
      setSystemTime(start);
      const r = rig();
      r.settings.select('ha');
      r.settings.reveal();
      r.fetches[0]!.resolve('ha page');
      await tick();
      r.show();
      r.settings.lost();
      r.fetches[1]!.resolve('ha page');
      await tick();
      r.show();

      setSystemTime(start + CRASH_LOOP_WINDOW_MS + 1);
      r.settings.lost();

      expect(r.state().failure).toBeNull();
      expect(r.fetches).toHaveLength(3);
      r.fetches[2]!.resolve('ha page');
      await tick();
      r.show();

      setSystemTime(start + CRASH_LOOP_WINDOW_MS + 2);
      r.settings.lost();

      expect(r.state()).toMatchObject({ failure: SETTINGS_PAGE_CRASHED, loaded: { target: null, key: 'default' } });
      expect(r.fetches).toHaveLength(3);
    } finally {
      setSystemTime();
    }
  });

  test('the crash window runs from when the reloaded page finished loading, not from the crash', async () => {
    const start = Date.now();
    try {
      setSystemTime(start);
      const r = rig();
      r.settings.select('ha');
      r.settings.reveal();
      r.fetches[0]!.resolve('ha page');
      await tick();
      r.show();
      r.settings.lost();

      setSystemTime(start + CRASH_LOOP_WINDOW_MS * 2);
      r.fetches[1]!.resolve('ha page');
      await tick();
      r.show();
      setSystemTime(start + CRASH_LOOP_WINDOW_MS * 2 + 1);
      r.settings.lost();

      expect(r.state().failure).toBe(SETTINGS_PAGE_CRASHED);
    } finally {
      setSystemTime();
    }
  });

  test('a new version of a page that crashed is given its own chance', async () => {
    const r = rig();
    r.settings.select('ha');
    r.settings.reveal();
    r.fetches[0]!.resolve('ha page');
    await tick();
    r.show();
    r.settings.lost();
    r.apps.set('ha', { settingsHash: 'ha-2', stamp: 'ha 1.1.0' });
    r.settings.select('ha');
    r.fetches.at(-1)!.resolve('ha page 2');
    await tick();
    r.show();

    r.settings.lost();

    expect(r.state().failure).toBeNull();
  });

  test('a document that replaced itself is loaded again as a fresh document', () => {
    const r = rig();
    const page = r.show();
    r.settings.select('weather');

    r.settings.reload(7);
    expect(page.session?.closed).toBe(false);
    r.settings.reload(0);

    expect(page.session?.closed).toBe(true);
    expect(r.state().loaded).toMatchObject({ target: 'weather', key: 'default', revision: 1 });
    expect(r.settings.ready(r.state())).toBe(false);
  });
});

describe('serveSettingsFrame', () => {
  function hub() {
    const listeners = new Set<(event: SettingsFrameEvent) => void>();
    return {
      listeners,
      addEventListener: (_type: 'message', listener: (event: SettingsFrameEvent) => void) =>
        void listeners.add(listener),
      removeEventListener: (_type: 'message', listener: (event: SettingsFrameEvent) => void) =>
        void listeners.delete(listener),
      post(source: unknown, data: unknown): void {
        for (const listener of [...listeners]) listener({ source, data });
      },
    };
  }

  function port() {
    const posted: SettingsMessage[] = [];
    return {
      posted,
      postMessage: (message: string, targetOrigin: string) => {
        expect(targetOrigin).toBe('*');
        posted.push(JSON.parse(message) as SettingsMessage);
      },
    };
  }

  const request = (id: number, verb: string, payload?: object, epoch?: number): string =>
    JSON.stringify({ id, verb, payload, epoch });

  test('serves only the window it was opened for and answers only that window', async () => {
    const r = rig();
    r.settings.select('weather');
    const windows = hub();
    const frame = port();
    const stranger = port();

    serveSettingsFrame(windows, frame, r.settings, 0);
    windows.post(stranger, request(1, 'config.set', { key: 'zip', value: '1' }));
    windows.post(frame, request(2, 'config.set', { key: 'zip', value: '2' }));
    await tick();

    expect(r.writes).toEqual(['weather zip=2']);
    expect(frame.posted).toEqual([{ id: 2, ok: true, value: { key: 'zip', value: '2' } }]);
    expect(stranger.posted).toEqual([]);
  });

  test('the frame of a replaced page is deaf and mute while the new frame serves the new app', async () => {
    const r = rig();
    r.settings.select('ha');
    r.fetches[0]!.resolve('ha page');
    await tick();
    const windows = hub();
    const old = port();
    const stop = serveSettingsFrame(windows, old, r.settings, 1);
    windows.post(old, request(1, 'blob.put', { key: 'bg', handle: 'h' }));
    await tick();

    r.settings.select('calendar');
    r.fetches[1]!.resolve('calendar page');
    await tick();
    const fresh = port();
    serveSettingsFrame(windows, fresh, r.settings, 2);

    windows.post(old, request(2, 'config.set', { key: 'zip', value: 'old' }));
    windows.post(fresh, request(1, 'config.set', { key: 'zip', value: 'new' }));
    r.puts[0]!.report({ sent: 1, total: 1 });
    r.puts[0]!.finish();
    await tick();

    expect(r.writes).toEqual(['calendar zip=new']);
    expect(old.posted).toEqual([]);
    expect(fresh.posted).toEqual([{ id: 1, ok: true, value: { key: 'zip', value: 'new' } }]);

    stop();
    expect(windows.listeners.size).toBe(1);
  });

  test('a frame for a page that is no longer loaded never listens', () => {
    const r = rig();
    r.settings.select('ha');
    r.settings.select('weather');
    const windows = hub();

    serveSettingsFrame(windows, port(), r.settings, 7)();

    expect(windows.listeners.size).toBe(0);
  });

  test('unmounting the frame stops listening and drops what its page still had in flight', async () => {
    const r = rig();
    r.settings.select('weather');
    const windows = hub();
    const frame = port();
    const stop = serveSettingsFrame(windows, frame, r.settings, 0);
    windows.post(frame, request(1, 'blob.put', { key: 'bg', handle: 'h' }));
    await tick();

    stop();
    r.puts[0]!.finish();
    windows.post(frame, request(2, 'config.set', { key: 'zip', value: '1' }));
    await tick();

    expect(windows.listeners.size).toBe(0);
    expect(frame.posted).toEqual([]);
    expect(r.writes).toEqual([]);
  });
});
