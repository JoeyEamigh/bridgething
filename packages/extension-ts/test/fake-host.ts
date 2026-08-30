import type { ExtensionHost } from '../src/host.js';
import type { ExtensionMessage, HostMessage } from '../src/protocol.js';

export class FakeHost implements ExtensionHost {
  readonly readable: ReadableStream<Uint8Array>;
  readonly writable: WritableStream<Uint8Array>;
  readonly written: ExtensionMessage[] = [];
  exitCode: number | undefined;

  private controller!: ReadableStreamDefaultController<Uint8Array>;
  private readonly encoder = new TextEncoder();
  private readonly decoder = new TextDecoder();
  private buffered = '';
  private waiters: (() => void)[] = [];

  constructor() {
    this.readable = new ReadableStream<Uint8Array>({
      start: controller => {
        this.controller = controller;
      },
    });
    this.writable = new WritableStream<Uint8Array>({
      write: chunk => {
        this.buffered += this.decoder.decode(chunk, { stream: true });
        for (let cut = this.buffered.indexOf('\n'); cut >= 0; cut = this.buffered.indexOf('\n')) {
          const line = this.buffered.slice(0, cut);
          this.buffered = this.buffered.slice(cut + 1);
          if (line.trim().length > 0) this.written.push(JSON.parse(line) as ExtensionMessage);
        }
        const waiting = this.waiters;
        this.waiters = [];
        for (const wake of waiting) wake();
      },
    });
  }

  exit(code: number): void {
    this.exitCode = code;
  }

  send(...messages: HostMessage[]): void {
    for (const message of messages) {
      this.controller.enqueue(this.encoder.encode(`${JSON.stringify(message)}\n`));
    }
  }

  sendRaw(line: string): void {
    this.controller.enqueue(this.encoder.encode(`${line}\n`));
  }

  sendChunked(raw: string, sizes: number[]): void {
    const bytes = this.encoder.encode(raw);
    let at = 0;
    for (const size of sizes) {
      this.controller.enqueue(bytes.slice(at, at + size));
      at += size;
    }
    if (at < bytes.length) this.controller.enqueue(bytes.slice(at));
  }

  close(): void {
    this.controller.close();
  }

  async waitFor(count: number): Promise<ExtensionMessage[]> {
    while (this.written.length < count) {
      await new Promise<void>(resolve => this.waiters.push(resolve));
    }
    return this.written;
  }

  async expect<T extends ExtensionMessage['t']>(t: T): Promise<Extract<ExtensionMessage, { t: T }>> {
    for (;;) {
      const found = this.written.find(message => message.t === t);
      if (found) return found as Extract<ExtensionMessage, { t: T }>;
      await new Promise<void>(resolve => this.waiters.push(resolve));
    }
  }
}

export function hello(): HostMessage {
  return {
    t: 'hello',
    api: 1,
    webapp: { id: '019e6701-13f8-71b5-ba04-85d326630e98', name: 'test-app', version: '0.1.0' },
    dataDir: '/tmp/bridgething-extension-test',
  };
}

export function connected(device: string, overrides: Partial<Extract<HostMessage, { t: 'device.connected' }>> = {}) {
  return {
    t: 'device.connected' as const,
    device,
    name: `thing-${device}`,
    config: {},
    active: true,
    ...overrides,
  };
}
