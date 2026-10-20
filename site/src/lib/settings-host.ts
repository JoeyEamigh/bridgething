import { SETTINGS_PAGE_MIME, settingsOriginFor } from '@bridgething/catalog';
import { DEFAULT_SETTINGS_PAGE } from '@bridgething/client/default-settings';
import {
  PICK_BUSY,
  UNKNOWN_VERB,
  base64FromBytes,
  blobChunks,
  type PickKind,
  type PickedMedia,
  type SettingsContext,
  type SettingsHost,
  type SettingsVerb,
  type Theme,
} from '@bridgething/client/settings-protocol';
import type { WebappInfo } from '@bridgething/companion-types';
import {
  deviceSettingsVerbs,
  warmSettings,
  type ResourceOrigin,
  type WarmSettings,
  type WarmSettingsOptions,
} from '@bridgething/ui';

import type { BrowserBackend } from './browser-tier';
import type { SourceFetchResult } from './catalog-source';

const ACCEPT: Record<PickKind, string> = { image: 'image/*', video: 'video/*', any: '' };
const UNTYPED = 'application/octet-stream';
export const NO_ACTIVATION = 'no_gesture: a browser opens its file picker only in answer to a tap or click';

let choosing = false;

export const DEFAULT_PAGE = new Blob([DEFAULT_SETTINGS_PAGE], { type: 'text/html' });

export type ChooseFile = (kind: PickKind) => Promise<File | null>;
export type FetchCatalog = (url: string) => Promise<SourceFetchResult>;
export type BrowserSettings = WarmSettings<string, Blob>;
export type BrowserSettingsOptions = {
  session: BrowserBackend;
  store: WarmSettingsOptions<string, Blob>['store'];
  webapp: () => WebappInfo;
  deviceId: () => string;
  theme: () => Theme;
  done: () => void;
  explain: (err: unknown) => string;
  fetchCatalog: FetchCatalog;
  choose?: ChooseFile;
};

function lacking(verb: SettingsVerb): () => Promise<never> {
  return () => Promise.reject(new Error(`${UNKNOWN_VERB}: ${verb}`));
}

export function chooseFile(kind: PickKind): Promise<File | null> {
  if (globalThis.navigator?.userActivation?.isActive === false) return Promise.reject(new Error(NO_ACTIVATION));
  return new Promise(resolve => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = ACCEPT[kind];
    input.addEventListener('change', () => resolve(input.files?.[0] ?? null), { once: true });
    input.addEventListener('cancel', () => resolve(null), { once: true });
    input.click();
  });
}

export async function pickedFile(session: BrowserBackend, file: File): Promise<PickedMedia> {
  const mime = file.type || UNTYPED;
  const handle = await session.createBlobHandle(mime);
  let size = 0;
  for await (const chunk of blobChunks(file)) size = await session.appendBlobHandle(handle, base64FromBytes(chunk));
  return { handle, mime, size };
}

export function browserSettingsHost(
  session: BrowserBackend,
  webapp: () => WebappInfo,
  context: () => SettingsContext,
  done: () => void,
  choose: ChooseFile = chooseFile,
): SettingsHost {
  const { id } = webapp();
  return {
    context: async () => context(),
    'config.fields': async () => webapp().config,
    ...deviceSettingsVerbs(session, id),
    fetch: lacking('fetch'),
    'auth.authorize': lacking('auth.authorize'),
    pick: async ({ kind }) => {
      if (choosing) throw new Error(PICK_BUSY);
      choosing = true;
      let file: File | null;
      try {
        file = await choose(kind);
      } finally {
        choosing = false;
      }
      return file ? pickedFile(session, file) : null;
    },
    done,
  };
}

export async function hostedSettingsOrigin(
  webapp: WebappInfo,
  fetchCatalog: FetchCatalog,
): Promise<ResourceOrigin | null> {
  const provenance = webapp.provenance ?? null;
  const hash = webapp.settingsHash ?? null;
  if (!provenance || !hash) return null;

  const fetched = await fetchCatalog(provenance).catch(() => null);
  if (!fetched?.ok) return null;

  const hosted = settingsOriginFor([{ url: provenance, catalog: fetched.catalog }], provenance, webapp.id, hash);
  return hosted ? { ...hosted, mime: SETTINGS_PAGE_MIME } : null;
}

export function browserSettings(options: BrowserSettingsOptions): BrowserSettings {
  const { session, webapp } = options;

  const context = (): SettingsContext => {
    const app = webapp();
    return {
      webappId: app.id,
      name: app.name,
      version: app.version,
      deviceId: options.deviceId(),
      theme: options.theme(),
    };
  };

  return warmSettings<string, Blob>({
    store: options.store,
    defaultPage: DEFAULT_PAGE,
    ident: id => id,
    app: id => {
      const app = webapp();
      if (app.id !== id) return null;
      return {
        settingsHash: app.settingsHash ?? null,
        stamp: [app.name, app.version, JSON.stringify(app.config)].join('\n'),
      };
    },
    context,
    fetch: async () => {
      const app = webapp();
      const origin = await hostedSettingsOrigin(app, options.fetchCatalog);
      const resource = await session.webappResource(app.id, 'settings', origin);
      return new Blob([Uint8Array.from(resource.bytes)], { type: resource.mime ?? 'text/html' });
    },
    bind: () => browserSettingsHost(session, webapp, context, options.done, options.choose),
    explain: options.explain,
  });
}
