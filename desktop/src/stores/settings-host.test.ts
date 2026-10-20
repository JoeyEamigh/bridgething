import type { SettingsMessage, Theme } from '@bridgething/client/settings-protocol';
import type { DocEntry, WebappInfo } from '@bridgething/companion-types';
import { serveSettingsFrame, warmSettingsState, type Invalidation, type SettingsFrameEvent } from '@bridgething/ui';
import { beforeEach, describe, expect, test } from 'bun:test';

import type { DesktopSession } from '../desktop.ts';
import { PATHS } from '../routes.ts';
import { seed, webapps } from './session.ts';
import {
  desktopSettings,
  docLedger,
  followRoute,
  forwardDocChanges,
  settingsRoute,
  settingsState,
  settingsTarget,
  type DesktopSettings,
} from './settings-host.ts';

function app(id: string, over: Partial<WebappInfo> = {}): WebappInfo {
  return {
    id,
    name: id,
    version: '1.0.0',
    source: 'installed',
    role: 'standard',
    config: [],
    permissions: [],
    ...over,
  } as WebappInfo;
}

const WEATHER = app('weather', {
  config: [{ type: 'string', data: { key: 'zip', label: 'ZIP' } }] as WebappInfo['config'],
});
const HA = app('home assistant', { settingsHash: 'h1' });
const CLOCK = app('clock');

describe('settings routes', () => {
  beforeEach(() => {
    settingsTarget.value = null;
  });

  test('reads an app detail and its settings from the url', () => {
    expect(settingsRoute(PATHS.app('home assistant'))).toEqual({ id: 'home assistant', open: false });
    expect(settingsRoute(PATHS.appSettings('home assistant'))).toEqual({ id: 'home assistant', open: true });
    expect(settingsRoute(`${PATHS.appSettings('weather')}?from=detail`)).toEqual({ id: 'weather', open: true });
    expect(settingsRoute(PATHS.apps)).toBeNull();
    expect(settingsRoute(PATHS.store)).toBeNull();
  });

  test('prewarms the app a detail screen shows and keeps it past apps without settings', () => {
    const installed = [WEATHER, HA, CLOCK];

    followRoute(PATHS.app('weather'), installed);
    expect(settingsTarget.value).toBe('weather');

    followRoute(PATHS.app('clock'), installed);
    followRoute(PATHS.store, installed);
    expect(settingsTarget.value).toBe('weather');

    followRoute(PATHS.appSettings('home assistant'), installed);
    expect(settingsTarget.value).toBe('home assistant');
  });
});

const tick = (): Promise<void> => new Promise(resolve => setTimeout(resolve, 0));
const THEME = { scheme: 'dark' } as Theme;

type Device = {
  session: DesktopSession;
  docs: Map<string, Map<string, string>>;
  config: string[];
  resources: Array<{ id: string; origin: unknown }>;
  invalidate(event: Invalidation): void;
};

