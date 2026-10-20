import { DEFAULT_SETTINGS_PAGE } from '@bridgething/client/default-settings';
import type { SettingsMessage } from '@bridgething/client/settings-protocol';
import type { BridgethingWebappInfo } from '@bridgething/session-react-native';
import { SETTINGS_PAGE_CRASHED } from '@bridgething/ui/settings-host';

import { DEVICE, peer } from './fixtures';
import { rig, type Rig } from './harness';

jest.mock('react-native-inappbrowser-reborn', () => ({
  __esModule: true,
  default: {
    isAvailable: jest.fn(),
    openAuth: jest.fn(),
    closeAuth: jest.fn(),
  },
}));

const WEATHER = '01890000-0000-7000-8000-00000000000a';
const BROWSER = '01890000-0000-7000-8000-00000000000b';
const HA = '01890000-0000-7000-8000-00000000000c';
const CALENDAR = '01890000-0000-7000-8000-00000000000d';

const HA_PAGE = '<!doctype html><title>home assistant</title>';
const FRAME = { x: 0, y: 100, width: 390, height: 600 };

function app(
  id: string,
  name: string,
  over: Partial<BridgethingWebappInfo> = {},
): BridgethingWebappInfo {
  return {
    id,
    name,
    version: '1.0.0',
    source: 'installed',
    role: 'standard',
    configJson: `[{"type":"string","data":{"key":"${name}","label":"${name} field"}}]`,
    permissions: [],
    ...over,
  };
}

const APPS = [
  app(WEATHER, 'weather'),
  app(BROWSER, 'browser'),
  app(HA, 'home-assistant', { settingsHash: 'ha-1' }),
  app(CALENDAR, 'calendar', { settingsHash: 'cal-1' }),
];

const tick = () => new Promise<void>(resolve => setTimeout(resolve, 0));

type Host = typeof import('../lib/settings-host');

type Page = {
  id: string;
  html: string;
  delivered: SettingsMessage[];
  blurs: number;
  finish(): void;
  send(verb: string, payload?: Record<string, unknown>): number;
  ask(
    verb: string,
    payload?: Record<string, unknown>,
  ): Promise<SettingsMessage>;
};

type Web = {
  pages: Page[];
  hold: boolean;
  stalled: boolean;
  load(): void;
  current(): Page;
  open(html: string): Page;
  reload(): Page;
  mute(): () => void;
};

type Put = {
  args: unknown[];
  progress: (sent: number, total: number) => void;
  finish: () => void;
};

type Harness = {
  r: Rig;
  host: Host;
  web: Web;
  markups: Array<{
    id: string;
    resolve: (html: string) => void;
    reject: (err: Error) => void;
  }>;
  configReads: string[];
  puts: Put[];
};

function webview(host: Host): Web {
  const pages: Page[] = [];
  let queued: string[] | null = null;
  let showing = '';

  const post = (raw: string): void => {
    if (queued) queued.push(raw);
    else host.serveSettingsMessage(raw);
  };

  const web: Web = {
    pages,
    hold: false,
    stalled: false,
    load() {
      const { instance, loaded } = host.useSettingsHostStore.getState();
      const key = `${instance}:${loaded.revision}`;
      if (web.stalled || key === showing) return;
      showing = key;
      web.open(host.settingsDocument(loaded.page, loaded.revision));
    },
    current() {
      const page = pages.at(-1);
      if (!page) throw new Error('the webview holds no document');
      return page;
    },
    open(html) {
      const source = /<script>([\s\S]*?)<\/script>/.exec(html)?.[1];
      if (!source) throw new Error('the document carries no identity script');

      const delivered: SettingsMessage[] = [];
      let loaded: () => void = () => undefined;
      let epoch = 0;
      let nextId = 1;
      const window: Record<string, unknown> = {
        ReactNativeWebView: { postMessage: post },
        __bridgethingSettingsDeliver: (json: string) => {
          const message = JSON.parse(json) as SettingsMessage;
          if ('event' in message && message.event === 'contextChanged')
            epoch = message.epoch;
          if ('ok' in message && message.ok) epoch = message.epoch ?? epoch;
          delivered.push(message);
        },
      };
      const document = {
        body: {},
        currentScript: null,
        activeElement: { blur: () => void (page.blurs += 1) },
        addEventListener: (_type: string, listener: () => void) => {
          loaded = listener;
        },
      };
      const run = (script: string): void =>
        void new Function('window', 'document', script)(window, document);

      const page: Page & { run(script: string): void } = {
        id: '',
        html,
        delivered,
        blurs: 0,
        run,
        finish: () => loaded(),
        send(verb, payload) {
          const id = nextId++;
          post(JSON.stringify({ id, verb, payload, epoch }));
          return id;
        },
        async ask(verb, payload) {
          const id = page.send(verb, payload);
          await tick();
          const reply = delivered.find(m => 'ok' in m && m.id === id);
          if (!reply) throw new Error(`no reply to ${verb}`);
          return reply;
        },
      };
      pages.push(page);
      run(source);
      page.id = String(window.__bridgethingSettingsDocument);
      if (!web.hold) page.finish();
      return page;
    },
    reload() {
      return web.open(web.current().html);
    },
    mute() {
      const held: string[] = [];
      queued = held;
      return () => {
        queued = null;
        for (const raw of held) host.serveSettingsMessage(raw);
      };
    },
  };

  host.attachSettingsView(script =>
    (web.current() as Page & { run(script: string): void }).run(script),
  );
  host.useSettingsHostStore.subscribe(() => queueMicrotask(web.load));
  web.load();
  return web;
}

