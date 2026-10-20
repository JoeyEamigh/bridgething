import { DEFAULT_SETTINGS_PAGE } from '@bridgething/client/default-settings';
import type {
  PickVerbRequest,
  SettingsContext,
  SettingsHost,
  SettingsMessage,
  SettingsSession,
  Theme,
} from '@bridgething/client/settings-protocol';
import type {
  BridgethingPickedMedia,
  BridgethingWebappInfo,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import {
  deviceSettingsVerbs,
  warmSettings,
  warmSettingsState,
  type SettingsDevice,
  type WarmSettingsState,
} from '@bridgething/ui/settings-host';
import { Appearance } from 'react-native';
import { create } from 'zustand';

import { getSession } from './bridge';
import { settingsOrigin } from './catalog';
import { settingsAuthorize, settingsFetch } from './settings-bridge';
import { schemeOf, settingsTheme, type Scheme } from './theme';
import { configFields, useWebappsStore } from './webapps';

export type SettingsTarget = { deviceId: string; id: string };
export type SettingsFrame = {
  x: number;
  y: number;
  width: number;
  height: number;
};

type SettingsHostState = WarmSettingsState<SettingsTarget, string> & {
  frame: SettingsFrame | null;
  theme: Theme;
  instance: number;
};

type Announcement = { load: string; id: string; phase: 'start' | 'ready' };
type ServedDocument = { id: string; session: SettingsSession<SettingsTarget> };

const ANNOUNCEMENT = '{"bridgethingSettingsDocument":';
const LOADS = Math.random().toString(36).slice(2);
const DOCTYPE = /^\uFEFF?(?:\s|<!--[\s\S]*?-->)*<!doctype[^>]*>/i;
const BLUR =
  '(function(){var e=document.activeElement;if(e&&e!==document.body&&typeof e.blur==="function")e.blur();})();true;';

export const useSettingsHostStore = create<SettingsHostState>(() => ({
  ...warmSettingsState<SettingsTarget, string>(DEFAULT_SETTINGS_PAGE),
  frame: null,
  theme: settingsTheme(schemeOf(Appearance.getColorScheme())),
  instance: 0,
}));

let inject: ((script: string) => void) | null = null;
let closer: (() => void) | null = null;
let served: ServedDocument | null = null;
let started = false;

function ident(target: SettingsTarget): string {
  return `${target.deviceId}/${target.id}`;
}

function infoOf(target: SettingsTarget): BridgethingWebappInfo | null {
  const list = useWebappsStore.getState().byDevice[target.deviceId]?.list ?? [];
  return list.find(w => w.id === target.id) ?? null;
}

function contextOf(target: SettingsTarget): SettingsContext {
  const info = infoOf(target);
  return {
    webappId: target.id,
    name: info?.name ?? '',
    version: info?.version ?? '',
    deviceId: target.deviceId,
    theme: useSettingsHostStore.getState().theme,
  };
}

function deviceOf(deviceId: string): SettingsDevice {
  const session = getSession();
  return {
    webappConfig: id => session.listWebappConfig(deviceId, id),
    setWebappConfigField: (id, key, value) =>
      session.setWebappConfigField(deviceId, id, key, value),
    deleteWebappConfigField: (id, key) =>
      session.deleteWebappConfigField(deviceId, id, key),
    webappDoc: id => session.listWebappDoc(deviceId, id),
    webappDocEntry: (id, key) => session.getWebappDoc(deviceId, id, key),
    setWebappDoc: (id, key, value) =>
      session.setWebappDoc(deviceId, id, key, value),
    deleteWebappDoc: (id, key) => session.deleteWebappDoc(deviceId, id, key),
    createBlobHandle: mime => session.createBlobHandle(mime),
    appendBlobHandle: (handle, base64) =>
      session.appendBlobHandle(handle, base64),
    putWebappBlob: (id, key, handle, onProgress) =>
      session.putWebappBlob(deviceId, id, key, handle, onProgress),
    deleteWebappBlob: (id, key) => session.deleteWebappBlob(deviceId, id, key),
    webappBlobs: id => session.listWebappBlobs(deviceId, id),
  };
}

async function pickFor(
  target: SettingsTarget,
  { kind, cover }: PickVerbRequest,
): Promise<BridgethingPickedMedia | null> {
  const session = getSession();
  const start = useSettingsHostStore.getState();
  const stays = (state: SettingsHostState): boolean =>
    (state.frame !== null || start.frame === null) &&
    state.selected !== null &&
    ident(state.selected) === ident(target) &&
    state.loaded.revision === start.loaded.revision;
  let abandoned = false;
  const stop = useSettingsHostStore.subscribe(state => {
    if (abandoned || stays(state)) return;
    abandoned = true;
    session.abandonPick();
  });
  try {
    const picked = await session.pickMedia(kind, cover);
    return abandoned ? null : (picked ?? null);
  } finally {
    stop();
  }
}

function bind(target: SettingsTarget): SettingsHost {
  return {
    context: async () => contextOf(target),
    'config.fields': async () => {
      const info = infoOf(target);
      return info ? configFields(info) : [];
    },
    ...deviceSettingsVerbs(deviceOf(target.deviceId), target.id),
    fetch: settingsFetch,
    'auth.authorize': ({ url }) => settingsAuthorize(url),
    pick: request => pickFor(target, request),
    done: () => closer?.(),
  };
}

const settings = warmSettings<SettingsTarget, string>({
  store: {
    get: () => useSettingsHostStore.getState(),
    set: patch => useSettingsHostStore.setState(patch),
  },
  defaultPage: DEFAULT_SETTINGS_PAGE,
  ident,
  app: target => {
    const info = infoOf(target);
    if (!info) return null;
    return {
      settingsHash: info.settingsHash ?? null,
      stamp: [info.name, info.version, info.configJson].join('\n'),
    };
  },
  context: contextOf,
  fetch: target => {
    const info = infoOf(target);
    return getSession().webappSettingsMarkup(
      target.deviceId,
      target.id,
      info ? settingsOrigin(info) : undefined,
    );
  },
  bind,
  explain: describeError,
});

function loadToken(revision: number): string {
  return `${LOADS}.${revision}`;
}

function identityScript(load: string): string {
  return (
    '<script>(function(){var n=window.ReactNativeWebView;if(!n)return;' +
    `var l=${JSON.stringify(load)};` +
    'var d=l+"/"+Date.now().toString(36)+Math.random().toString(36).slice(2);' +
    'Object.defineProperty(window,"__bridgethingSettingsDocument",{value:d});' +
    'var a=function(p){n.postMessage(JSON.stringify({bridgethingSettingsDocument:{load:l,id:d,phase:p}}))};' +
    'a("start");document.addEventListener("DOMContentLoaded",function(){a("ready")});' +
    'var s=document.currentScript;if(s&&s.parentNode)s.parentNode.removeChild(s)})();</script>'
  );
}

function announcement(raw: string): Announcement | null {
  try {
    const held = (
      JSON.parse(raw) as { bridgethingSettingsDocument?: Partial<Announcement> }
    ).bridgethingSettingsDocument;
    if (typeof held?.load !== 'string' || typeof held.id !== 'string')
      return null;
    if (held.phase !== 'start' && held.phase !== 'ready') return null;
    return { load: held.load, id: held.id, phase: held.phase };
  } catch {
    return null;
  }
}

function delivery(document: string, message: SettingsMessage): string {
  const id = JSON.stringify(document);
  const json = JSON.stringify(JSON.stringify(message));
  return `window.__bridgethingSettingsDocument===${id}&&window.__bridgethingSettingsDeliver&&window.__bridgethingSettingsDeliver(${json});true;`;
}

export function settingsDocument(page: string, revision: number): string {
  const script = identityScript(loadToken(revision));
  const doctype = DOCTYPE.exec(page)?.[0] ?? '';
  return doctype + script + page.slice(doctype.length);
}

export function selectSettings(target: SettingsTarget): void {
  settings.select(target);
}

export function retrySettings(): void {
  settings.retry();
}

export function settingsReady(state: SettingsHostState): boolean {
  return settings.ready(state);
}

export function settingsAbsent(state: SettingsHostState): boolean {
  return state.selected !== null && state.wanted === null;
}

export function revealSettings(frame: SettingsFrame): void {
  useSettingsHostStore.setState({ frame });
  settings.reveal();
}

export function hideSettings(): void {
  if (useSettingsHostStore.getState().frame === null) return;
  useSettingsHostStore.setState({ frame: null });
  settings.hide();
  inject?.(BLUR);
}

export function onSettingsDone(close: () => void): () => void {
  closer = close;
  return () => {
    if (closer === close) closer = null;
  };
}

export function setSettingsScheme(scheme: Scheme): void {
  if (useSettingsHostStore.getState().theme.scheme === scheme) return;
  const theme = settingsTheme(scheme);
  useSettingsHostStore.setState({ theme });
  settings.themeChanged(theme);
}

export function settingsProcessGone(): void {
  useSettingsHostStore.setState(state => ({ instance: state.instance + 1 }));
  settings.lost();
}

export function serveSettingsMessage(raw: unknown): void {
  if (typeof raw !== 'string') return;
  if (!raw.startsWith(ANNOUNCEMENT)) {
    served?.session.receive(raw);
    return;
  }

  const announced = announcement(raw);
  if (!announced) return;
  const { revision } = useSettingsHostStore.getState().loaded;
  const current = announced.load === loadToken(revision);
  const { id } = announced;

  if (announced.phase === 'ready') {
    if (current && served?.id === id) settings.settle(revision);
    return;
  }

  served?.session.close();
  served = null;
  if (!current) return;
  const session = settings.attach(revision, message =>
    inject?.(delivery(id, message)),
  );
  if (session) served = { id, session };
}

export function attachSettingsView(
  injectInto: (script: string) => void,
): () => void {
  inject = injectInto;
  return () => {
    if (inject === injectInto) inject = null;
  };
}

export function startSettingsHost(): void {
  if (started) return;
  started = true;
  getSession().subscribe(event => {
    if (event.type !== 'webappDocChanged') return;
    settings.docChanged(
      { deviceId: event.deviceId, id: event.webappId },
      event.key,
      event.value,
    );
  });
  useWebappsStore.subscribe(() => {
    const { selected } = useSettingsHostStore.getState();
    if (selected) selectSettings(selected);
  });
}
