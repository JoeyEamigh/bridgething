import {
  DEFAULT_SETTINGS_KEY,
  openSettingsSession,
  settingsPageKey,
  settingsSwitch,
  type SettingsContext,
  type SettingsHost,
  type SettingsMessage,
  type SettingsSession,
  type Theme,
} from '@bridgething/client/settings-protocol';

import type { DeviceSession } from './session.ts';

export const SETTINGS_PAGE_CRASHED = 'the settings page crashed';
export const CRASH_LOOP_WINDOW_MS = 60_000;

export function hasSettings(webapp: { config: readonly unknown[]; settingsHash?: string | null }): boolean {
  return webapp.config.length > 0 || Boolean(webapp.settingsHash);
}

export type DeviceSettingsVerbs = Pick<
  SettingsHost,
  | 'config.list'
  | 'config.set'
  | 'config.delete'
  | 'doc.get'
  | 'doc.list'
  | 'doc.set'
  | 'doc.delete'
  | 'blob.create'
  | 'blob.append'
  | 'blob.put'
  | 'blob.delete'
  | 'blob.list'
>;

export type SettingsDevice = Pick<
  DeviceSession,
  | 'webappConfig'
  | 'setWebappConfigField'
  | 'deleteWebappConfigField'
  | 'webappDoc'
  | 'webappDocEntry'
  | 'setWebappDoc'
  | 'deleteWebappDoc'
  | 'createBlobHandle'
  | 'appendBlobHandle'
  | 'putWebappBlob'
  | 'deleteWebappBlob'
  | 'webappBlobs'
>;

export function deviceSettingsVerbs(device: SettingsDevice, id: string): DeviceSettingsVerbs {
  return {
    'config.list': () => device.webappConfig(id),
    'config.set': async ({ key, value }) => {
      await device.setWebappConfigField(id, key, value);
      return { key, value };
    },
    'config.delete': async ({ key }) => {
      await device.deleteWebappConfigField(id, key);
      const fresh = await device.webappConfig(id);
      return { key, value: fresh.find(entry => entry.key === key)?.value ?? null };
    },
    'doc.get': async ({ key }) => ({ key, value: await device.webappDocEntry(id, key) }),
    'doc.list': () => device.webappDoc(id),
    'doc.set': async ({ key, value }) => {
      await device.setWebappDoc(id, key, value);
      return { key, value };
    },
    'doc.delete': async ({ key }) => {
      await device.deleteWebappDoc(id, key);
      return { key, value: null };
    },
    'blob.create': async ({ mime }) => ({ handle: await device.createBlobHandle(mime) }),
    'blob.append': async ({ handle, data }) => ({ handle, size: await device.appendBlobHandle(handle, data) }),
    'blob.put': ({ key, handle }, report) =>
      device.putWebappBlob(id, key, handle, (sent, total) => report({ sent, total })),
    'blob.delete': async ({ key }) => {
      await device.deleteWebappBlob(id, key);
      return { key };
    },
    'blob.list': () => device.webappBlobs(id),
  };
}

export type WarmSettingsApp = { settingsHash: string | null; stamp: string };

export type WarmPage<T, P> = { target: T | null; key: string; stamp: string | null; page: P; revision: number };

export type WarmSettingsState<T, P> = {
  selected: T | null;
  wanted: string | null;
  loaded: WarmPage<T, P>;
  settled: number | null;
  failure: string | null;
};

export type WarmSettingsOptions<T, P> = {
  store: { get(): WarmSettingsState<T, P>; set(patch: Partial<WarmSettingsState<T, P>>): void };
  defaultPage: P;
  ident(target: T): string;
  app(target: T): WarmSettingsApp | null;
  context(target: T): SettingsContext;
  fetch(target: T): Promise<P>;
  bind(target: T): SettingsHost;
  explain(err: unknown): string;
};

export type WarmSettings<T, P> = {
  select(target: T): void;
  retry(): void;
  ready(state: WarmSettingsState<T, P>): boolean;
  attach(revision: number, deliver: (message: SettingsMessage) => void): SettingsSession<T> | null;
  settle(revision: number): void;
  reveal(): void;
  hide(): void;
  reload(revision: number): void;
  lost(): void;
  themeChanged(theme: Theme): void;
  docChanged(target: T, key: string, value: string | null): void;
};

export function warmSettingsState<T, P>(defaultPage: P): WarmSettingsState<T, P> {
  return {
    selected: null,
    wanted: null,
    loaded: { target: null, key: DEFAULT_SETTINGS_KEY, stamp: null, page: defaultPage, revision: 0 },
    settled: null,
    failure: null,
  };
}