function list(r: Rig, webapps: BridgethingWebappInfo[]): void {
  const entry = { deviceId: DEVICE, webapps, active: undefined, listed: true };
  r.native.__world.webapps = [entry];
  r.emit('webappsChanged', entry);
}

async function setUp(): Promise<Harness> {
  const r = rig();
  const markups: Harness['markups'] = [];
  const configReads: string[] = [];
  const puts: Put[] = [];
  r.native.__returns.set(
    'webappSettingsMarkup',
    (_deviceId: string, id: string) =>
      new Promise<string>((resolve, reject) =>
        markups.push({ id, resolve, reject }),
      ),
  );
  r.native.__returns.set(
    'listWebappConfig',
    (_deviceId: string, id: string) => {
      configReads.push(id);
      return Promise.resolve([{ key: 'k', value: id }]);
    },
  );
  r.native.__returns.set(
    'putWebappBlob',
    (
      deviceId: string,
      id: string,
      key: string,
      handle: string,
      progress: (sent: number, total: number) => void,
    ) =>
      new Promise(resolve =>
        puts.push({
          args: [deviceId, id, key, handle],
          progress,
          finish: () =>
            resolve({ key, sha256: handle, mime: 'image/gif', size: 4 }),
        }),
      ),
  );
  r.native.__returns.set('webappSlots', () => Promise.resolve({}));
  list(r, APPS);
  r.emit('peerConnected', peer());

  const host = require('../lib/settings-host') as Host;
  const web = webview(host);
  host.startSettingsHost();
  await tick();
  return { r, host, web, markups, configReads, puts };
}

function state(h: Harness) {
  return h.host.useSettingsHostStore.getState();
}

function ready(h: Harness): boolean {
  return h.host.settingsReady(state(h));
}

function events(page: Page): string[] {
  return page.delivered.flatMap(m => ('event' in m ? [m.event] : []));
}

async function custom(h: Harness, id: string, html: string): Promise<Page> {
  h.host.selectSettings({ deviceId: DEVICE, id });
  h.markups.at(-1)!.resolve(html);
  await tick();
  return h.web.current();
}

