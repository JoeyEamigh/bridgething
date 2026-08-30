import type {
  AuthBridge,
  ConfigListener,
  Device,
  DeviceEvent,
  DeviceListener,
  DeviceRef,
  ExtensionContext,
  ExtensionLog,
  ExtensionSpec,
  KvStore,
  MessageListener,
  Unsubscribe,
} from './context.js';
import { ExtensionError, LineWriter, readLines, type ExtensionHost } from './host.js';
import { fromWire, intoWire, type SendableMessage } from './message.js';
import {
  EXTENSION_API_VERSION,
  type ExtensionMessage,
  type ExtensionWebapp,
  type HostMessage,
  type LogLevel,
} from './protocol.js';

type Pending = {
  resolve(value: unknown): void;
  reject(reason: ExtensionError): void;
};

function describe(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value instanceof Error) return value.stack ?? `${value.name}: ${value.message}`;
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

class DeviceImpl implements Device {
  name = '';
  active = false;
  connected = false;
  config: Record<string, string> = {};

  constructor(
    readonly id: string,
    private readonly runtime: ExtensionRuntime,
  ) {}

  send(message: SendableMessage): void {
    this.runtime.forward(this.id, message);
  }
}

export class ExtensionRuntime implements ExtensionContext {
  private readonly writer: LineWriter;
  private readonly devices_ = new Map<string, DeviceImpl>();
  private readonly pending = new Map<string, Pending>();
  private readonly onDevice = new Set<DeviceListener>();
  private readonly onMessage = new Set<MessageListener>();
  private readonly onConfig = new Set<ConfigListener>();
  private nextRequestId = 1;
  private started = false;
  private finishing: Promise<void> | undefined;
  private settled: ExtensionError | undefined;
  private helloApi = EXTENSION_API_VERSION;
  private helloWebapp: ExtensionWebapp = { id: '', name: '', version: '' };
  private helloDataDir = '';
  private reader: ReadableStreamDefaultReader<Uint8Array> | undefined;

  constructor(
    private readonly spec: ExtensionSpec,
    private readonly host: ExtensionHost,
  ) {
    this.writer = new LineWriter(host.writable);
  }

  get api(): number {
    return this.helloApi;
  }

  get webapp(): ExtensionWebapp {
    return this.helloWebapp;
  }

  get dataDir(): string {
    return this.helloDataDir;
  }

  get devices(): readonly Device[] {
    return [...this.devices_.values()].filter(device => device.connected);
  }

  device(id: string): Device {
    return this.ensure(id);
  }

  broadcast(message: SendableMessage): void {
    this.forward(undefined, message);
  }

  config(device: DeviceRef): Readonly<Record<string, string>> {
    const id = typeof device === 'string' ? device : device.id;
    return this.devices_.get(id)?.config ?? {};
  }

  on(event: 'device', listener: DeviceListener): Unsubscribe;
  on(event: 'message', listener: MessageListener): Unsubscribe;
  on(event: 'config', listener: ConfigListener): Unsubscribe;
  on(event: 'device' | 'message' | 'config', listener: DeviceListener | MessageListener | ConfigListener): Unsubscribe {
    if (event === 'device') {
      const fn = listener as DeviceListener;
      this.onDevice.add(fn);
      return () => this.onDevice.delete(fn);
    }
    if (event === 'message') {
      const fn = listener as MessageListener;
      this.onMessage.add(fn);
      return () => this.onMessage.delete(fn);
    }
    const fn = listener as ConfigListener;
    this.onConfig.add(fn);
    return () => this.onConfig.delete(fn);
  }

  readonly kv: KvStore = {
    get: <T = unknown>(key: string): Promise<T | undefined> =>
      this.request(id => ({ t: 'kv.get', id, key })).then(value => (value === null ? undefined : (value as T))),
    set: (key: string, value: unknown): Promise<void> =>
      this.request(id => ({ t: 'kv.set', id, key, value })).then(() => undefined),
    delete: (key: string): Promise<void> => this.request(id => ({ t: 'kv.delete', id, key })).then(() => undefined),
    list: (): Promise<string[]> => this.request(id => ({ t: 'kv.list', id })).then(value => (value ?? []) as string[]),
  };

  readonly auth: AuthBridge = {
    authorize: (url: string): Promise<string> =>
      this.request(id => ({ t: 'auth.authorize', id, url })).then(value => String(value)),
  };

  readonly log: ExtensionLog = {
    debug: (...args: unknown[]) => this.log.log('debug', ...args),
    info: (...args: unknown[]) => this.log.log('info', ...args),
    warn: (...args: unknown[]) => this.log.log('warn', ...args),
    error: (...args: unknown[]) => this.log.log('error', ...args),
    log: (level: LogLevel, ...args: unknown[]) => {
      this.emit({ t: 'log', level, message: args.map(describe).join(' ') });
    },
  };

  forward(device: string | undefined, message: SendableMessage): void {
    const wire = intoWire(message);
    this.emit(device === undefined ? { t: 'device.send', message: wire } : { t: 'device.send', device, message: wire });
  }

