import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { EXTENSION_DATA_DIR, ExtensionDevHost } from '../src/extension.ts';
import { frame, FrameAccumulator, freshMsgId, parseUuid, uuidToString, type GatewayMsg } from '../src/gateway.ts';

const APP = '019e6701-13f8-71b5-ba04-85d326630e98';
const OTHER = '019e6701-13f8-71b5-ba04-85d326630e99';
const SETTLE_MS = 60_000;

type Seen = { type: string; event: string; data: unknown };

type Sock = { send(data: Uint8Array): void; close(): void };

class FakeGateway {
  readonly seen: Seen[] = [];
  readonly logs: string[] = [];
  active: string | null = APP;
  config: Array<{ key: string; value: string }> = [{ key: 'greeting', value: 'hi' }];
  private socket: Sock | null = null;
  private readonly server: ReturnType<typeof Bun.serve>;
  private readonly waiters: Array<() => void> = [];

  constructor() {
    const gateway = this;
    this.server = Bun.serve<{ acc: FrameAccumulator }>({
      port: 0,
      fetch(req, server) {
        return server.upgrade(req, { data: { acc: new FrameAccumulator() } })
          ? undefined
          : new Response('gateway only', { status: 426 });
      },
      websocket: {
        open(ws) {
          gateway.socket = ws;
          gateway.notify();
        },
        message(ws, raw) {
          if (typeof raw === 'string') return;
          ws.data.acc.append(new Uint8Array(raw));
          for (let msg = ws.data.acc.next(); msg !== null; msg = ws.data.acc.next()) gateway.handle(ws, msg);
        },
        close(ws) {
          if (gateway.socket === ws) gateway.socket = null;
          gateway.notify();
        },
      },
    });
  }

  get port(): number {
    return this.server.port!;
  }

  get connected(): boolean {
    return this.socket !== null;
  }

  push(data: unknown): void {
    this.socket?.send(frame({ id: freshMsgId(), meta: { kind: 'event' }, data }));
  }

  drop(): void {
    this.socket?.close();
  }

  stop(): void {
    this.server.stop(true);
  }

