import {
  UNKNOWN_VERB,
  base64FromBytes,
  blobChunks,
  decodeSettingsMessage,
  type BlobData,
  type ConfigEntry,
  type ConfigField,
  type DocEntry,
  type FetchVerbReply,
  type FetchVerbRequest,
  type KeyValue,
  type PickKind,
  type PickedMedia,
  type SettingsContext,
  type SettingsPayload,
  type SettingsRequest,
  type SettingsResult,
  type SettingsVerb,
  type Theme,
  type WebappBlobEntry,
  type WireBody,
  type WireHeader,
} from './settings-protocol.js';
import { THEME_DEFAULTS_CSS, themeCss } from './theme.js';

export type {
  AuthorizeVerbReply,
  AuthorizeVerbRequest,
  BlobData,
  BodyKind,
  ByteProgress,
  ConfigEntry,
  ConfigField,
  DocEntry,
  FetchVerbReply,
  FetchVerbRequest,
  KeyValue,
  PickKind,
  PickedMedia,
  SettingsContext,
  Theme,
  ThemeColor,
  ThemeFont,
  ThemeScheme,
  ThemeText,
  WebappBlobEntry,
  WireBody,
  WireHeader,
} from './settings-protocol.js';

const CALL_TIMEOUT_MS = 15_000;
const AUTHORIZE_TIMEOUT_MS = 600_000;
const FETCH_TIMEOUT_SLACK_MS = 5_000;
const MAX_BODY_BYTES = 1024 * 1024;
const TEXTY = /json|text|xml|urlencoded/i;
const NULL_BODY_STATUS = new Set([101, 103, 204, 205, 304]);
const HOST_MISSING = 'not running inside a bridgething settings host (no companion webview, no host frame)';

export type SettingsRequestInit = RequestInit & { timeoutMs?: number };
export type FetchErrorKind = 'network' | 'timeout' | 'invalid_url';
export type AuthorizeErrorKind = 'cancelled' | 'busy' | 'unsupported';
export type BlobErrorKind =
  | 'unsupported'
  | 'disconnected'
  | 'too_large'
  | 'quota_exceeded'
  | 'storage_full'
  | 'not_media'
  | 'unknown_handle'
  | 'busy'
  | 'no_gesture'
  | 'failed';
export type PickOptions = { kind: PickKind; cover?: [width: number, height: number] };
export type BlobProgressListener = (sent: number, total: number) => void;
export type BlobPutOptions = { onProgress?: BlobProgressListener };
export type DocChangedListener = (key: string, value: string | null) => void;
export type ThemeChangedListener = (theme: Theme) => void;
export type ContextChangedListener = (context: SettingsContext, previous: SettingsContext | null) => void;
export type RefreshListener = (context: SettingsContext) => void;

type Pending = {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout> | undefined;
  progress?: BlobProgressListener;
};

declare global {
  interface Window {
    ReactNativeWebView?: { postMessage: (json: string) => void };
    __bridgethingSettingsDeliver?: (json: string) => void;
  }
}

let nextId = 1;
let epoch = 0;
let known: SettingsContext | null = null;
const pins = new Map<number, number>();
const pending = new Map<number, Pending>();
const docListeners = new Set<DocChangedListener>();
const themeListeners = new Set<ThemeChangedListener>();
const contextListeners = new Set<ContextChangedListener>();
const refreshListeners = new Set<RefreshListener>();
let themeStyle: HTMLStyleElement | null = null;

function paintTheme(theme: Theme | undefined): void {
  if (typeof document === 'undefined') return;
  if (!themeStyle?.isConnected) {
    themeStyle = document.createElement('style');
    themeStyle.dataset.bridgethingTheme = '';
    const parent = document.head ?? document.documentElement;
    parent.insertBefore(themeStyle, parent.firstChild);
  }
  themeStyle.textContent = theme ? THEME_DEFAULTS_CSS + themeCss(theme) : THEME_DEFAULTS_CSS;
}

function release(at: number): void {
  send({ id: nextId++, verb: 'release', epoch: at });
}

function retarget(context: SettingsContext, next: number): void {
  if (next < epoch) return;
  const previous = known;
  known = context;
  if (next === epoch) return;
  const left = epoch;
  epoch = next;
  for (let at = left; at < next; at += 1) if (!pins.has(at)) release(at);
  if (context.theme) paintTheme(context.theme);
  for (const listener of contextListeners) listener(context, previous);
}