  async run(): Promise<void> {
    this.reader = this.host.readable.getReader();
    try {
      for await (const line of readLines(this.reader)) {
        const message = this.parse(line);
        if (!message) continue;
        if (message.t === 'stop') {
          await this.finish(0);
          return;
        }
        this.dispatch(message);
      }
    } catch (err) {
      await this.finish(1, new ExtensionError(`stdin read failed: ${describe(err)}`, 'disconnected'));
      return;
    }
    await this.finish(0);
  }

  private ensure(id: string): DeviceImpl {
    const existing = this.devices_.get(id);
    if (existing) return existing;
    const fresh = new DeviceImpl(id, this);
    this.devices_.set(id, fresh);
    return fresh;
  }

  private parse(line: string): HostMessage | undefined {
    try {
      return JSON.parse(line) as HostMessage;
    } catch (err) {
      this.log.error(`unparseable line from host: ${describe(err)}`);
      return undefined;
    }
  }

  private dispatch(message: Exclude<HostMessage, { t: 'stop' }>): void {
    switch (message.t) {
      case 'hello': {
        this.helloApi = message.api;
        this.helloWebapp = message.webapp;
        this.helloDataDir = message.dataDir;
        this.begin();
        return;
      }
      case 'device.connected': {
        const device = this.ensure(message.device);
        device.name = message.name;
        device.config = { ...message.config };
        device.active = message.active;
        device.connected = true;
        this.announce({ type: 'connected', device });
        return;
      }
      case 'device.disconnected': {
        const device = this.devices_.get(message.device);
        if (!device) return;
        device.connected = false;
        device.active = false;
        this.announce({ type: 'disconnected', device });
        return;
      }
      case 'device.active': {
        const device = this.ensure(message.device);
        device.active = message.active;
        this.announce({ type: 'active', device });
        return;
      }
      case 'device.message': {
        const device = this.ensure(message.device);
        const forwarded = this.guard(() => fromWire(message.message));
        if (!forwarded) return;
        for (const listener of [...this.onMessage]) this.guard(() => listener(device, forwarded));
        return;
      }
      case 'config.changed': {
        const device = this.ensure(message.device);
        const next = { ...device.config };
        if (message.value === null) delete next[message.key];
        else next[message.key] = message.value;
        device.config = next;
        for (const listener of [...this.onConfig]) {
          this.guard(() => listener(device, message.key, message.value));
        }
        return;
      }
      case 'reply': {
        const waiting = this.pending.get(message.id);
        if (!waiting) return;
        this.pending.delete(message.id);
        if (message.ok) waiting.resolve(message.value);
        else waiting.reject(new ExtensionError(message.error, 'host-error'));
        return;
      }
    }
  }

  private begin(): void {
    if (this.started) return;
    this.started = true;
    let starting: void | Promise<void>;
    try {
      starting = this.spec.start(this);
    } catch (err) {
      this.abortStart(err);
      return;
    }
    void Promise.resolve(starting).then(
      () => this.emit({ t: 'ready' }),
      (err: unknown) => this.abortStart(err),
    );
  }

  private abortStart(err: unknown): void {
    this.log.error(`start failed: ${describe(err)}`);
    void this.finish(1, new ExtensionError(describe(err), 'host-error'));
  }

  private announce(event: DeviceEvent): void {
    for (const listener of [...this.onDevice]) this.guard(() => listener(event));
  }

  private guard<T>(run: () => T): T | undefined {
    try {
      return run();
    } catch (err) {
      this.log.error(describe(err));
      return undefined;
    }
  }

  private emit(message: ExtensionMessage): void {
    this.writer.write(message).catch((err: unknown) => {
      void this.finish(1, new ExtensionError(`stdout write failed: ${describe(err)}`, 'write-failed'));
    });
  }

  private request(build: (id: string) => ExtensionMessage): Promise<unknown> {
    if (this.settled) {
      const advice = 'kv and auth cannot be reached once shutdown has begun, so persist eagerly rather than from stop';
      return Promise.reject(new ExtensionError(`${this.settled.message}; ${advice}`, this.settled.kind));
    }
    const id = String(this.nextRequestId++);
    return new Promise<unknown>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.writer.write(build(id)).catch((err: unknown) => {
        if (!this.pending.delete(id)) return;
        reject(new ExtensionError(`stdout write failed: ${describe(err)}`, 'write-failed'));
      });
    });
  }

  private finish(code: number, reason?: ExtensionError): Promise<void> {
    this.finishing ??= this.shutdown(code, reason);
    return this.finishing;
  }

  private async shutdown(code: number, reason?: ExtensionError): Promise<void> {
    const failure = reason ?? new ExtensionError('the host closed the connection', 'disconnected');
    this.settled = failure;

    await this.reader?.cancel().catch(() => undefined);

    for (const waiting of [...this.pending.values()]) waiting.reject(failure);
    this.pending.clear();

    try {
      await this.spec.stop?.();
    } catch (err) {
      await this.writer
        .write({ t: 'log', level: 'error', message: `stop failed: ${describe(err)}` })
        .catch(() => undefined);
    }

    await this.writer.close();
    this.host.exit(code);
  }
}
