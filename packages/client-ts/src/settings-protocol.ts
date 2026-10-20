import type { ConfigEntry, ConfigField, DocEntry } from '@bridgething/lib';
import type { WebappBlobEntry } from '@bridgething/lib/gateway';

import type { Theme } from './theme.js';

export type { ConfigEntry, ConfigField, DocEntry } from '@bridgething/lib';
export type { WebappBlobEntry } from '@bridgething/lib/gateway';
export type { Theme, ThemeColor, ThemeFont, ThemeScheme, ThemeText } from './theme.js';

export type SettingsContext = { webappId: string; name: string; version: string; deviceId: string; theme?: Theme };
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
export type KeyRequest = { key: string };
export type KeyValueRequest = { key: string; value: string };
export type KeyValue = { key: string; value: string | null };
export type PickKind = 'image' | 'video' | 'any';
export type PickVerbRequest = { kind: PickKind; cover?: [width: number, height: number] };
export type PickedMedia = {
  handle: string;
  mime: string;
  size: number;
  width?: number;
  height?: number;
  durationMs?: number;
  preview?: string;
};
export type BlobCreateRequest = { mime: string };
export type BlobHandleReply = { handle: string };
export type BlobAppendRequest = { handle: string; data: string };
export type BlobAppendReply = { handle: string; size: number };
export type BlobPutRequest = { key: string; handle: string };
export type ByteProgress = { sent: number; total: number };

export type SettingsVerbs = {
  context: { request: void; reply: SettingsContext };
  'config.fields': { request: void; reply: ConfigField[] };
  'config.list': { request: void; reply: ConfigEntry[] };
  'config.set': { request: KeyValueRequest; reply: KeyValue };
  'config.delete': { request: KeyRequest; reply: KeyValue };
  'doc.get': { request: KeyRequest; reply: KeyValue };
  'doc.list': { request: void; reply: DocEntry[] };
  'doc.set': { request: KeyValueRequest; reply: KeyValue };
  'doc.delete': { request: KeyRequest; reply: { key: string; value: null } };
  fetch: { request: FetchVerbRequest; reply: FetchVerbReply };
  'auth.authorize': { request: AuthorizeVerbRequest; reply: AuthorizeVerbReply };
  pick: { request: PickVerbRequest; reply: PickedMedia | null };
  'blob.create': { request: BlobCreateRequest; reply: BlobHandleReply };
  'blob.append': { request: BlobAppendRequest; reply: BlobAppendReply };
  'blob.put': { request: BlobPutRequest; reply: WebappBlobEntry; progress: ByteProgress };
  'blob.delete': { request: KeyRequest; reply: { key: string } };
  'blob.list': { request: void; reply: WebappBlobEntry[] };
};

export type SettingsVerb = keyof SettingsVerbs;
export type SettingsPayload<V extends SettingsVerb> = SettingsVerbs[V]['request'];
export type SettingsResult<V extends SettingsVerb> = SettingsVerbs[V]['reply'];
export type SettingsProgress<V extends SettingsVerb> = SettingsVerbs[V] extends { progress: infer P } ? P : never;

export type SettingsRequest =
  | { [V in SettingsVerb]: { id: number; verb: V; payload?: SettingsPayload<V>; epoch?: number } }[SettingsVerb]
  | { id: number; verb: 'done'; epoch?: number }
  | { id: number; verb: 'release'; epoch: number };
export type SettingsReply =
  | { id: number; ok: true; value: unknown; epoch?: number }
  | { id: number; ok: false; error: string };
export type SettingsNotice =
  | { event: 'docChanged'; key: string; value: string | null }
  | { event: 'themeChanged'; theme: Theme }
  | { event: 'refresh'; context: SettingsContext };
export type SettingsEvent =
  | SettingsNotice
  | { event: 'contextChanged'; context: SettingsContext; epoch: number }
  | ({ event: 'progress'; id: number } & ByteProgress);
export type SettingsMessage = SettingsReply | SettingsEvent;

export type SettingsHost = {
  [V in SettingsVerb]: (
    payload: SettingsPayload<V>,
    report: (progress: SettingsProgress<V>) => void,
  ) => Promise<SettingsResult<V>>;
} & { done(): void };

