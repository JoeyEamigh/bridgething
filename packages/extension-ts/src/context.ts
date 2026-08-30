import type { ForwardMessage, SendableMessage } from './message.js';
import type { ExtensionWebapp, LogLevel } from './protocol.js';

/** Drops a listener registered with `ctx.on`. Safe to call more than once. */
export type Unsubscribe = () => void;

export type DeviceRef = string | { id: string };

/**
 * One Car Thing the host has announced to this extension. A handle stays valid for the life of the process.
 * A disconnected device keeps its last known `name` and `config`, with `connected` false.
 */
export interface Device {
  /** The device serial. Stable across reconnects and across host restarts. */
  readonly id: string;
  readonly name: string;
  /** True while this extension's webapp is the active webapp on this device. */
  readonly active: boolean;
  readonly connected: boolean;
  /** The webapp's settings for this device, keyed by config key from `manifest.json`. */
  readonly config: Readonly<Record<string, string>>;
  /**
   * Sends a message to this device's webapp. The host delivers it while the webapp is active,
   * and drops it otherwise.
   */
  send(message: SendableMessage): void;
}

/** Connect, disconnect, and active-webapp changes, in one stream. */
export type DeviceEvent = {
  type: 'connected' | 'disconnected' | 'active';
  device: Device;
};

export type DeviceListener = (event: DeviceEvent) => void;
export type MessageListener = (device: Device, message: ForwardMessage) => void;
/** `value` is null when the settings page clears the key, which removes it from `device.config`. */
export type ConfigListener = (device: Device, key: string, value: string | null) => void;

/**
 * Persistent JSON store for this extension, held in a file under `ctx.dataDir`.
 * Every method rejects once shutdown starts, so write as state changes.
 */
export interface KvStore {
  get<T = unknown>(key: string): Promise<T | undefined>;
  set(key: string, value: unknown): Promise<void>;
  delete(key: string): Promise<void>;
  list(): Promise<string[]>;
}

export interface AuthBridge {
  /**
   * Opens `url` in the user's browser. Resolves with the callback URL the host captured,
   * including its query string. Rejects if the user cancels.
   */
  authorize(url: string): Promise<string>;
}

/** Writes to the desktop app's log, prefixed with the webapp name. */
export interface ExtensionLog {
  debug(...args: unknown[]): void;
  info(...args: unknown[]): void;
  warn(...args: unknown[]): void;
  error(...args: unknown[]): void;
  log(level: LogLevel, ...args: unknown[]): void;
}

/** Everything an extension can reach. `start` receives it. */
export interface ExtensionContext {
  /** The host protocol revision. */
  readonly api: number;
  readonly webapp: ExtensionWebapp;
  /** Directory this extension may write to. The kv store file lives here. */
  readonly dataDir: string;
  /** The connected devices, recomputed on every read. */
  readonly devices: readonly Device[];
  /** Returns a handle for any device id, connected or not. */
  device(id: string): Device;
  /** Sends a message to every connected device where this webapp is active. */
  broadcast(message: SendableMessage): void;
  /** Returns the settings for a device. An id the host has not announced returns an empty object. */
  config(device: DeviceRef): Readonly<Record<string, string>>;
  /**
   * Registers a listener. Call `on` synchronously at the top of `start`. The host replays
   * connected devices right after `start` begins, so a listener registered after an `await`
   * can miss them.
   */
  on(event: 'device', listener: DeviceListener): Unsubscribe;
  on(event: 'message', listener: MessageListener): Unsubscribe;
  on(event: 'config', listener: ConfigListener): Unsubscribe;
  readonly kv: KvStore;
  readonly auth: AuthBridge;
  readonly log: ExtensionLog;
}

export type ExtensionSpec = {
  /**
   * Runs once, after the host connects. Register listeners synchronously, then do the rest of
   * your setup. The desktop app shows this extension as starting until the returned promise settles.
   */
  start(ctx: ExtensionContext): void | Promise<void>;
  /**
   * Runs before the process exits, on host shutdown or disconnect. Release timers, sockets, and
   * child processes here. `ctx.kv` and `ctx.auth` reject from this hook, so persist state as it changes.
   */
  stop?(): void | Promise<void>;
};
