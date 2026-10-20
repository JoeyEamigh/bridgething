import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createServer, type ViteDevServer } from 'vite';
import { DAEMON_PROXY_PATH } from '../src/daemon.ts';
import { BLOB_URL_PREFIX, daemonProxy } from '../src/dev.ts';

const DAEMON_BLOBS = resolve(import.meta.dir, '..', '..', '..', 'crates', 'core', 'src', 'state', 'blobs.rs');
const BYTES = new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x00, 0xff]);
const SHA = 'ab'.repeat(32);

type Seen = { path: string; search: string; range: string | null };

class FakeDaemon {
  readonly seen: Seen[] = [];
  readonly sockets: string[] = [];
  private readonly server: ReturnType<typeof Bun.serve>;

  constructor() {
    const daemon = this;
    this.server = Bun.serve({
      port: 0,
      hostname: '127.0.0.1',
      fetch(req, server) {
        const url = new URL(req.url);
        if (req.headers.get('upgrade') === 'websocket') {
          daemon.sockets.push(url.pathname);
          return server.upgrade(req, { data: null }) ? undefined : new Response('no upgrade', { status: 400 });
        }
        const range = req.headers.get('range');
        daemon.seen.push({ path: url.pathname, search: url.search, range });
        if (!url.pathname.startsWith('/.bridgething/blob/')) return new Response('not a blob', { status: 404 });
        const headers = { 'content-type': 'image/gif', etag: `"${SHA}"`, 'accept-ranges': 'bytes' };
        if (range === 'bytes=2-3') {
          return new Response(BYTES.subarray(2, 4), {
            status: 206,
            headers: { ...headers, 'content-range': `bytes 2-3/${BYTES.length}` },
          });
        }
        return new Response(BYTES, { headers });
      },
      websocket: {
        open(ws) {
          ws.send('hello from the daemon');
        },
        message() {},
      },
    });
  }

  get port(): number {
    return this.server.port!;
  }

  async stop(): Promise<void> {
    await this.server.stop(true);
  }
}

describe('the dev server in front of a daemon', () => {
  let daemon: FakeDaemon;
  let vite: ViteDevServer;
  let root: string;
  let origin: string;
  let previous: string | undefined;

  beforeAll(async () => {
    daemon = new FakeDaemon();
    previous = process.env.BRIDGETHING_DAEMON_URL;
    process.env.BRIDGETHING_DAEMON_URL = `ws://127.0.0.1:${daemon.port}/`;
    root = mkdtempSync(join(tmpdir(), 'bridgething-dev-proxy-'));
    writeFileSync(join(root, 'index.html'), '<!doctype html><title>the app shell</title>');
    vite = await createServer({
      configFile: false,
      root,
      logLevel: 'silent',
      server: { host: '127.0.0.1', port: 0, proxy: await daemonProxy() },
    });
    await vite.listen();
    const address = vite.httpServer!.address() as AddressInfo;
    origin = `127.0.0.1:${address.port}`;
  });

  afterAll(async () => {
    await vite.close();
    await daemon.stop();
    rmSync(root, { recursive: true, force: true });
    if (previous === undefined) delete process.env.BRIDGETHING_DAEMON_URL;
    else process.env.BRIDGETHING_DAEMON_URL = previous;
  });

  test('a blob url the daemon hands a webapp loads the daemon bytes', async () => {
    const res = await fetch(`http://${origin}/.bridgething/blob/bg%2Fhero%20gif?v=${SHA}`);

    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toBe('image/gif');
    expect(res.headers.get('etag')).toBe(`"${SHA}"`);
    expect(new Uint8Array(await res.arrayBuffer())).toEqual(BYTES);
    expect(daemon.seen.at(-1)).toEqual({
      path: '/.bridgething/blob/bg%2Fhero%20gif',
      search: `?v=${SHA}`,
      range: null,
    });
  });

  test('a range request for a video blob reaches the daemon as a range', async () => {
    const res = await fetch(`http://${origin}/.bridgething/blob/loop?v=${SHA}`, { headers: { range: 'bytes=2-3' } });

    expect(res.status).toBe(206);
    expect(res.headers.get('content-range')).toBe(`bytes 2-3/${BYTES.length}`);
    expect(new Uint8Array(await res.arrayBuffer())).toEqual(BYTES.subarray(2, 4));
    expect(daemon.seen.at(-1)?.range).toBe('bytes=2-3');
  });

  test('the daemon websocket still arrives at the daemon root', async () => {
    const greeting = await new Promise<string>((res, rej) => {
      const socket = new WebSocket(`ws://${origin}${DAEMON_PROXY_PATH}/`);
      socket.onmessage = event => {
        socket.close();
        res(String(event.data));
      };
      socket.onerror = () => rej(new Error('the proxied websocket failed'));
    });

    expect(greeting).toBe('hello from the daemon');
    expect(daemon.sockets).toEqual(['/']);
  });

  test('every other path stays with the dev server', async () => {
    const before = daemon.seen.length;
    const res = await fetch(`http://${origin}/some/route`, { headers: { accept: 'text/html' } });

    expect(await res.text()).toContain('the app shell');
    expect(daemon.seen.length).toBe(before);
  });
});

test('the proxied blob prefix is the one the daemon serves', () => {
  const declared = /pub const BLOB_URL_PREFIX: &str = "([^"]+)";/.exec(readFileSync(DAEMON_BLOBS, 'utf8'));

  expect(declared?.[1]).toBe(BLOB_URL_PREFIX);
});