function deliver(json: string): void {
  const message = decodeSettingsMessage(json);
  if (!message) return;

  if ('event' in message) {
    if (message.event === 'docChanged') for (const listener of docListeners) listener(message.key, message.value);
    if (message.event === 'themeChanged') {
      paintTheme(message.theme);
      for (const listener of themeListeners) listener(message.theme);
    }
    if (message.event === 'contextChanged') retarget(message.context, message.epoch);
    if (message.event === 'refresh') {
      known = message.context;
      if (message.context.theme) paintTheme(message.context.theme);
      for (const listener of refreshListeners) listener(message.context);
    }
    if (message.event === 'progress') pending.get(message.id)?.progress?.(message.sent, message.total);
    return;
  }

  if (message.ok && message.epoch !== undefined) retarget(message.value as SettingsContext, message.epoch);

  const entry = pending.get(message.id);
  if (!entry) return;
  pending.delete(message.id);
  clearTimeout(entry.timer);
  if (message.ok) entry.resolve(message.value);
  else entry.reject(new Error(message.error));
}

function send(request: SettingsRequest): boolean {
  const json = JSON.stringify(request);
  if (typeof window === 'undefined') return false;

  const webview = window.ReactNativeWebView;
  if (webview) {
    webview.postMessage(json);
    return true;
  }

  if (window.parent !== window) {
    window.parent.postMessage(json, '*');
    return true;
  }

  return false;
}

if (typeof window !== 'undefined') {
  window.__bridgethingSettingsDeliver = deliver;
  window.addEventListener('message', event => {
    if (event.source !== window.parent || event.source === window) return;
    if (typeof event.data === 'string') deliver(event.data);
  });
}

function call<V extends SettingsVerb>(
  verb: V,
  payload: SettingsPayload<V>,
  timeoutMs: number | null = CALL_TIMEOUT_MS,
  progress?: BlobProgressListener,
  at: number = epoch,
): Promise<SettingsResult<V>> {
  const id = nextId++;
  return new Promise<SettingsResult<V>>((resolve, reject) => {
    const timer =
      timeoutMs === null
        ? undefined
        : setTimeout(() => {
            if (pending.delete(id)) {
              reject(new Error(`bridgething settings call '${verb}' timed out after ${timeoutMs}ms`));
            }
          }, timeoutMs);
    pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer, progress });
    try {
      if (!send({ id, verb, payload, epoch: at } as SettingsRequest)) throw new Error(HOST_MISSING);
    } catch (err) {
      pending.delete(id);
      clearTimeout(timer);
      reject(err instanceof Error ? err : new Error(String(err)));
    }
  });
}

export class SettingsFetchError extends Error {
  constructor(
    public readonly kind: FetchErrorKind,
    message: string,
  ) {
    super(message);
    this.name = 'SettingsFetchError';
  }
}

export class BlobError extends Error {
  constructor(
    public readonly kind: BlobErrorKind,
    message: string,
  ) {
    super(message);
    this.name = 'BlobError';
  }
}

export class AuthorizeError extends Error {
  constructor(
    public readonly kind: AuthorizeErrorKind,
    message: string,
  ) {
    super(message);
    this.name = 'AuthorizeError';
  }
}

