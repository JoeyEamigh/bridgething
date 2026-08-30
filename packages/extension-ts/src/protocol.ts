/** Host protocol revision this package speaks. The host refuses a mismatch. */
export const EXTENSION_API_VERSION = 1;

/** Severity accepted by `ctx.log`. */
export type LogLevel = 'debug' | 'info' | 'warn' | 'error';

/** Identity of the webapp this extension ships inside, as the host reports it. */
export type ExtensionWebapp = {
  id: string;
  name: string;
  version: string;
};

/**
 * `ForwardMessage` as it appears on stdio. The transport is JSON lines, so `binary` payloads
 * are base64.
 */
export type WireForwardMessage =
  | { encoding: 'text'; data: string }
  | { encoding: 'json'; data: unknown }
  | { encoding: 'binary'; data: string };

type NoFields = Record<never, never>;

/** Every `t` the host writes. `HostMessage` derives its arms from this list. */
export const HOST_MESSAGE_TYPES = [
  'hello',
  'device.connected',
  'device.disconnected',
  'device.active',
  'device.message',
  'config.changed',
  'reply',
  'stop',
] as const;

export type HostMessageType = (typeof HOST_MESSAGE_TYPES)[number];

type HostPayloads = {
  hello: { api: number; webapp: ExtensionWebapp; dataDir: string };
  'device.connected': { device: string; name: string; config: Record<string, string>; active: boolean };
  'device.disconnected': { device: string };
  'device.active': { device: string; active: boolean };
  'device.message': { device: string; message: WireForwardMessage };
  'config.changed': { device: string; key: string; value: string | null };
  reply: { id: string; ok: true; value?: unknown } | { id: string; ok: false; error: string };
  stop: NoFields;
};

/** A line the host writes to the extension's stdin. */
export type HostMessage = { [K in HostMessageType]: { t: K } & HostPayloads[K] }[HostMessageType];

/** Every `t` the extension writes. `ExtensionMessage` derives its arms from this list. */
export const EXTENSION_MESSAGE_TYPES = [
  'device.send',
  'kv.get',
  'kv.set',
  'kv.delete',
  'kv.list',
  'auth.authorize',
  'log',
  'ready',
] as const;

export type ExtensionMessageType = (typeof EXTENSION_MESSAGE_TYPES)[number];

type ExtensionPayloads = {
  'device.send': { device?: string; message: WireForwardMessage };
  'kv.get': { id: string; key: string };
  'kv.set': { id: string; key: string; value: unknown };
  'kv.delete': { id: string; key: string };
  'kv.list': { id: string };
  'auth.authorize': { id: string; url: string };
  log: { level: LogLevel; message: string };
  ready: NoFields;
};

/** A line the extension writes to its stdout. */
export type ExtensionMessage = {
  [K in ExtensionMessageType]: { t: K } & ExtensionPayloads[K];
}[ExtensionMessageType];
