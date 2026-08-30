import { spawn } from 'node:child_process';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { isIPv4, type AddressInfo } from 'node:net';
import { networkInterfaces } from 'node:os';
import { relative } from 'node:path';
import type { Logger, Plugin, ProxyOptions, ViteDevServer } from 'vite';
import { DAEMON_PROXY_PATH } from './daemon.js';
import { buildExtension, ExtensionDevHost, readManifest, type WebappManifest } from './extension.js';
import {
  deviceHostName,
  listWebapps,
  navigateKiosk,
  resolveGatewayTarget,
  resolveHost,
  switchTo,
  uuidToString,
  type GatewayTarget,
} from './gateway.js';

export { buildExtension } from './extension.js';

export const DEVICE_MODE = 'device';
export const AUTHORIZE_PATH = '/__extension/authorize';

const DAEMON_PORT = 8891;
const SWITCH_SETTLE_MS = 1_000;

export async function daemonProxyTarget(): Promise<string> {
  const explicit = process.env.BRIDGETHING_DAEMON_URL;
  if (explicit) return explicit.replace(/\/$/, '');
  return `ws://${await resolveHost(deviceHostName())}:${DAEMON_PORT}`;
}

export async function daemonProxy(): Promise<Record<string, ProxyOptions>> {
  return {
    [DAEMON_PROXY_PATH]: {
      target: await daemonProxyTarget(),
      ws: true,
      changeOrigin: true,
      rewrite: path => path.slice(DAEMON_PROXY_PATH.length) || '/',
    },
  };
}

export function bridgething(): Plugin {
  let command: 'serve' | 'build' = 'serve';
  let mode = '';
  let logger: Logger | undefined;
  let root = process.cwd();
  let publicDir = '';
  let outDir = 'dist';
  let device: DeviceSession | null = null;
  let host: ExtensionDevHost | null = null;

  return {
    name: 'bridgething',
    configResolved(config) {
      command = config.command;
      mode = config.mode;
      logger = config.logger;
      root = config.root;
      publicDir = config.publicDir;
      outDir = config.build.outDir;
    },
    configureServer(server) {
      if (!logger) return;
      const log = logger;
      const http = server.httpServer;
      if (!http) return;
      let manifest: WebappManifest;
      try {
        manifest = readManifest(publicDir);
      } catch (err) {
        log.warn(
          `bridgething: ${err instanceof Error ? err.message : String(err)}; device mode and extensions are off`,
        );
        return;
      }
      const extension = manifest.extension;
      if (extension)
        server.middlewares.use(
          AUTHORIZE_PATH,
          authorizePage(() => host),
        );

      http.once('listening', () => {
        void (async () => {
          const target = await resolveGatewayTarget();
          try {
            if (mode === DEVICE_MODE) device = await attachDevice(server, root, manifest, target, log);
            else if (extension) await makeActive(root, manifest, target, log);
          } catch (err) {
            log.error(`device mode: ${err instanceof Error ? err.message : String(err)}`);
          }
          if (!extension) return;
          const address = http.address() as AddressInfo | null;
          const started = new ExtensionDevHost({
            root,
            manifest: { ...manifest, extension },
            target,
            log,
            authorizePage: address ? `http://localhost:${address.port}${AUTHORIZE_PATH}` : undefined,
          });
          try {
            await started.start();
            host = started;
          } catch (err) {
            log.error(`extension: ${err instanceof Error ? err.message : String(err)}`);
          }
        })();
      });

      const close = server.close.bind(server);
      server.close = async () => {
        const ending = device;
        device = null;
        const stopping = host;
        host = null;
        await stopping?.close().catch((err: unknown) => {
          log.warn(`extension: did not stop cleanly: ${err instanceof Error ? err.message : String(err)}`);
        });
        if (ending) {
          await ending.release().catch((err: unknown) => {
            log.warn(
              `device mode: could not hand the screen back: ${err instanceof Error ? err.message : String(err)}`,
            );
          });
        }
        await close();
      };
      const onSignal = () => {
        server.close().finally(() => process.exit(0));
      };
      process.once('SIGINT', onSignal);
      process.once('SIGTERM', onSignal);
    },
    async closeBundle() {
      if (command !== 'build') return;
      const manifest = readManifest(publicDir);
      if (!manifest.extension) return;
      const outfile = await buildExtension(root, outDir, manifest.extension);
      logger?.info(`wrote ${relative(root, outfile)}`);
    },
  };
}

type DeviceSession = { release(): Promise<void> };

async function attachDevice(
  server: ViteDevServer,
  root: string,
  manifest: WebappManifest,
  target: GatewayTarget,
  log: Logger,
): Promise<DeviceSession> {
  if (!isIPv4(target.host)) throw new Error(`could not resolve ${target.name}; is the car thing plugged in?`);
  const local = hostAddressToward(target.host);
  if (!local) throw new Error(`no interface on this machine shares a subnet with ${target.name} (${target.host})`);
  const address = server.httpServer?.address() as AddressInfo | null;
  if (!address) throw new Error('the dev server is not listening');
  const url = `http://${local}:${address.port}/`;

  await installOnce(root, manifest, target, log);
  await activate(target, manifest.id);
  await new Promise(res => setTimeout(res, SWITCH_SETTLE_MS));
  await navigateKiosk(target, url);
  log.info(
    `car thing is showing ${url}; edits hot-reload on the device, ctrl-c hands the screen back to the installed build`,
  );

  return {
    async release() {
      await activate(target, manifest.id);
    },
  };
}