function reason(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function token(message: string): string {
  return message.split(':', 1)[0].trim();
}

function hostLacksVerb(message: string): boolean {
  return message === HOST_MISSING || message.startsWith(UNKNOWN_VERB);
}

function fetchFailure(err: unknown): Error {
  if (err instanceof SettingsFetchError) return err;
  const message = reason(err);
  if (hostLacksVerb(message)) return err instanceof Error ? err : new Error(message);
  const kind = token(message);
  if (kind === 'network' || kind === 'timeout' || kind === 'invalid_url') return new SettingsFetchError(kind, message);
  if (message.includes('timed out')) return new SettingsFetchError('timeout', message);
  return new SettingsFetchError('network', message);
}

function authorizeFailure(err: unknown): Error {
  if (err instanceof AuthorizeError) return err;
  const message = reason(err);
  if (hostLacksVerb(message)) return new AuthorizeError('unsupported', message);
  const kind = token(message);
  if (kind === 'cancelled' || kind === 'busy' || kind === 'unsupported') return new AuthorizeError(kind, message);
  return err instanceof Error ? err : new Error(message);
}

const BLOB_KINDS: Record<string, BlobErrorKind> = {
  unsupported: 'unsupported',
  not_connected: 'disconnected',
  link_lost: 'disconnected',
  too_large: 'too_large',
  quota_exceeded: 'quota_exceeded',
  storage_full: 'storage_full',
  not_media: 'not_media',
  unknown_handle: 'unknown_handle',
  busy: 'busy',
  no_gesture: 'no_gesture',
};

function blobFailure(err: unknown): Error {
  if (err instanceof BlobError) return err;
  const message = reason(err);
  if (hostLacksVerb(message)) return new BlobError('unsupported', message);
  return new BlobError(BLOB_KINDS[token(message)] ?? 'failed', message);
}

async function blobCall<V extends SettingsVerb>(
  verb: V,
  payload: SettingsPayload<V>,
  timeoutMs?: number | null,
  progress?: BlobProgressListener,
): Promise<SettingsResult<V>> {
  try {
    return await call(verb, payload, timeoutMs, progress);
  } catch (err) {
    throw blobFailure(err);
  }
}

function decodeUtf8(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

function bytesFromBase64(data: string): Uint8Array<ArrayBuffer> {
  const binary = atob(data);
  const bytes = new Uint8Array(new ArrayBuffer(binary.length));
  for (let at = 0; at < binary.length; at += 1) bytes[at] = binary.charCodeAt(at);
  return bytes;
}

function encodeBody(bytes: Uint8Array, contentType: string | null): WireBody {
  const text = TEXTY.test(contentType ?? '') ? decodeUtf8(bytes) : null;
  return text === null ? { kind: 'base64', data: base64FromBytes(bytes) } : { kind: 'text', data: text };
}

function toResponse(reply: FetchVerbReply): Response {
  const headers = new Headers(reply.headers);
  const empty = reply.body.data.length === 0 || NULL_BODY_STATUS.has(reply.status);
  const body = empty ? null : reply.body.kind === 'text' ? reply.body.data : bytesFromBase64(reply.body.data);
  return new Response(body, { status: reply.status, headers });
}

function requestFailure(input: RequestInfo | URL, err: unknown): Error {
  const target = typeof input === 'string' || input instanceof URL ? String(input) : input.url;
  try {
    new URL(target);
  } catch {
    return new SettingsFetchError('invalid_url', `${target}: ${reason(err)}`);
  }
  return err instanceof Error ? err : new Error(reason(err));
}

async function toVerbRequest(input: RequestInfo | URL, init?: SettingsRequestInit): Promise<FetchVerbRequest> {
  let request: Request;
  try {
    request = new Request(input, init);
  } catch (err) {
    throw requestFailure(input, err);
  }

  const buffer = await request.arrayBuffer();
  if (buffer.byteLength > MAX_BODY_BYTES) {
    throw new SettingsFetchError(
      'network',
      `settings fetch request body is ${buffer.byteLength} bytes, over the ${MAX_BODY_BYTES} byte cap`,
    );
  }

  const verb: FetchVerbRequest = {
    url: request.url,
    method: request.method,
    headers: [...request.headers].map(([name, value]): WireHeader => [name, value]),
  };
  if (buffer.byteLength > 0) verb.body = encodeBody(new Uint8Array(buffer), request.headers.get('content-type'));
  if (init?.timeoutMs !== undefined) verb.timeoutMs = init.timeoutMs;
  return verb;
}

export type ConfigVerbs = {
  fields(): Promise<ConfigField[]>;
  list(): Promise<ConfigEntry[]>;
  set(key: string, value: string): Promise<KeyValue>;
  delete(key: string): Promise<KeyValue>;
};
export type PinnedConfigVerbs = ConfigVerbs & { release(): void };

function configVerbs(at: () => number): ConfigVerbs {
  return {
    fields: () => call('config.fields', undefined, CALL_TIMEOUT_MS, undefined, at()),
    list: () => call('config.list', undefined, CALL_TIMEOUT_MS, undefined, at()),
    set: (key, value) => call('config.set', { key, value }, CALL_TIMEOUT_MS, undefined, at()),
    delete: key => call('config.delete', { key }, CALL_TIMEOUT_MS, undefined, at()),
  };
}

export const settings = {
  async context(): Promise<SettingsContext> {
    const context = await call('context', undefined);
    if (context.theme) paintTheme(context.theme);
    return context;
  },
  config: {
    ...configVerbs(() => epoch),
    pin(): PinnedConfigVerbs {
      const pinned = epoch;
      pins.set(pinned, (pins.get(pinned) ?? 0) + 1);
      let held = true;
      return {
        ...configVerbs(() => pinned),
        release() {
          if (!held) return;
          held = false;
          const left = (pins.get(pinned) ?? 1) - 1;
          if (left > 0) {
            pins.set(pinned, left);
            return;
          }
          pins.delete(pinned);
          if (pinned < epoch) release(pinned);
        },
      };
    },
  },
  doc: {
    get(key: string): Promise<KeyValue> {
      return call('doc.get', { key });
    },
    list(): Promise<DocEntry[]> {
      return call('doc.list', undefined);
    },
    set(key: string, value: string): Promise<KeyValue> {
      return call('doc.set', { key, value });
    },
    delete(key: string): Promise<{ key: string; value: null }> {
      return call('doc.delete', { key });
    },
  },
  async fetch(input: RequestInfo | URL, init?: SettingsRequestInit): Promise<Response> {
    const verb = await toVerbRequest(input, init);
    const budget = verb.timeoutMs === undefined ? CALL_TIMEOUT_MS : verb.timeoutMs + FETCH_TIMEOUT_SLACK_MS;
    try {
      return toResponse(await call('fetch', verb, Math.max(CALL_TIMEOUT_MS, budget)));
    } catch (err) {
      throw fetchFailure(err);
    }
  },
  installFetch(): () => void {
    const previous = globalThis.fetch;
    globalThis.fetch = ((input: RequestInfo | URL, init?: SettingsRequestInit) =>
      settings.fetch(input, init)) as typeof fetch;
    return () => {
      globalThis.fetch = previous;
    };
  },
  auth: {
    async authorize(url: string | URL): Promise<URL> {
      try {
        const reply = await call('auth.authorize', { url: String(url) }, AUTHORIZE_TIMEOUT_MS);
        return new URL(reply.url);
      } catch (err) {
        throw authorizeFailure(err);
      }
    },
  },
  pick(options: PickOptions): Promise<PickedMedia | null> {
    return blobCall(
      'pick',
      options.cover ? { kind: options.kind, cover: options.cover } : { kind: options.kind },
      null,
    );
  },
  blob: {
    async create(mime: string): Promise<string> {
      return (await blobCall('blob.create', { mime })).handle;
    },
    async append(handle: string, data: BlobData): Promise<number> {
      let size: number | null = null;
      for await (const chunk of blobChunks(data)) {
        size = (await blobCall('blob.append', { handle, data: base64FromBytes(chunk) })).size;
      }
      return size ?? (await blobCall('blob.append', { handle, data: '' })).size;
    },
    put(key: string, handle: string, options: BlobPutOptions = {}): Promise<WebappBlobEntry> {
      return blobCall('blob.put', { key, handle }, null, options.onProgress);
    },
    async delete(key: string): Promise<void> {
      await blobCall('blob.delete', { key });
    },
    list(): Promise<WebappBlobEntry[]> {
      return blobCall('blob.list', undefined);
    },
  },
  onDocChanged(listener: DocChangedListener): () => void {
    docListeners.add(listener);
    return () => {
      docListeners.delete(listener);
    };
  },
  onThemeChanged(listener: ThemeChangedListener): () => void {
    themeListeners.add(listener);
    return () => {
      themeListeners.delete(listener);
    };
  },
  onContextChanged(listener: ContextChangedListener): () => void {
    contextListeners.add(listener);
    return () => {
      contextListeners.delete(listener);
    };
  },
  onRefresh(listener: RefreshListener): () => void {
    refreshListeners.add(listener);
    return () => {
      refreshListeners.delete(listener);
    };
  },
  done(): void {
    if (!send({ id: nextId++, verb: 'done', epoch })) throw new Error(HOST_MISSING);
  },
};

if (typeof document !== 'undefined') {
  paintTheme(undefined);
  settings.context().catch(() => undefined);
}
