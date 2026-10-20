import { SETTINGS_PAGE_MIME, settingsOriginFor } from '@bridgething/catalog';
import { DEFAULT_SETTINGS_PAGE } from '@bridgething/client/default-settings';
import type {
  AuthorizeVerbReply,
  FetchVerbReply,
  KeyValue,
  SettingsContext,
  PickedMedia as SettingsPickedMedia,
  Theme,
} from '@bridgething/client/settings-protocol';
import type { DocEntry, PickedMedia, WebappInfo } from '@bridgething/companion-types';
import {
  describeError,
  deviceSettingsVerbs,
  hasSettings,
  warmSettings,
  warmSettingsState,
  type ResourceOrigin,
  type WarmSettings,
  type WarmSettingsState,
} from '@bridgething/ui';
import { computed, effect, signal } from '@preact/signals';
import { invoke } from '@tauri-apps/api/core';

import type { DesktopSession } from '../desktop.ts';
import { pickMedia } from '../lib/picker.ts';
import { catalogFor } from './catalog.ts';
import { settled } from './resource.ts';
import { peers, webappDocFor, webapps } from './session.ts';

const APP_ROUTE = /^\/apps\/([^/?#]+)(\/settings)?(?:[?#]|$)/;
const DEFAULT_PAGE = new Blob([DEFAULT_SETTINGS_PAGE], { type: 'text/html' });

export type SettingsRoute = { id: string; open: boolean };
export type DesktopSettings = WarmSettings<string, Blob>;
export type DesktopSettingsOptions = { session: DesktopSession; theme: () => Theme; done: () => void };

export const settingsTarget = signal<string | null>(null);
export const settingsState = signal<WarmSettingsState<string, Blob>>(warmSettingsState(DEFAULT_PAGE));

const loadedTarget = computed(() => settingsState.value.loaded.target);

export function settingsRoute(url: string): SettingsRoute | null {
  const match = APP_ROUTE.exec(url);
  return match ? { id: decodeURIComponent(match[1]!), open: match[2] !== undefined } : null;
}

export function followRoute(url: string, installed: WebappInfo[]): void {
  const route = settingsRoute(url);
  const webapp = route ? installed.find(app => app.id === route.id) : undefined;
  if (webapp && hasSettings(webapp)) settingsTarget.value = webapp.id;
}

export function docLedger() {
  let target: string | null = null;
  let held: Map<string, string> | null = null;

  return {
    wrote(id: string, key: string, value: string | null): void {
      if (id !== target || !held) return;
      if (value === null) held.delete(key);
      else held.set(key, value);
    },
    observe(id: string, entries: DocEntry[]): KeyValue[] {
      const next = new Map(entries.map(entry => [entry.key, entry.value]));
      const before = id === target ? held : null;
      target = id;
      held = next;
      if (!before) return [];

      const changes: KeyValue[] = [];
      for (const [key, value] of next) if (before.get(key) !== value) changes.push({ key, value });
      for (const key of before.keys()) if (!next.has(key)) changes.push({ key, value: null });
      return changes;
    },
  };
}

const written = docLedger();

function installed(id: string): WebappInfo | null {
  return webapps.data.peek().find(app => app.id === id) ?? null;
}

function contextOf(id: string, theme: Theme): SettingsContext {
  const app = installed(id);
  const deviceId = peers.peek().find(peer => peer.status === 'connected')?.id ?? '';
  return { webappId: id, name: app?.name ?? '', version: app?.version ?? '', deviceId, theme };
}

function pickedMedia(media: PickedMedia): SettingsPickedMedia {
  return {
    handle: media.handle,
    mime: media.mime,
    size: media.size,
    ...(media.width === null ? {} : { width: media.width }),
    ...(media.height === null ? {} : { height: media.height }),
    ...(media.durationMs === null ? {} : { durationMs: media.durationMs }),
    ...(media.preview === null ? {} : { preview: media.preview }),
  };
}

async function hostedOrigin(webapp: WebappInfo): Promise<ResourceOrigin | null> {
  const provenance = webapp.provenance ?? null;
  const hash = webapp.settingsHash ?? null;
  if (!provenance || !hash) return null;

  const catalog = await settled(catalogFor(provenance));
  if (!catalog) return null;

  const hosted = settingsOriginFor([{ url: provenance, catalog }], provenance, webapp.id, hash);
  return hosted ? { ...hosted, mime: SETTINGS_PAGE_MIME } : null;
}

async function settingsPage(session: DesktopSession, id: string): Promise<Blob> {
  const webapp = installed(id);
  const origin = webapp ? await hostedOrigin(webapp) : null;
  const markup = await session.webappResource(id, 'settings', origin);
  return new Blob([Uint8Array.from(markup.bytes)], { type: markup.mime ?? 'text/html' });
}

export function desktopSettings({ session, theme, done }: DesktopSettingsOptions): DesktopSettings {
  return warmSettings<string, Blob>({
    store: {
      get: () => settingsState.peek(),
      set: patch => {
        settingsState.value = { ...settingsState.peek(), ...patch };
      },
    },
    defaultPage: DEFAULT_PAGE,
    ident: id => id,
    app: id => {
      const app = installed(id);
      if (!app) return null;
      return {
        settingsHash: app.settingsHash ?? null,
        stamp: [app.name, app.version, JSON.stringify(app.config)].join('\n'),
      };
    },
    context: id => contextOf(id, theme()),
    fetch: id => settingsPage(session, id),
    bind: id => {
      const device = deviceSettingsVerbs(session, id);
      return {
        context: async () => contextOf(id, theme()),
        'config.fields': async () => installed(id)?.config ?? [],
        ...device,
        'doc.set': async (request, report) => {
          const reply = await device['doc.set'](request, report);
          written.wrote(id, reply.key, reply.value);
          return reply;
        },
        'doc.delete': async (request, report) => {
          const reply = await device['doc.delete'](request, report);
          written.wrote(id, reply.key, null);
          return reply;
        },
        fetch: request => invoke<FetchVerbReply>('settings_fetch', { request }),
        'auth.authorize': ({ url }) => invoke<AuthorizeVerbReply>('settings_authorize', { url }),
        pick: async ({ kind, cover }) => {
          const box = cover ? { width: cover[0], height: cover[1] } : null;
          const media = await pickMedia(session, kind, box);
          return media === null ? null : pickedMedia(media);
        },
        done,
      };
    },
    explain: describeError,
  });
}

export function forwardDocChanges(settings: DesktopSettings): () => void {
  return effect(() => {
    const id = loadedTarget.value;
    if (id === null) return;
    const docs = webappDocFor(id);
    const entries = docs.data.value;
    if (!docs.settled.value || docs.error.value !== undefined) return;
    for (const change of written.observe(id, entries)) settings.docChanged(id, change.key, change.value);
  });
}