  async until(predicate: () => boolean, what: string, timeout = SETTLE_MS): Promise<void> {
    const deadline = Date.now() + timeout;
    while (!predicate()) {
      if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}; saw ${JSON.stringify(this.seen)}`);
      await new Promise<void>(res => {
        const timer = setTimeout(res, 50);
        this.waiters.push(() => {
          clearTimeout(timer);
          res();
        });
      });
    }
  }

  running(): string[][] {
    return this.seen
      .filter(s => s.type === 'forward' && s.event === 'extensionsRunning')
      .map(s => ((s.data as { webapps: Uint8Array[] }).webapps ?? []).map(uuidToString));
  }

  routed(): Array<{ webapp: string; message: { encoding: string; data: unknown } }> {
    return this.seen
      .filter(s => s.type === 'forward' && s.event === 'routed')
      .map(s => {
        const routed = s.data as { webapp: Uint8Array; message: { encoding: string; data: unknown } };
        return { webapp: uuidToString(routed.webapp), message: routed.message };
      });
  }

  private notify(): void {
    for (const waiter of this.waiters.splice(0)) waiter();
  }

  private handle(ws: Sock, msg: GatewayMsg): void {
    const outer = msg.data as { type: string; data: { event: string; data?: unknown } };
    if (msg.meta.kind === 'event') {
      this.seen.push({ type: outer.type, event: outer.data.event, data: outer.data.data });
      this.notify();
      return;
    }
    const reply = (data: unknown) =>
      ws.send(frame({ id: freshMsgId(), meta: { kind: 'response', data: { requestId: msg.id } }, data }));
    const key = `${outer.type}.${outer.data.event}`;
    if (key === 'webapp.getActive') {
      reply({
        type: 'webapp',
        data: { event: 'active', data: { id: this.active ? parseUuid(this.active) : null, name: 'weather' } },
      });
    } else if (key === 'webapp.configList') {
      reply({ type: 'webapp', data: { event: 'configList', data: { entries: this.config } } });
    } else if (key === 'system.deviceGetNickname') {
      reply({ type: 'system', data: { event: 'deviceNickname', data: { nickname: 'bench thing' } } });
    } else {
      reply({ type: 'ack' });
    }
  }
}

const FIXTURE = (
  marker: string,
  imports = '',
) => `import { asJson, defineExtension, json } from '@bridgething/extension';
${imports}
defineExtension({
  async start(ctx) {
    ctx.on('device', event => {
      if (event.type === 'connected') {
        event.device.send(json({ ${marker}: ctx.config(event.device).greeting ?? null, name: event.device.name }));
      }
      if (event.type === 'active') event.device.send(json({ active: event.device.active }));
    });
    ctx.on('message', (device, message) => device.send(json({ echo: asJson(message) })));
    ctx.on('config', (device, key, value) => device.send(json({ config: [key, value] })));
    await ctx.kv.set('seen', 1);
    ctx.log.info('fixture up');
  },
});
`;

let root = '';
let gateway: FakeGateway;
let host: ExtensionDevHost;
const logs: string[] = [];

beforeAll(async () => {
  root = mkdtempSync(join(tmpdir(), 'bridgething-extension-host-'));
  symlinkSync(resolve(import.meta.dir, '..', 'node_modules'), join(root, 'node_modules'), 'dir');
  writeFileSync(join(root, 'package.json'), JSON.stringify({ name: 'fixture', private: true, type: 'module' }));
  mkdirSync(join(root, 'extension'));
  writeFileSync(join(root, 'extension', 'main.ts'), FIXTURE('hello'));
  gateway = new FakeGateway();
  host = new ExtensionDevHost({
    root,
    manifest: {
      id: APP,
      name: 'weather',
      version: '1.2.0',
      extension: { entry: 'extension/desktop.mjs', permissions: ['sys:hostname', 'env'], api: 1 },
    },
    target: { name: 'bench', host: '127.0.0.1', port: gateway.port },
    log: {
      info: line => logs.push(`info ${line}`),
      warn: line => logs.push(`warn ${line}`),
      error: line => logs.push(`error ${line}`),
    },
  });
  await host.start();
}, 120_000);

afterAll(async () => {
  await host?.close();
  gateway?.stop();
  if (root) rmSync(root, { recursive: true, force: true });
});

describe('the extension dev host', () => {
  test('bundles the entry, runs it under deno, and reports it running to the daemon once it is ready', async () => {
    await gateway.until(() => gateway.running().some(set => set.includes(APP)), 'an extensionsRunning report');
    expect(host.running).toBe(true);
    expect(logs.some(line => line.includes('[weather] fixture up'))).toBe(true);
  }, 120_000);

  test('hands the extension the device with its config and carries its first send to the daemon', async () => {
    await gateway.until(() => gateway.routed().length > 0, 'the connect greeting');
    expect(gateway.routed()[0]).toEqual({
      webapp: APP,
      message: { encoding: 'json', data: { hello: 'hi', name: 'bench thing' } },
    });
  });

  test('persists kv writes as the same json file the desktop host keeps', () => {
    expect(JSON.parse(readFileSync(join(root, EXTENSION_DATA_DIR, 'kv.json'), 'utf8'))).toEqual({ seen: 1 });
  });

  test('delivers a forward from the webapp and carries the reply back', async () => {
    const before = gateway.routed().length;
    gateway.push({
      type: 'forward',
      data: { event: 'routed', data: { webapp: parseUuid(APP), message: { encoding: 'json', data: { ping: 1 } } } },
    });
    await gateway.until(() => gateway.routed().length > before, 'the echo');
    expect(gateway.routed().at(-1)?.message).toEqual({ encoding: 'json', data: { echo: { ping: 1 } } });
  });

  test('ignores a forward addressed to another webapp', async () => {
    const before = gateway.routed().length;
    gateway.push({
      type: 'forward',
      data: { event: 'routed', data: { webapp: parseUuid(OTHER), message: { encoding: 'json', data: { ping: 2 } } } },
    });
    await new Promise(res => setTimeout(res, 500));
    expect(gateway.routed().length).toBe(before);
  });

  test('relays an active-webapp change and a config change', async () => {
    const before = gateway.routed().length;
    gateway.push({ type: 'webapp', data: { event: 'activeChanged', data: { id: null, name: null, art: null } } });
    await gateway.until(() => gateway.routed().length > before, 'the active flag');
    expect(gateway.routed().at(-1)?.message).toEqual({ encoding: 'json', data: { active: false } });

    const again = gateway.routed().length;
    gateway.push({
      type: 'webapp',
      data: { event: 'configChanged', data: { id: parseUuid(APP), key: 'greeting', value: 'yo' } },
    });
    await gateway.until(() => gateway.routed().length > again, 'the config change');
    expect(gateway.routed().at(-1)?.message).toEqual({ encoding: 'json', data: { config: ['greeting', 'yo'] } });
    gateway.push({
      type: 'webapp',
      data: { event: 'activeChanged', data: { id: parseUuid(APP), name: 'weather', art: null } },
    });
    await gateway.until(() => gateway.routed().length > again + 1, 'the active flag back');
  });

  test('a save rebuilds and restarts the extension, and an npm: specifier resolves at runtime', async () => {
    const runs = gateway.running().length;
    writeFileSync(
      join(root, 'extension', 'main.ts'),
      FIXTURE(
        'hello2',
        "import chalk from 'npm:chalk@5';\nif (typeof chalk.green !== 'function') throw new Error('chalk');",
      ),
    );
    await gateway.until(() => gateway.running().length >= runs + 2, 'the restart to be reported');
    expect(gateway.running().slice(runs)).toEqual([[], [APP]]);
    await gateway.until(
      () => gateway.routed().some(r => (r.message.data as { hello2?: string }).hello2 === 'yo'),
      'the rebuilt extension to greet with the updated config',
    );
  }, 120_000);

  test('a dropped link disconnects the device and a new one reconnects it', async () => {
    const before = gateway.routed().length;
    gateway.drop();
    await gateway.until(() => !gateway.connected, 'the socket to close');
    await gateway.until(() => gateway.connected, 'the host to reconnect');
    await gateway.until(() => gateway.routed().length > before, 'the greeting after relink');
    expect(gateway.running().at(-1)).toEqual([APP]);
  });

  test('closing stops the extension and clears the running set', async () => {
    await host.close();
    await gateway.until(() => gateway.running().at(-1)?.length === 0, 'the empty running set', 5_000);
    await gateway.until(() => !gateway.connected, 'the link to close', 5_000);
    expect(host.running).toBe(false);
    expect(existsSync(join(root, EXTENSION_DATA_DIR, 'build', 'desktop.mjs'))).toBe(true);
  });
});