async function makeActive(root: string, manifest: WebappManifest, target: GatewayTarget, log: Logger): Promise<void> {
  const name = manifest.name ?? manifest.id;
  try {
    await installOnce(root, manifest, target, log);
    await activate(target, manifest.id);
    log.info(`${name} is the active webapp on the car thing, so forwards reach the extension`);
  } catch (err) {
    log.warn(
      `could not make ${name} active on the car thing (${err instanceof Error ? err.message : String(err)}); forwards only route while it is`,
    );
  }
}

async function installOnce(root: string, manifest: WebappManifest, target: GatewayTarget, log: Logger): Promise<void> {
  if (await installed(target, manifest.id)) return;
  log.info(`${manifest.name ?? manifest.id} is not on the car thing yet; installing it once so the daemon knows it`);
  await run('bun', ['run', 'push', '--no-switch', target.name], root);
}

async function installed(target: GatewayTarget, id: string): Promise<boolean> {
  const listed = await listWebapps(target);
  if (!listed.ok) throw new Error(listed.reason);
  const wanted = id.toLowerCase();
  return listed.value.webapps.some(webapp => uuidToString(webapp.id) === wanted);
}

async function activate(target: GatewayTarget, id: string): Promise<void> {
  const switched = await switchTo(target, id);
  if (!switched.ok) throw new Error(switched.reason);
}

function hostAddressToward(ip: string): string | null {
  const wanted = ipv4Bits(ip);
  if (wanted === null) return null;
  for (const entries of Object.values(networkInterfaces())) {
    for (const entry of entries ?? []) {
      if (entry.family !== 'IPv4' || entry.internal || !entry.cidr) continue;
      const prefix = Number(entry.cidr.split('/')[1]);
      const own = ipv4Bits(entry.address);
      if (own === null || !Number.isFinite(prefix)) continue;
      const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
      if ((own & mask) >>> 0 === (wanted & mask) >>> 0) return entry.address;
    }
  }
  return null;
}

function ipv4Bits(ip: string): number | null {
  const parts = ip.split('.').map(Number);
  if (parts.length !== 4 || parts.some(part => !Number.isInteger(part) || part < 0 || part > 255)) return null;
  return ((parts[0]! << 24) | (parts[1]! << 16) | (parts[2]! << 8) | parts[3]!) >>> 0;
}

function run(cmd: string, args: string[], cwd: string): Promise<void> {
  return new Promise<void>((res, rej) => {
    const child = spawn(cmd, args, { stdio: 'inherit', cwd });
    child.on('exit', code => (code === 0 ? res() : rej(new Error(`${cmd} ${args.join(' ')} exited ${code}`))));
    child.on('error', rej);
  });
}

type Middleware = (req: IncomingMessage, res: ServerResponse, next: () => void) => void;

function authorizePage(host: () => ExtensionDevHost | null): Middleware {
  return (req, res, next) => {
    if (req.method === 'GET') {
      res.setHeader('content-type', 'text/html; charset=utf-8');
      res.end(renderAuthorize(host()?.pendingAuthorize?.url ?? null, null));
      return;
    }
    if (req.method !== 'POST') {
      next();
      return;
    }
    let body = '';
    req.setEncoding('utf8');
    req.on('data', (chunk: string) => {
      body += chunk;
    });
    req.on('end', () => {
      const form = new URLSearchParams(body);
      const current = host();
      let note: string;
      if (!current) note = 'no extension is running';
      else if (form.has('cancel')) note = current.cancelAuthorize() ? 'cancelled' : 'nothing was waiting';
      else {
        const callback = normalizeCallback(form.get('url') ?? '');
        if (!callback) note = 'that is not a url';
        else note = current.settleAuthorize(callback) ? 'delivered to the extension' : 'nothing was waiting';
      }
      res.setHeader('content-type', 'text/html; charset=utf-8');
      res.end(renderAuthorize(current?.pendingAuthorize?.url ?? null, note));
    });
  };
}

function normalizeCallback(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  try {
    const url = new URL(trimmed);
    return `bridgething://oauth/callback${url.search}${url.hash}`;
  } catch {
    return null;
  }
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, ch => `&#${ch.charCodeAt(0)};`);
}

function renderAuthorize(pending: string | null, note: string | null): string {
  const status = pending
    ? `<p>the extension is waiting on <a href="${escapeHtml(pending)}" target="_blank" rel="noreferrer">${escapeHtml(pending)}</a>.</p>
<p>finish signing in there. the provider sends the browser to bridgething.com's callback page, which tries to open the desktop app; copy that page's address and paste it here.</p>
<form method="post">
  <input name="url" placeholder="https://bridgething.com/oauth/callback?code=..." autofocus />
  <button type="submit">deliver</button>
  <button type="submit" name="cancel" value="1">cancel</button>
</form>`
    : '<p>nothing is waiting for authorization. this page fills in when the extension calls <code>ctx.auth.authorize</code>.</p>';
  return `<!doctype html>
<meta charset="utf-8" />
<title>bridgething extension authorize</title>
<style>
  body { font: 15px/1.5 system-ui, sans-serif; background: #111; color: #eee; max-width: 44rem; margin: 3rem auto; padding: 0 1rem; }
  a { color: #8ab4f8; word-break: break-all; }
  input { width: 100%; box-sizing: border-box; padding: .5rem; margin: .5rem 0; background: #222; color: #eee; border: 1px solid #444; }
  button { padding: .4rem .9rem; margin-right: .5rem; }
  .note { color: #9f9; }
</style>
<h1>extension authorize</h1>
${note ? `<p class="note">${escapeHtml(note)}</p>` : ''}
${status}
`;
}