function device(): Device {
  const docs = new Map<string, Map<string, string>>();
  const config: string[] = [];
  const resources: Device['resources'] = [];
  const listeners = new Set<(event: Invalidation) => void>();
  const of = (id: string): Map<string, string> => docs.get(id) ?? docs.set(id, new Map()).get(id)!;
  const served = {
    subscribe: (listener: (event: Invalidation) => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    webapps: async () => webapps.data.peek(),
    webappDoc: async (id: string): Promise<DocEntry[]> => [...of(id)].map(([key, value]) => ({ key, value })),
    setWebappDoc: async (id: string, key: string, value: string) => void of(id).set(key, value),
    deleteWebappDoc: async (id: string, key: string) => void of(id).delete(key),
    webappConfig: async (id: string) => [{ key: 'app', value: id }],
    setWebappConfigField: async (id: string, key: string, value: string) => void config.push(`${id} ${key}=${value}`),
    webappResource: async (id: string, _kind: string, origin: unknown) => {
      resources.push({ id, origin });
      return { digest: 'd', mime: 'text/html', bytes: new TextEncoder().encode(`${id} page`) };
    },
  };
  const session = new Proxy(served, {
    get: (held, name) => (name in held ? held[name as keyof typeof held] : async () => null),
  }) as unknown as DesktopSession;
  return {
    session,
    docs,
    config,
    resources,
    invalidate: event => {
      for (const listener of listeners) listener(event);
    },
  };
}

type Frame = { posted: SettingsMessage[]; send(id: number, verb: string, payload?: object, epoch?: number): void };

function windows() {
  const listeners = new Set<(event: SettingsFrameEvent) => void>();
  return {
    addEventListener: (_type: 'message', listener: (event: SettingsFrameEvent) => void) => void listeners.add(listener),
    removeEventListener: (_type: 'message', listener: (event: SettingsFrameEvent) => void) =>
      void listeners.delete(listener),
    frame(settings: DesktopSettings): Frame {
      const posted: SettingsMessage[] = [];
      const port = { postMessage: (message: string) => void posted.push(JSON.parse(message) as SettingsMessage) };
      const { revision } = settingsState.peek().loaded;
      serveSettingsFrame(this, port, settings, revision);
      settings.settle(revision);
      return {
        posted,
        send: (id, verb, payload, epoch) => {
          for (const listener of [...listeners])
            listener({ source: port, data: JSON.stringify({ id, verb, payload, epoch }) });
        },
      };
    },
  };
}

describe('the desktop settings host', () => {
  let closed = 0;
  let thing: Device;
  let settings: DesktopSettings;

  beforeEach(() => {
    closed = 0;
    thing = device();
    webapps.data.value = [WEATHER, HA, CLOCK];
    settingsState.value = warmSettingsState(settingsState.peek().loaded.page);
    settings = desktopSettings({ session: thing.session, theme: () => THEME, done: () => (closed += 1) });
  });

  test('points the default page at a config-only app without loading anything', async () => {
    const hub = windows();
    const frame = hub.frame(settings);

    settings.select('weather');
    frame.send(1, 'config.set', { key: 'zip', value: '90210' }, 1);
    frame.send(2, 'config.fields', undefined, 1);
    await tick();

    expect(settingsState.peek().loaded.revision).toBe(0);
    expect(settings.ready(settingsState.peek())).toBe(true);
    expect(thing.resources).toEqual([]);
    expect(thing.config).toEqual(['weather zip=90210']);
    expect(frame.posted[0]).toEqual({
      event: 'contextChanged',
      context: { webappId: 'weather', name: 'weather', version: '1.0.0', deviceId: '', theme: THEME },
      epoch: 1,
    });
    expect(frame.posted.slice(1)).toContainEqual({ id: 1, ok: true, value: { key: 'zip', value: '90210' } });
    expect(frame.posted.slice(1)).toContainEqual({ id: 2, ok: true, value: WEATHER.config });
    expect(frame.posted).toHaveLength(3);
  });

  test('fetches a custom page from the device, and the frame it replaces stops acting', async () => {
    const hub = windows();
    const old = hub.frame(settings);
    settings.select('weather');

    settings.select('home assistant');
    await tick();

    const { loaded } = settingsState.peek();
    expect(loaded).toMatchObject({ target: 'home assistant', revision: 1 });
    expect(await loaded.page.text()).toBe('home assistant page');
    expect(thing.resources).toEqual([{ id: 'home assistant', origin: null }]);
    expect(settings.ready(settingsState.peek())).toBe(false);

    const fresh = hub.frame(settings);
    old.send(1, 'config.set', { key: 'zip', value: 'old' }, 1);
    fresh.send(1, 'config.set', { key: 'url', value: 'new' });
    await tick();

    expect(settings.ready(settingsState.peek())).toBe(true);
    expect(thing.config).toEqual(['home assistant url=new']);
    expect(old.posted.filter(message => 'ok' in message)).toEqual([]);
  });

  test('ignores done from the warm page unless its settings screen is open', () => {
    const frame = windows().frame(settings);
    settings.select('weather');

    frame.send(1, 'done', undefined, 1);
    expect(closed).toBe(0);

    settings.reveal();
    frame.send(2, 'done', undefined, 1);
    settings.hide();
    frame.send(3, 'done', undefined, 1);

    expect(closed).toBe(1);
  });

  test('refreshes the open page when an update changes its app, and when the screen is opened again', () => {
    const frame = windows().frame(settings);
    settings.select('weather');
    settings.reveal();

    webapps.data.value = [{ ...WEATHER, version: '1.1.0' }, HA, CLOCK];
    settings.select('weather');
    settings.hide();
    settings.reveal();

    expect(frame.posted.map(message => ('event' in message ? message.event : 'reply'))).toEqual([
      'contextChanged',
      'refresh',
      'refresh',
    ]);
    expect(frame.posted[1]).toMatchObject({ context: { webappId: 'weather', version: '1.1.0' } });
  });

  test('forwards doc changes from the device, never what the page wrote itself or the first read', async () => {
    await seed(thing.session);
    thing.docs.set('weather', new Map([['saved', 'oslo']]));
    const frame = windows().frame(settings);
    settings.select('weather');
    const stop = forwardDocChanges(settings);
    await tick();

    frame.send(1, 'doc.set', { key: 'saved', value: 'bergen' }, 1);
    frame.send(2, 'doc.delete', { key: 'saved' }, 1);
    await tick();
    thing.invalidate({ topic: 'webapp-doc', id: 'weather' });
    await tick();
    expect(frame.posted.filter(message => 'event' in message && message.event === 'docChanged')).toEqual([]);

    thing.docs.get('weather')!.set('layout', '[1]');
    thing.invalidate({ topic: 'webapp-doc', id: 'weather' });
    await tick();
    stop();

    expect(frame.posted.filter(message => 'event' in message && message.event === 'docChanged')).toEqual([
      { event: 'docChanged', key: 'layout', value: '[1]' },
    ]);
  });
});

describe('docLedger', () => {
  test('the first read of an app is a baseline, and later reads report what moved', () => {
    const ledger = docLedger();

    expect(ledger.observe('weather', [{ key: 'saved', value: 'oslo' }])).toEqual([]);
    expect(
      ledger.observe('weather', [
        { key: 'saved', value: 'bergen' },
        { key: 'layout', value: '[1]' },
      ]),
    ).toEqual([
      { key: 'saved', value: 'bergen' },
      { key: 'layout', value: '[1]' },
    ]);
    expect(ledger.observe('weather', [{ key: 'layout', value: '[1]' }])).toEqual([{ key: 'saved', value: null }]);
  });

  test('what the page wrote is already known, for the app it wrote to only', () => {
    const ledger = docLedger();
    ledger.observe('weather', [{ key: 'saved', value: 'oslo' }]);

    ledger.wrote('weather', 'saved', 'bergen');
    ledger.wrote('weather', 'gone', null);
    ledger.wrote('dash', 'saved', 'tromso');

    expect(ledger.observe('weather', [{ key: 'saved', value: 'bergen' }])).toEqual([]);
    expect(ledger.observe('weather', [{ key: 'saved', value: 'tromso' }])).toEqual([{ key: 'saved', value: 'tromso' }]);
  });

  test('another app starts from its own baseline', () => {
    const ledger = docLedger();
    ledger.observe('weather', [{ key: 'saved', value: 'oslo' }]);

    expect(ledger.observe('dash', [{ key: 'layout', value: '[1]' }])).toEqual([]);
    expect(ledger.observe('weather', [])).toEqual([]);
  });
});