export function warmSettings<T, P>(options: WarmSettingsOptions<T, P>): WarmSettings<T, P> {
  const { store, ident } = options;
  const fetching = new Set<string>();
  const strikes = new Map<string, number | null>();
  let live: SettingsSession<T> | null = null;
  let visible = false;
  let seen = false;

  const same = (a: T | null, b: T | null): boolean => a !== null && b !== null && ident(a) === ident(b);

  const ready = (state: WarmSettingsState<T, P>): boolean =>
    state.wanted !== null &&
    state.loaded.key === state.wanted &&
    same(state.loaded.target, state.selected) &&
    state.settled === state.loaded.revision;

  const session = (): SettingsSession<T> | null => (live && !live.closed ? live : null);

  const strikeOf = (loaded: WarmPage<T, P>): string =>
    [loaded.target === null ? '' : ident(loaded.target), loaded.key, loaded.stamp].join('\n');

  const mark = (): void => {
    if (visible && ready(store.get())) seen = true;
  };

  const bind = (target: T): SettingsHost => {
    const host = options.bind(target);
    return {
      ...host,
      done: () => {
        if (visible && ready(store.get())) host.done();
      },
    };
  };

  const load = (target: T | null, key: string, stamp: string | null, page: P): void => {
    live?.close();
    live = null;
    seen = false;
    store.set({ loaded: { target, key, stamp, page, revision: store.get().loaded.revision + 1 }, failure: null });
  };

  const fetchPage = (target: T, key: string): void => {
    if (fetching.has(key)) return;
    fetching.add(key);
    options.fetch(target).then(
      page => {
        fetching.delete(key);
        if (store.get().wanted === key) load(target, key, options.app(target)?.stamp ?? null, page);
      },
      (err: unknown) => {
        fetching.delete(key);
        if (store.get().wanted === key) store.set({ failure: options.explain(err) });
      },
    );
  };

  const select = (target: T): void => {
    const state = store.get();
    const app = options.app(target);
    const key = app ? settingsPageKey(ident(target), app.settingsHash) : null;
    const switched = !same(state.selected, target);
    if (switched || state.wanted !== key) {
      store.set({ selected: target, wanted: key, failure: switched ? null : state.failure });
    }
    if (!app || key === null) return;

    const { loaded } = state;
    const at = loaded.target === null ? null : ident(loaded.target);
    switch (settingsSwitch({ target: at, key: loaded.key, stamp: loaded.stamp }, ident(target), key, app.stamp)) {
      case 'stay':
        return;
      case 'refresh':
        store.set({ loaded: { ...loaded, stamp: app.stamp } });
        session()?.notify({ event: 'refresh', context: options.context(target) });
        return;
      case 'retarget':
        seen = false;
        store.set({ loaded: { ...loaded, target, stamp: app.stamp }, failure: null });
        session()?.retarget(target, options.context(target));
        mark();
        return;
      case 'load':
        if (key === DEFAULT_SETTINGS_KEY) load(target, key, app.stamp, options.defaultPage);
        else if (switched || state.failure === null) fetchPage(target, key);
    }
  };

  return {
    select,
    ready,
    retry() {
      const { selected } = store.get();
      strikes.clear();
      store.set({ failure: null });
      if (selected !== null) select(selected);
    },
    attach(revision, deliver) {
      const state = store.get();
      if (revision !== state.loaded.revision) return null;
      live?.close();
      seen = false;
      if (state.settled === revision) store.set({ settled: null });
      live = openSettingsSession<T>({ target: state.loaded.target, bind, deliver, explain: options.explain });
      return live;
    },
    settle(revision) {
      if (revision !== store.get().loaded.revision) return;
      store.set({ settled: revision });
      const strike = strikeOf(store.get().loaded);
      if (strikes.get(strike) === null) strikes.set(strike, Date.now());
      mark();
    },
    reveal() {
      if (visible) return;
      visible = true;
      const state = store.get();
      if (seen && ready(state) && state.loaded.target !== null) {
        session()?.notify({ event: 'refresh', context: options.context(state.loaded.target) });
      }
      mark();
    },
    hide() {
      visible = false;
    },
    reload(revision) {
      const { loaded } = store.get();
      if (revision === loaded.revision) load(loaded.target, loaded.key, loaded.stamp, loaded.page);
    },
    lost() {
      const state = store.get();
      const { loaded } = state;
      const wanted = state.wanted !== null && state.wanted === loaded.key;
      const strike = strikeOf(loaded);
      const counted = visible && wanted;
      const reloadedAt = strikes.get(strike) ?? null;
      const repeat = counted && reloadedAt !== null && Date.now() - reloadedAt <= CRASH_LOOP_WINDOW_MS;
      if (counted) strikes.set(strike, null);
      load(null, DEFAULT_SETTINGS_KEY, null, options.defaultPage);
      if ((wanted && state.settled !== loaded.revision) || repeat) store.set({ failure: SETTINGS_PAGE_CRASHED });
      else if (visible && state.selected !== null) select(state.selected);
    },
    themeChanged(theme) {
      session()?.notify({ event: 'themeChanged', theme });
    },
    docChanged(target, key, value) {
      const current = session();
      if (current && same(current.target, target)) current.notify({ event: 'docChanged', key, value });
    },
  };
}

export type SettingsFramePort = { postMessage(message: string, targetOrigin: string): void };
export type SettingsFrameEvent = { source: unknown; data: unknown };
export type SettingsFrameHub = {
  addEventListener(type: 'message', listener: (event: SettingsFrameEvent) => void): void;
  removeEventListener(type: 'message', listener: (event: SettingsFrameEvent) => void): void;
};

export function serveSettingsFrame<T, P>(
  hub: SettingsFrameHub,
  port: SettingsFramePort,
  settings: WarmSettings<T, P>,
  revision: number,
): () => void {
  const session = settings.attach(revision, message => port.postMessage(JSON.stringify(message), '*'));
  if (!session) return () => undefined;

  const onMessage = (event: SettingsFrameEvent): void => {
    if (event.source === port) session.receive(event.data);
  };
  hub.addEventListener('message', onMessage);
  return () => {
    hub.removeEventListener('message', onMessage);
    session.close();
  };
}