export type SettingsSessionOptions<T> = {
  target: T | null;
  bind: (target: T) => SettingsHost;
  deliver: (message: SettingsMessage) => void;
  explain?: (err: unknown) => string;
};

export type SettingsSession<T> = {
  readonly target: T | null;
  readonly closed: boolean;
  receive(raw: unknown): void;
  retarget(target: T, context: SettingsContext): void;
  notify(notice: SettingsNotice): void;
  close(): void;
};

export const UNKNOWN_VERB = 'unknown settings bridge verb';
export const NO_SETTINGS_TARGET = 'no app is selected for settings';
export const UNKNOWN_SETTINGS_EPOCH = 'settings bridge request names an epoch this page was never given or released';
export const PICK_BUSY = 'busy: a pick is already in flight';
export const MAX_COVER_EDGE = 0xffff_ffff;
export const BLOB_CHUNK_BYTES = 256 * 1024;

export type BlobData = Blob | ArrayBuffer | ArrayBufferView;

export function base64FromBytes(bytes: Uint8Array): string {
  const chunk = 0x8000;
  let binary = '';
  for (let at = 0; at < bytes.length; at += chunk) {
    binary += String.fromCharCode(...bytes.subarray(at, at + chunk));
  }
  return btoa(binary);
}

export async function* blobChunks(data: BlobData): AsyncGenerator<Uint8Array> {
  if (data instanceof Blob) {
    for (let at = 0; at < data.size; at += BLOB_CHUNK_BYTES) {
      yield new Uint8Array(await data.slice(at, at + BLOB_CHUNK_BYTES).arrayBuffer());
    }
    return;
  }
  const bytes = ArrayBuffer.isView(data)
    ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
    : new Uint8Array(data);
  for (let at = 0; at < bytes.length; at += BLOB_CHUNK_BYTES) yield bytes.subarray(at, at + BLOB_CHUNK_BYTES);
}

export const DEFAULT_SETTINGS_KEY = 'default';

export type LoadedSettingsPage = { target: string | null; key: string; stamp: string | null };
export type SettingsSwitch = 'stay' | 'refresh' | 'retarget' | 'load';

export function settingsPageKey(target: string, settingsHash: string | null | undefined): string {
  return settingsHash ? `${target}#${settingsHash}` : DEFAULT_SETTINGS_KEY;
}

export function settingsSwitch(loaded: LoadedSettingsPage, target: string, key: string, stamp: string): SettingsSwitch {
  if (loaded.key !== key) return 'load';
  if (loaded.target !== target) return key === DEFAULT_SETTINGS_KEY ? 'retarget' : 'load';
  return loaded.stamp === stamp ? 'stay' : 'refresh';
}

type Fields = Record<string, unknown>;

function text(fields: Fields, name: string): string {
  const value = fields[name];
  if (typeof value !== 'string') throw new Error(`settings bridge payload needs a string ${name}`);
  return value;
}

const PICK_KINDS: readonly PickKind[] = ['image', 'video', 'any'];

function pickKind(fields: Fields): PickKind {
  const kind = text(fields, 'kind');
  if (!(PICK_KINDS as readonly string[]).includes(kind)) {
    throw new Error(`settings bridge pick kind must be one of ${PICK_KINDS.join(', ')}`);
  }
  return kind as PickKind;
}

function edge(value: unknown): number {
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 1 || value > MAX_COVER_EDGE) {
    throw new Error(`settings bridge pick cover edges must be whole numbers from 1 to ${MAX_COVER_EDGE}`);
  }
  return value;
}

function pick(fields: Fields): PickVerbRequest {
  const cover = fields.cover;
  if (cover === undefined || cover === null) return { kind: pickKind(fields) };
  if (!Array.isArray(cover) || cover.length !== 2) {
    throw new Error('settings bridge pick cover must be [width, height]');
  }
  return { kind: pickKind(fields), cover: [edge(cover[0]), edge(cover[1])] };
}

const none = (): void => undefined;
const key = (fields: Fields): KeyRequest => ({ key: text(fields, 'key') });
const keyValue = (fields: Fields): KeyValueRequest => ({ key: text(fields, 'key'), value: text(fields, 'value') });

