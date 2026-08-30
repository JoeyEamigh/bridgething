import type { ConfigEntry, ConfigField, DocEntry } from '@bridgething/lib';

export type { ConfigEntry, ConfigField, DocEntry } from '@bridgething/lib';

const CALL_TIMEOUT_MS = 15_000;
const AUTHORIZE_TIMEOUT_MS = 600_000;
const FETCH_TIMEOUT_SLACK_MS = 5_000;
const MAX_BODY_BYTES = 1024 * 1024;
const TEXTY = /json|text|xml|urlencoded/i;
const NULL_BODY_STATUS = new Set([101, 103, 204, 205, 304]);
const HOST_MISSING = 'not running inside a bridgething settings host (no companion webview, no host frame)';

export type SettingsContext = { webappId: string; name: string; version: string; deviceId: string };
export type BodyKind = 'text' | 'base64';
export type WireBody = { kind: BodyKind; data: string };
export type WireHeader = [name: string, value: string];
export type FetchVerbRequest = {
  url: string;
  method?: string;
  headers?: WireHeader[];
  body?: WireBody;
  timeoutMs?: number;
};
export type FetchVerbReply = { status: number; headers: WireHeader[]; body: WireBody };
export type AuthorizeVerbRequest = { url: string };
export type AuthorizeVerbReply = { url: string };
export type SettingsRequestInit = RequestInit & { timeoutMs?: number };
export type FetchErrorKind = 'network' | 'timeout' | 'invalid_url';
export type AuthorizeErrorKind = 'cancelled' | 'busy' | 'unsupported';
export type KeyValue = { key: string; value: string | null };
export type DocChangedListener = (key: string, value: string | null) => void;

type HostReply = { id: number; ok: true; value: unknown } | { id: number; ok: false; error: string };
type HostEvent = { event: 'docChanged'; key: string; value: string | null };

type Pending = {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout>;
};

declare global {
  interface Window {
    ReactNativeWebView?: { postMessage: (json: string) => void };
    __bridgethingSettingsDeliver?: (json: string) => void;
  }
}

let nextId = 1;
const pending = new Map<number, Pending>();
const docListeners = new Set<DocChangedListener>();

function deliver(json: string): void {
  let parsed: HostReply | HostEvent;
  try {
    parsed = JSON.parse(json) as HostReply | HostEvent;
  } catch {
    return;
  }

  if ('event' in parsed && parsed.event === 'docChanged') {
    for (const listener of docListeners) listener(parsed.key, parsed.value);
    return;
  }

  const reply = parsed as HostReply;
  const entry = pending.get(reply.id);
  if (!entry) return;
  pending.delete(reply.id);
  clearTimeout(entry.timer);
  if (reply.ok) entry.resolve(reply.value);
  else entry.reject(new Error(reply.error));
}

function send(json: string): boolean {
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

function call<T>(verb: string, payload?: unknown, timeoutMs: number = CALL_TIMEOUT_MS): Promise<T> {
  const id = nextId++;
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => {
      if (pending.delete(id)) reject(new Error(`bridgething settings call '${verb}' timed out after ${timeoutMs}ms`));
    }, timeoutMs);
    pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
    try {
      if (!send(JSON.stringify({ id, verb, payload }))) throw new Error(HOST_MISSING);
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
  return message === HOST_MISSING || message.startsWith('unknown settings bridge verb');
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

function decodeUtf8(bytes: Uint8Array): string | null {
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

function base64FromBytes(bytes: Uint8Array): string {
  const chunk = 0x8000;
  let binary = '';
  for (let at = 0; at < bytes.length; at += chunk) {
    binary += String.fromCharCode(...bytes.subarray(at, at + chunk));
  }
  return btoa(binary);
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

export const settings = {
  context(): Promise<SettingsContext> {
    return call<SettingsContext>('context');
  },
  config: {
    fields(): Promise<ConfigField[]> {
      return call<ConfigField[]>('config.fields');
    },
    list(): Promise<ConfigEntry[]> {
      return call<ConfigEntry[]>('config.list');
    },
    set(key: string, value: string): Promise<KeyValue> {
      return call<KeyValue>('config.set', { key, value });
    },
    delete(key: string): Promise<KeyValue> {
      return call<KeyValue>('config.delete', { key });
    },
  },
  doc: {
    get(key: string): Promise<KeyValue> {
      return call<KeyValue>('doc.get', { key });
    },
    list(): Promise<DocEntry[]> {
      return call<DocEntry[]>('doc.list');
    },
    set(key: string, value: string): Promise<KeyValue> {
      return call<KeyValue>('doc.set', { key, value });
    },
    delete(key: string): Promise<{ key: string; value: null }> {
      return call<{ key: string; value: null }>('doc.delete', { key });
    },
  },
  async fetch(input: RequestInfo | URL, init?: SettingsRequestInit): Promise<Response> {
    const verb = await toVerbRequest(input, init);
    const budget = verb.timeoutMs === undefined ? CALL_TIMEOUT_MS : verb.timeoutMs + FETCH_TIMEOUT_SLACK_MS;
    try {
      return toResponse(await call<FetchVerbReply>('fetch', verb, Math.max(CALL_TIMEOUT_MS, budget)));
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
        const reply = await call<AuthorizeVerbReply>('auth.authorize', { url: String(url) }, AUTHORIZE_TIMEOUT_MS);
        return new URL(reply.url);
      } catch (err) {
        throw authorizeFailure(err);
      }
    },
  },
  onDocChanged(listener: DocChangedListener): () => void {
    docListeners.add(listener);
    return () => docListeners.delete(listener);
  },
  done(): void {
    if (!send(JSON.stringify({ id: nextId++, verb: 'done' }))) throw new Error(HOST_MISSING);
  },
};