describe('the warm settings host', () => {
  test('preloads the default page and refuses to act for no app', async () => {
    const h = await setUp();

    expect(state(h).loaded.page).toBe(DEFAULT_SETTINGS_PAGE);
    expect(h.web.pages).toHaveLength(1);
    expect(await h.web.current().ask('config.list')).toMatchObject({
      ok: false,
      error: 'no app is selected for settings',
    });
  });

  test('moves the default page between apps by context, never by reload', async () => {
    const h = await setUp();
    const page = h.web.current();
    const revision = state(h).loaded.revision;

    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    expect(ready(h)).toBe(true);
    expect(await page.ask('config.fields')).toMatchObject({
      ok: true,
      value: [{ data: { key: 'weather' } }],
    });

    h.host.selectSettings({ deviceId: DEVICE, id: BROWSER });
    await tick();

    expect(state(h).loaded.revision).toBe(revision);
    expect(h.web.pages).toHaveLength(1);
    expect(events(page)).toEqual(['contextChanged', 'contextChanged']);
    expect(page.delivered.filter(m => 'event' in m).at(-1)).toMatchObject({
      event: 'contextChanged',
      epoch: 2,
      context: {
        webappId: BROWSER,
        name: 'browser',
        deviceId: DEVICE,
        theme: { scheme: 'light' },
      },
    });
    expect(await page.ask('config.list')).toMatchObject({
      ok: true,
      value: [{ value: BROWSER }],
    });
  });

  test('keeps the loaded page bound to its own app until a custom page arrives', async () => {
    const h = await setUp();
    const weather = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    expect(ready(h)).toBe(false);
    expect(await weather.ask('config.list')).toMatchObject({
      value: [{ value: WEATHER }],
    });

    h.markups[0]!.resolve(HA_PAGE);
    await tick();

    expect(ready(h)).toBe(true);
    expect(state(h).loaded).toMatchObject({
      page: HA_PAGE,
      target: { id: HA },
    });
    expect(h.web.current()).not.toBe(weather);
    expect(await h.web.current().ask('config.list')).toMatchObject({
      value: [{ value: HA }],
    });
  });

  test('a document still running when its page is replaced never acts on the next app', async () => {
    const h = await setUp();
    const weather = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    const writes: unknown[][] = [];
    h.r.native.__returns.set('setWebappConfigField', (...args: unknown[]) => {
      writes.push(args);
      return Promise.resolve();
    });

    h.web.stalled = true;
    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    h.markups[0]!.resolve(HA_PAGE);
    await tick();
    expect(state(h).loaded.target).toMatchObject({ id: HA });
    expect(h.web.current()).toBe(weather);

    const delivered = weather.delivered.length;
    weather.send('config.set', { key: 'weather', value: 'oslo' });
    weather.send('config.list');
    await tick();

    expect(writes).toEqual([]);
    expect(h.configReads).toEqual([]);
    expect(weather.delivered.length).toBe(delivered);

    h.web.stalled = false;
    h.web.load();
    expect(await h.web.current().ask('config.list')).toMatchObject({
      value: [{ value: HA }],
    });
  });

  test('a reply for a replaced document never settles the same request id in the next one', async () => {
    const h = await setUp();
    const first = await custom(h, HA, HA_PAGE);
    const put = first.send('blob.put', { key: 'background', handle: 'h1' });
    await tick();

    await custom(h, CALENDAR, HA_PAGE);
    const second = await custom(h, HA, HA_PAGE);
    expect(second.send('blob.put', { key: 'background', handle: 'h2' })).toBe(
      put,
    );
    await tick();

    h.puts[0]!.progress(2, 4);
    h.puts[0]!.finish();
    await tick();
    expect(second.delivered).toEqual([]);

    h.puts[1]!.progress(4, 4);
    h.puts[1]!.finish();
    await tick();
    expect(h.puts.map(p => p.args)).toEqual([
      [DEVICE, HA, 'background', 'h1'],
      [DEVICE, HA, 'background', 'h2'],
    ]);
    expect(second.delivered).toEqual([
      { event: 'progress', id: put, sent: 4, total: 4 },
      {
        id: put,
        ok: true,
        value: { key: 'background', sha256: 'h2', mime: 'image/gif', size: 4 },
      },
    ]);
  });

  test('a document the host did not load gets its own session, and the one before it goes quiet', async () => {
    const h = await setUp();
    const first = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    first.send('blob.put', { key: 'background', handle: 'h1' });
    await tick();

    h.web.hold = true;
    const second = h.web.reload();

    expect(second.id).not.toBe(first.id);
    expect(ready(h)).toBe(false);
    second.finish();
    expect(ready(h)).toBe(true);

    h.puts[0]!.finish();
    await tick();
    expect(second.delivered).toEqual([]);
    expect(await second.ask('config.list')).toMatchObject({
      value: [{ value: WEATHER }],
    });
  });

  test('a reply only runs in the document it was produced for', async () => {
    const h = await setUp();
    const first = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    first.send('blob.put', { key: 'background', handle: 'h1' });
    await tick();
    const before = first.delivered.length;

    const flush = h.web.mute();
    const second = h.web.reload();
    h.puts[0]!.finish();
    await tick();

    expect(second.delivered).toEqual([]);
    expect(first.delivered.length).toBe(before);

    flush();
    expect(await second.ask('config.list')).toMatchObject({
      value: [{ value: WEATHER }],
    });
  });

  test('a document from an earlier load is never served, and neither is whatever it displaced', async () => {
    const h = await setUp();
    const stale = h.web.current().html;
    const page = await custom(h, HA, HA_PAGE);

    const straggler = h.web.open(stale);
    straggler.send('config.list');
    page.send('config.list');
    await tick();

    expect(h.configReads).toEqual([]);
    expect(straggler.delivered).toEqual([]);
  });

  test('puts the identity script after the doctype so the page keeps its rendering mode', async () => {
    const h = await setUp();

    const standards = h.host.settingsDocument(
      '\n<!-- built --><!DOCTYPE html><html><head></head></html>',
      3,
    );
    const marked = h.host.settingsDocument('\uFEFF<!doctype html><p>hi</p>', 3);
    const quirks = h.host.settingsDocument('<p>hi</p>', 3);

    expect(standards).toMatch(
      /^\n<!-- built --><!DOCTYPE html><script>.*<\/script><html>/,
    );
    expect(marked).toMatch(
      /^\uFEFF<!doctype html><script>.*<\/script><p>hi<\/p>$/,
    );
    expect(quirks).toMatch(/^<script>.*<\/script><p>hi<\/p>$/);
    expect(h.host.settingsDocument('<p>hi</p>', 3)).toBe(quirks);
    expect(h.host.settingsDocument('<p>hi</p>', 4)).not.toBe(quirks);
  });

  test('shows a page only once its own document has loaded', async () => {
    const h = await setUp();
    h.web.hold = true;

    const page = await custom(h, HA, HA_PAGE);
    expect(state(h).loaded.page).toBe(HA_PAGE);
    expect(ready(h)).toBe(false);

    page.finish();
    expect(ready(h)).toBe(true);
  });

  test('reloads the default page after a custom one and refetches nothing it already holds', async () => {
    const h = await setUp();
    await custom(h, HA, HA_PAGE);
    const revision = state(h).loaded.revision;

    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    expect(state(h).loaded).toMatchObject({
      page: DEFAULT_SETTINGS_PAGE,
      revision: revision + 1,
      target: { id: WEATHER },
    });

    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    expect(h.markups).toHaveLength(2);
  });

  test('fetches a custom page once however the user wanders while it is on its way', async () => {
    const h = await setUp();

    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    h.host.selectSettings({ deviceId: DEVICE, id: CALENDAR });
    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    expect(h.markups.map(m => m.id)).toEqual([HA, CALENDAR]);

    h.markups[0]!.resolve(HA_PAGE);
    await tick();
    const revision = state(h).loaded.revision;
    const page = h.web.current();
    h.markups[1]!.resolve(HA_PAGE);
    await tick();

    expect(state(h).loaded).toMatchObject({ target: { id: HA }, revision });
    expect(h.web.current()).toBe(page);
  });

  test('reloads for a second app even when its page is byte-identical', async () => {
    const h = await setUp();
    const first = await custom(h, HA, HA_PAGE);
    const revision = state(h).loaded.revision;

    const second = await custom(h, CALENDAR, HA_PAGE);

    expect(state(h).loaded).toMatchObject({
      revision: revision + 1,
      target: { id: CALENDAR },
    });
    expect(second).not.toBe(first);
    expect(second.html).not.toBe(first.html);
  });

  test('drops a custom page that lands after the user moved on', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    h.markups[0]!.resolve(HA_PAGE);
    await tick();

    expect(state(h).loaded).toMatchObject({
      page: DEFAULT_SETTINGS_PAGE,
      target: { id: WEATHER },
    });
    expect(ready(h)).toBe(true);
  });

  test('shows why a page failed and fetches again only on retry', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    h.markups[0]!.reject(new Error('the car thing went away'));
    await tick();

    expect(state(h).failure).toContain('the car thing went away');
    h.host.selectSettings({ deviceId: DEVICE, id: HA });
    expect(h.markups).toHaveLength(1);

    h.host.retrySettings();
    h.markups[1]!.resolve(HA_PAGE);
    await tick();

    expect(state(h).failure).toBeNull();
    expect(ready(h)).toBe(true);
  });

  test('swaps in a settings page an app update adds while it is selected', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    list(
      h.r,
      APPS.map(a =>
        a.id === WEATHER ? { ...a, settingsHash: 'weather-2' } : a,
      ),
    );
    expect(ready(h)).toBe(false);
    h.markups[0]!.resolve('<!doctype html><title>weather</title>');
    await tick();

    expect(state(h).loaded).toMatchObject({
      target: { id: WEATHER },
      page: '<!doctype html><title>weather</title>',
    });
  });

  test('tells a config-only page to refresh when an update changes its app', async () => {
    const h = await setUp();
    const page = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    const units =
      '[{"type":"string","data":{"key":"units","label":"units field"}}]';

    list(
      h.r,
      APPS.map(a =>
        a.id === WEATHER ? { ...a, version: '1.1.0', configJson: units } : a,
      ),
    );
    list(
      h.r,
      APPS.map(a =>
        a.id === WEATHER ? { ...a, version: '1.1.0', configJson: units } : a,
      ),
    );

    expect(h.web.pages).toHaveLength(1);
    expect(events(page)).toEqual(['contextChanged', 'refresh']);
    expect(page.delivered.at(-1)).toMatchObject({
      event: 'refresh',
      context: { webappId: WEATHER, version: '1.1.0' },
    });
    expect(await page.ask('config.fields')).toMatchObject({
      value: [{ data: { key: 'units' } }],
    });
  });

  test('tells the page to refresh each time its screen comes back, and not the first time', async () => {
    const h = await setUp();
    const page = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    h.host.revealSettings(FRAME);
    h.host.revealSettings({ ...FRAME, height: 400 });
    expect(events(page)).toEqual(['contextChanged']);

    h.host.hideSettings();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    h.host.revealSettings(FRAME);

    expect(events(page)).toEqual(['contextChanged', 'refresh']);
    expect(page.delivered.at(-1)).toMatchObject({
      event: 'refresh',
      context: { webappId: WEATHER, name: 'weather' },
    });
  });

  test('closes through the registered screen, and only for the page that is on screen', async () => {
    const h = await setUp();
    const page = h.web.current();
    const closed: string[] = [];
    const off = h.host.onSettingsDone(() => closed.push('screen'));
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    page.send('done');
    expect(closed).toEqual([]);

    h.host.revealSettings(FRAME);
    page.send('done');
    expect(closed).toEqual(['screen']);

    h.host.hideSettings();
    page.send('done');
    h.host.revealSettings(FRAME);
    off();
    page.send('done');

    expect(closed).toEqual(['screen']);
  });

  test('pushes a scheme flip and answers context with the current theme', async () => {
    const h = await setUp();
    const page = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    h.host.setSettingsScheme('dark');
    h.host.setSettingsScheme('dark');

    expect(
      page.delivered.filter(m => 'event' in m && m.event === 'themeChanged'),
    ).toEqual([
      {
        event: 'themeChanged',
        theme: expect.objectContaining({
          scheme: 'dark',
          colors: expect.objectContaining({ bg: '#0a0c0e' }),
        }),
      },
    ]);
    expect(await page.ask('context')).toMatchObject({
      value: { theme: { scheme: 'dark' } },
    });
  });

  test('forwards doc changes for the loaded app only', async () => {
    const h = await setUp();
    const page = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    h.r.emit('webappDocChanged', DEVICE, BROWSER, 'saved', 'x');
    h.r.emit('webappDocChanged', DEVICE, WEATHER, 'saved', 'y');

    expect(
      page.delivered.filter(m => 'event' in m && m.event === 'docChanged'),
    ).toEqual([{ event: 'docChanged', key: 'saved', value: 'y' }]);
  });

  test('puts a handle for the loaded app and reports its progress under the request id', async () => {
    const h = await setUp();
    const page = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });

    const id = page.send('blob.put', { key: 'background', handle: 'h1' });
    await tick();
    h.puts[0]!.progress(0, 4);
    h.puts[0]!.progress(4, 4);
    h.puts[0]!.finish();
    await tick();

    expect(h.puts[0]!.args).toEqual([DEVICE, WEATHER, 'background', 'h1']);
    expect(page.delivered.slice(1)).toEqual([
      { event: 'progress', id, sent: 0, total: 4 },
      { event: 'progress', id, sent: 4, total: 4 },
      {
        id,
        ok: true,
        value: { key: 'background', sha256: 'h1', mime: 'image/gif', size: 4 },
      },
    ]);
  });

  test('hands a pick to the native picker and a dismissed pick back as null', async () => {
    const h = await setUp();
    const page = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    const picks: unknown[][] = [];
    h.r.native.__returns.set('pickMedia', (...args: unknown[]) => {
      picks.push(args);
      return Promise.resolve(
        picks.length === 1
          ? { handle: 'h1', mime: 'image/gif', size: 9 }
          : undefined,
      );
    });

    const picked = await page.ask('pick', { kind: 'image', cover: [640, 360] });
    const dismissed = await page.ask('pick', { kind: 'any' });

    expect(picks).toEqual([
      ['image', 640, 360],
      ['any', undefined, undefined],
    ]);
    expect(picked).toMatchObject({
      ok: true,
      value: { handle: 'h1', mime: 'image/gif', size: 9 },
    });
    expect(dismissed).toMatchObject({ ok: true, value: null });
  });

  function heldPicks(h: Harness) {
    const held: Array<(media: unknown) => void> = [];
    h.r.native.__returns.set(
      'pickMedia',
      () => new Promise(resolve => held.push(resolve)),
    );
    return held;
  }

  function replied(page: Page, id: number): SettingsMessage | undefined {
    return page.delivered.find(m => 'ok' in m && m.id === id);
  }

  function abandons(h: Harness): number {
    return h.r.native.__calls.filter(call => call === 'abandonPick').length;
  }

  const PICKED = { handle: 'h1', mime: 'video/mp4', size: 9 };

  test('a pick in flight when the settings screen goes away is abandoned and answers null', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    h.host.revealSettings(FRAME);
    const page = h.web.current();
    const held = heldPicks(h);

    const id = page.send('pick', { kind: 'video' });
    await tick();
    expect(held).toHaveLength(1);
    h.host.revealSettings({ ...FRAME, height: 400 });
    expect(abandons(h)).toBe(0);

    h.host.hideSettings();
    expect(abandons(h)).toBe(1);
    held[0]!(PICKED);
    await tick();

    expect(replied(page, id)).toMatchObject({ ok: true, value: null });
    h.host.revealSettings(FRAME);
    h.host.hideSettings();
    expect(abandons(h)).toBe(1);
  });

  test('a pick in flight when the screen moves to another app is abandoned', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    h.host.revealSettings(FRAME);
    const page = h.web.current();
    const held = heldPicks(h);

    const id = page.send('pick', { kind: 'image' });
    await tick();
    h.host.selectSettings({ deviceId: DEVICE, id: BROWSER });

    expect(abandons(h)).toBe(1);
    held[0]!(PICKED);
    await tick();
    expect(replied(page, id)).toMatchObject({ ok: true, value: null });
  });

  test('a pick in flight when its page is torn down is abandoned', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    h.host.revealSettings(FRAME);
    const page = h.web.current();
    heldPicks(h);

    page.send('pick', { kind: 'image' });
    await tick();
    h.host.settingsProcessGone();

    expect(abandons(h)).toBe(1);
  });

  test('a pick that lands is never abandoned afterwards', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    h.host.revealSettings(FRAME);
    const page = h.web.current();
    const held = heldPicks(h);

    const id = page.send('pick', { kind: 'video' });
    await tick();
    held[0]!(PICKED);
    await tick();
    expect(replied(page, id)).toMatchObject({ ok: true, value: PICKED });

    h.host.hideSettings();
    h.host.selectSettings({ deviceId: DEVICE, id: BROWSER });
    expect(abandons(h)).toBe(0);
  });

  test('builds, lists and deletes blobs on the loaded app', async () => {
    const h = await setUp();
    const page = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    const seen: unknown[][] = [];
    h.r.native.__returns.set('createBlobHandle', (mime: string) =>
      Promise.resolve(`handle:${mime}`),
    );
    h.r.native.__returns.set(
      'appendBlobHandle',
      (handle: string, data: string) => {
        seen.push(['append', handle, data]);
        return Promise.resolve(3);
      },
    );
    h.r.native.__returns.set(
      'listWebappBlobs',
      (deviceId: string, id: string) => {
        seen.push(['list', deviceId, id]);
        return Promise.resolve([]);
      },
    );
    h.r.native.__returns.set(
      'deleteWebappBlob',
      (deviceId: string, id: string, key: string) => {
        seen.push(['delete', deviceId, id, key]);
        return Promise.resolve();
      },
    );

    expect(await page.ask('blob.create', { mime: 'image/png' })).toMatchObject({
      value: { handle: 'handle:image/png' },
    });
    expect(
      await page.ask('blob.append', { handle: 'h', data: 'AAEC' }),
    ).toMatchObject({
      value: { handle: 'h', size: 3 },
    });
    expect(
      await page.ask('blob.append', { handle: 'h', data: '' }),
    ).toMatchObject({
      value: { handle: 'h', size: 3 },
    });
    expect(await page.ask('blob.list')).toMatchObject({ value: [] });
    expect(await page.ask('blob.delete', { key: 'bg' })).toMatchObject({
      value: { key: 'bg' },
    });
    expect(seen).toEqual([
      ['append', 'h', 'AAEC'],
      ['append', 'h', ''],
      ['list', DEVICE, WEATHER],
      ['delete', DEVICE, WEATHER, 'bg'],
    ]);
  });

  test('reveals only where the screen asks, and hiding takes focus off the page once', async () => {
    const h = await setUp();
    const page = h.web.current();

    h.host.hideSettings();
    expect(page.blurs).toBe(0);

    h.host.revealSettings(FRAME);
    expect(state(h).frame).toEqual(FRAME);
    expect(page.blurs).toBe(0);

    h.host.hideSettings();
    h.host.hideSettings();
    expect(state(h).frame).toBeNull();
    expect(page.blurs).toBe(1);
  });

  test('says the app is gone when its car thing disconnects, and recovers when it returns', async () => {
    const h = await setUp();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    h.host.revealSettings(FRAME);
    expect(h.host.settingsAbsent(state(h))).toBe(false);

    h.r.emit('peerDisconnected', DEVICE);
    expect(h.host.settingsAbsent(state(h))).toBe(true);
    expect(ready(h)).toBe(false);

    list(h.r, APPS);
    expect(h.host.settingsAbsent(state(h))).toBe(false);
    expect(ready(h)).toBe(true);
  });

  test('brings the app on screen straight back when the web process dies', async () => {
    const h = await setUp();
    const dead = h.web.current();
    h.host.selectSettings({ deviceId: DEVICE, id: WEATHER });
    h.host.revealSettings(FRAME);
    const { instance } = state(h);

    h.host.settingsProcessGone();
    expect(state(h).instance).toBe(instance + 1);
    expect(ready(h)).toBe(false);
    await tick();

    const page = h.web.current();
    expect(page).not.toBe(dead);
    expect(state(h)).toMatchObject({
      failure: null,
      loaded: { page: DEFAULT_SETTINGS_PAGE, target: { id: WEATHER } },
    });
    expect(ready(h)).toBe(true);
    expect(await page.ask('config.list')).toMatchObject({
      value: [{ value: WEATHER }],
    });
  });

  test('comes back on the default page when the web process dies out of sight, and loads the app on the next open', async () => {
    const h = await setUp();
    await custom(h, HA, HA_PAGE);

    h.host.settingsProcessGone();
    await tick();

    expect(state(h)).toMatchObject({
      selected: { id: HA },
      failure: null,
      loaded: { page: DEFAULT_SETTINGS_PAGE, target: null },
    });
    expect(h.markups).toHaveLength(1);
    expect(ready(h)).toBe(false);

    const page = await custom(h, HA, HA_PAGE);
    expect(ready(h)).toBe(true);
    expect(await page.ask('config.list')).toMatchObject({
      value: [{ value: HA }],
    });
  });

  test('reports a page that kills the web process while loading instead of loading it again', async () => {
    const h = await setUp();
    h.web.hold = true;
    h.host.revealSettings(FRAME);
    await custom(h, HA, HA_PAGE);

    h.host.settingsProcessGone();
    await tick();

    expect(state(h).failure).toBe(SETTINGS_PAGE_CRASHED);
    expect(state(h).loaded).toMatchObject({
      page: DEFAULT_SETTINGS_PAGE,
      target: null,
    });
    expect(h.markups).toHaveLength(1);

    h.host.retrySettings();
    expect(h.markups).toHaveLength(2);
  });
});