const PAYLOADS: { [V in SettingsVerb]: (fields: Fields) => SettingsPayload<V> } = {
  context: none,
  'config.fields': none,
  'config.list': none,
  'config.set': keyValue,
  'config.delete': key,
  'doc.get': key,
  'doc.list': none,
  'doc.set': keyValue,
  'doc.delete': key,
  fetch: fields => ({ ...(fields as FetchVerbRequest), url: text(fields, 'url') }),
  'auth.authorize': fields => ({ url: text(fields, 'url') }),
  pick,
  'blob.create': fields => ({ mime: text(fields, 'mime') }),
  'blob.append': fields => ({ handle: text(fields, 'handle'), data: text(fields, 'data') }),
  'blob.put': fields => ({ key: text(fields, 'key'), handle: text(fields, 'handle') }),
  'blob.delete': key,
  'blob.list': none,
};

function isVerb(verb: string): verb is SettingsVerb {
  return Object.prototype.hasOwnProperty.call(PAYLOADS, verb);
}

function invoke<V extends SettingsVerb>(
  host: SettingsHost,
  verb: V,
  fields: Fields,
  report: (progress: ByteProgress) => void,
): Promise<SettingsResult<V>> {
  const handler = host[verb] as (
    payload: SettingsPayload<V>,
    report: (progress: ByteProgress) => void,
  ) => Promise<SettingsResult<V>>;
  return handler(PAYLOADS[verb](fields), report);
}

function describe(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function decodeSettingsMessage(raw: unknown): SettingsMessage | null {
  if (typeof raw !== 'string') return null;
  try {
    const parsed = JSON.parse(raw) as unknown;
    return typeof parsed === 'object' && parsed !== null ? (parsed as SettingsMessage) : null;
  } catch {
    return null;
  }
}

type DecodedRequest = { id: number; verb: string; fields: Fields; epoch: number };

function decodeSettingsRequest(raw: unknown): DecodedRequest | null {
  if (typeof raw !== 'string') return null;

  let request: Fields;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== 'object' || parsed === null) return null;
    request = parsed as Fields;
  } catch {
    return null;
  }

  const { id, verb, payload, epoch } = request;
  if (typeof id !== 'number' || typeof verb !== 'string') return null;
  return {
    id,
    verb,
    fields: typeof payload === 'object' && payload !== null ? (payload as Fields) : {},
    epoch: typeof epoch === 'number' ? epoch : 0,
  };
}

export function openSettingsSession<T>(options: SettingsSessionOptions<T>): SettingsSession<T> {
  const explain = options.explain ?? describe;
  let target = options.target;
  let current = 0;
  const hosts = new Map<number, SettingsHost | null>([[0, target === null ? null : options.bind(target)]]);
  let closed = false;

  const deliver = (message: SettingsMessage): void => {
    if (!closed) options.deliver(message);
  };

  const receive = (raw: unknown): void => {
    if (closed) return;
    const request = decodeSettingsRequest(raw);
    if (!request) return;

    const { id, verb, fields } = request;
    const now = current;
    if (verb === 'done') {
      if (request.epoch === now) hosts.get(now)?.done();
      return;
    }
    if (verb === 'release') {
      if (request.epoch < now) hosts.delete(request.epoch);
      return;
    }

    void (async () => {
      try {
        if (!isVerb(verb)) throw new Error(`${UNKNOWN_VERB}: ${verb}`);
        const at = verb === 'context' ? now : request.epoch;
        const bound = hosts.get(at);
        if (bound === undefined) throw new Error(UNKNOWN_SETTINGS_EPOCH);
        if (bound === null) throw new Error(NO_SETTINGS_TARGET);
        const report = ({ sent, total }: ByteProgress) => deliver({ event: 'progress', id, sent, total });
        const value = await invoke(bound, verb, fields, report);
        if (verb !== 'context') deliver({ id, ok: true, value });
        else if (current === now) deliver({ id, ok: true, value, epoch: now });
        else receive(raw);
      } catch (err) {
        deliver({ id, ok: false, error: explain(err) });
      }
    })();
  };

  return {
    get target() {
      return target;
    },
    get closed() {
      return closed;
    },
    receive,
    retarget(next, context) {
      if (closed) return;
      target = next;
      current += 1;
      hosts.set(current, options.bind(next));
      deliver({ event: 'contextChanged', context, epoch: current });
    },
    notify(notice) {
      deliver(notice);
    },
    close() {
      closed = true;
    },
  };
}
