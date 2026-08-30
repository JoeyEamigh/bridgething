// Generated from @bridgething/webapp-shared. It is yours now; tweak it freely.
import { createRequire } from 'node:module';
var __require = /* @__PURE__ */ createRequire(import.meta.url);

// ../webapp-shared/src/dev.ts
import { spawn as spawn2 } from 'node:child_process';
import { isIPv4 } from 'node:net';
import { networkInterfaces } from 'node:os';
import { relative } from 'node:path';

// ../webapp-shared/src/daemon.ts
var DAEMON_PROXY_PATH = '/__bridgething';

// ../webapp-shared/src/extension.ts
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { createRequire as createRequire2 } from 'node:module';
import { homedir, platform } from 'node:os';
import { delimiter, dirname, join as join2, resolve } from 'node:path';
import { createInterface } from 'node:readline';

// ../webapp-shared/src/gateway.ts
import { decode as msgpackDecode, encode as msgpackEncode } from '@msgpack/msgpack';
import { randomUUID } from 'node:crypto';
import { lookup } from 'node:dns/promises';
import { isIP } from 'node:net';
var DEFAULT_DEVICE_HOST = 'bridgething.local';
function deviceHostName(host) {
  return host ?? process.env.SUPERBIRD_HOST ?? DEFAULT_DEVICE_HOST;
}
async function resolveGatewayTarget(host) {
  const name = deviceHostName(host);
  const port = Number(process.env.BRIDGETHING_GATEWAY_PORT ?? 8892);
  return { name, host: await resolveHost(name), port };
}
async function resolveHost(name) {
  if (isIP(name)) return name;
  try {
    return (await lookup(name, { family: 4 })).address;
  } catch {
    return name;
  }
}
function parseUuid(s) {
  const hex = s.replace(/-/g, '').toLowerCase();
  if (hex.length !== 32 || !/^[0-9a-f]+$/.test(hex)) {
    throw new Error(`invalid uuid: ${s}`);
  }
  const out = new Uint8Array(16);
  for (let i = 0; i < 16; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}
function freshMsgId() {
  return parseUuid(randomUUID());
}
function uuidToString(bytes) {
  const hex = Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
var FRAME_HEADER_LENGTH = 16;
var FRAME_MAGIC = 57005;
var FRAME_VERSION = 2;
var COMPRESSION_NONE = 0;
var ENCODING_MSGPACK = 0;
var PRIORITY_NORMAL = 0;
function writeFrameHeader(payloadLength) {
  const buf = new Uint8Array(FRAME_HEADER_LENGTH);
  const view = new DataView(buf.buffer);
  view.setUint16(0, FRAME_MAGIC, false);
  view.setUint8(2, FRAME_VERSION);
  view.setUint8(3, COMPRESSION_NONE);
  view.setUint8(4, ENCODING_MSGPACK);
  view.setUint8(5, PRIORITY_NORMAL);
  view.setBigUint64(8, BigInt(payloadLength), false);
  return buf;
}
function frame(message) {
  const body = msgpackEncode(message);
  const header = writeFrameHeader(body.length);
  const out = new Uint8Array(header.length + body.length);
  out.set(header, 0);
  out.set(body, header.length);
  return out;
}

class FrameAccumulator {
  buffer = new Uint8Array(0);
  append(chunk) {
    if (chunk.length === 0) return;
    const merged = new Uint8Array(this.buffer.length + chunk.length);
    merged.set(this.buffer, 0);
    merged.set(chunk, this.buffer.length);
    this.buffer = merged;
  }
  next() {
    if (this.buffer.length < FRAME_HEADER_LENGTH) return null;
    const view = new DataView(this.buffer.buffer, this.buffer.byteOffset, this.buffer.byteLength);
    const magic = view.getUint16(0, false);
    if (magic !== FRAME_MAGIC) throw new Error(`bad framing magic 0x${magic.toString(16)}`);
    const version = view.getUint8(2);
    if (version !== FRAME_VERSION) throw new Error(`unsupported frame version ${version}`);
    const compression = view.getUint8(3);
    if (compression !== COMPRESSION_NONE) {
      throw new Error(`unsupported inbound compression ${compression} (this script only handles uncompressed)`);
    }
    const encoding = view.getUint8(4);
    if (encoding !== ENCODING_MSGPACK) throw new Error(`unsupported inbound encoding ${encoding}`);
    const len = Number(view.getBigUint64(8, false));
    const total = FRAME_HEADER_LENGTH + len;
    if (this.buffer.length < total) return null;
    const body = this.buffer.subarray(FRAME_HEADER_LENGTH, total);
    const decoded = msgpackDecode(body);
    this.buffer = this.buffer.slice(total);
    return decoded;
  }
}
var OPEN_TIMEOUT_MS = 15000;
var REQUEST_TIMEOUT_MS = 15000;

class GatewayLink {
  ws;
  url;
  acc = new FrameAccumulator();
  pending = new Map();
  handlers = new Set();
  closers = new Set();
  closed = null;
  constructor(ws, url) {
    this.ws = ws;
    this.url = url;
    ws.addEventListener('message', event => this.receive(event.data));
    ws.addEventListener('close', event => this.finish(`gateway closed (code ${event.code})`));
    ws.addEventListener('error', () => this.finish('gateway socket error'));
  }
  static open(target) {
    const url = `ws://${target.host}:${target.port}/`;
    return new Promise((res, rej) => {
      const ws = new WebSocket(url);
      ws.binaryType = 'arraybuffer';
      const timer = setTimeout(() => {
        ws.close();
        rej(new Error(`gateway connect timed out (${OPEN_TIMEOUT_MS / 1000}s) against ${url}`));
      }, OPEN_TIMEOUT_MS);
      ws.addEventListener('open', () => {
        clearTimeout(timer);
        res(new GatewayLink(ws, url));
      });
      ws.addEventListener('error', () => {
        clearTimeout(timer);
        rej(new Error(`could not reach the gateway at ${url}`));
      });
      ws.addEventListener('close', event => {
        clearTimeout(timer);
        rej(new Error(`gateway closed before opening (code ${event.code})`));
      });
    });
  }
  get isOpen() {
    return this.closed === null;
  }
  onMessage(handler) {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }
  onClose(handler) {
    this.closers.add(handler);
    return () => this.closers.delete(handler);
  }
  event(data) {
    this.write({ id: freshMsgId(), meta: { kind: 'event' }, data });
  }
  request(kind, data) {
    if (this.closed !== null) return Promise.reject(new Error(this.closed));
    const id = freshMsgId();
    const key = uuidToString(id);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(key);
        reject(new Error(`gateway ${kind} timed out (${REQUEST_TIMEOUT_MS / 1000}s) against ${this.url}`));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(key, { resolve, reject, timer });
      this.write({ id, meta: { kind }, data });
    });
  }
  close() {
    try {
      this.ws.close();
    } catch {}
    this.finish('gateway link closed');
  }
  write(message) {
    if (this.closed !== null) return;
    this.ws.send(frame(message));
  }
  receive(raw) {
    const bytes = raw instanceof ArrayBuffer ? new Uint8Array(raw) : raw instanceof Uint8Array ? raw : null;
    if (!bytes) return;
    try {
      this.acc.append(bytes);
      for (let msg = this.acc.next(); msg !== null; msg = this.acc.next()) this.dispatch(msg);
    } catch (err) {
      this.finish(err instanceof Error ? err.message : String(err));
      this.close();
    }
  }
  dispatch(msg) {
    if (msg.meta?.kind === 'response') {
      const waiting = this.pending.get(uuidToString(msg.meta.data.requestId));
      if (waiting) {
        this.pending.delete(uuidToString(msg.meta.data.requestId));
        clearTimeout(waiting.timer);
        waiting.resolve(msg.data);
        return;
      }
    }
    for (const handler of [...this.handlers]) handler(msg.data);
  }
  finish(reason) {
    if (this.closed !== null) return;
    this.closed = reason;
    for (const waiting of this.pending.values()) {
      clearTimeout(waiting.timer);
      waiting.reject(new Error(reason));
    }
    this.pending.clear();
    for (const closer of [...this.closers]) closer(reason);
  }
}
async function exchange(target, kind, data) {
  const link = await GatewayLink.open(target);
  try {
    return await link.request(kind, data);
  } finally {
    link.close();
  }
}
function interpret(data, expected) {
  const outer = data;
  if (outer?.type !== 'webapp') {
    return { ok: false, reason: `unexpected response type ${JSON.stringify(outer?.type)}` };
  }
  const inner = outer.data;
  if (inner?.event === expected) return { ok: true, value: inner.data };
  if (inner?.event === 'webappError') {
    const err = inner.data;
    return { ok: false, reason: `daemon refused: ${err?.type} ${JSON.stringify(err?.data ?? {})}` };
  }
  return { ok: false, reason: `unexpected webapp response variant ${JSON.stringify(inner?.event)}` };
}
async function switchTo(target, id) {
  const data = { type: 'webapp', data: { event: 'switchTo', data: { id: parseUuid(id) } } };
  return interpret(await exchange(target, 'request', data), 'switched');
}
async function listWebapps(target) {
  const data = { type: 'webapp', data: { event: 'list' } };
  return interpret(await exchange(target, 'request', data), 'webapps');
}
async function navigateKiosk(target, url) {
  await exchange(target, 'command', { type: 'chrome', data: { event: 'navigate', data: { url } } });
}
async function getActive(link) {
  return interpret(await link.request('request', { type: 'webapp', data: { event: 'getActive' } }), 'active');
}
async function listConfig(link, id) {
  const data = { type: 'webapp', data: { event: 'configList', data: { id: parseUuid(id) } } };
  return interpret(await link.request('request', data), 'configList');
}
async function getNickname(link) {
  const reply = await link.request('request', { type: 'system', data: { event: 'deviceGetNickname' } });
  if (reply?.type !== 'system' || reply.data?.event !== 'deviceNickname') return null;
  return reply.data.data?.nickname ?? null;
}

// ../webapp-shared/src/permissions.ts
import { join } from 'node:path';
var KINDS = ['net', 'read', 'write', 'run', 'env', 'sys', 'ffi'];
var PATHY = new Set(['read', 'write', 'run', 'ffi']);

class PermissionError extends Error {
  descriptor;
  constructor(descriptor, reason) {
    super(`invalid permission ${JSON.stringify(descriptor)}: ${reason}`);
    this.descriptor = descriptor;
    this.name = 'PermissionError';
  }
}
function parse(descriptor) {
  const cut = descriptor.indexOf(':');
  const kind = cut < 0 ? descriptor : descriptor.slice(0, cut);
  const scope = cut < 0 ? null : descriptor.slice(cut + 1);
  if (scope === '') throw new PermissionError(descriptor, 'scope is empty');
  if (scope?.includes(',')) throw new PermissionError(descriptor, 'scope contains a comma');
  if (kind === 'all') {
    if (scope !== null) throw new PermissionError(descriptor, '`all` takes no scope');
    return { kind: 'all' };
  }
  if (!KINDS.includes(kind)) throw new PermissionError(descriptor, 'unknown permission kind');
  return { kind, scope };
}
function expandHome(permission, home) {
  if (permission.kind === 'all' || permission.scope === null || !PATHY.has(permission.kind)) return permission;
  const { scope } = permission;
  if (scope === '~') return { ...permission, scope: home };
  if (scope.startsWith('~/')) return { ...permission, scope: join(home, scope.slice(2)) };
  return permission;
}
function denoFlags(descriptors, home) {
  const permissions = descriptors.map(parse).map(p => (home === undefined ? p : expandHome(p, home)));
  if (permissions.some(p => p.kind === 'all')) return ['--allow-all'];
  const flags = [];
  for (const kind of KINDS) {
    const ofKind = permissions.filter(p => p.kind === kind);
    if (ofKind.length === 0) continue;
    const scopes = [];
    let bare = false;
    for (const permission of ofKind) {
      if (permission.scope === null) bare = true;
      else if (!scopes.includes(permission.scope)) scopes.push(permission.scope);
    }
    flags.push(bare || scopes.length === 0 ? `--allow-${kind}` : `--allow-${kind}=${scopes.join(',')}`);
  }
  return flags;
}

// ../webapp-shared/src/extension.ts
var EXTENSION_SOURCE = 'extension/main.ts';
var EXTENSION_DATA_DIR = '.dev-extension';
var DENO_PACKAGE_VERSION = '2.9.6';
var STOP_GRACE_MS = 1500;
var TERM_GRACE_MS = 1500;
var CRASH_BACKOFF_BASE_MS = 1000;
var CRASH_BACKOFF_CEILING_MS = 60000;
var LINK_BACKOFF_BASE_MS = 1000;
var LINK_BACKOFF_CEILING_MS = 1e4;
function readManifest(publicDir) {
  const manifest = JSON.parse(readFileSync(join2(publicDir, 'manifest.json'), 'utf8'));
  if (!manifest.id) throw new Error('public/manifest.json has no id');
  return manifest;
}
function extensionBuildOptions(source, outfile) {
  return {
    entryPoints: [source],
    outfile,
    bundle: true,
    format: 'esm',
    platform: 'node',
    external: ['npm:*', 'jsr:*', 'node:*'],
    target: 'esnext',
    minify: false,
    sourcemap: false,
    logLevel: 'silent',
  };
}
async function buildExtension(root, outDir, extension) {
  const esbuild = await import('esbuild');
  const outfile = resolve(root, outDir, extension.entry);
  const result = await esbuild.build(extensionBuildOptions(resolve(root, EXTENSION_SOURCE), outfile));
  if (result.errors.length > 0) {
    const lines = await esbuild.formatMessages(result.errors, { kind: 'error', color: false });
    throw new Error(
      lines.join(`
`),
    );
  }
  return outfile;
}
async function resolveDeno(root) {
  const explicit = process.env.BRIDGETHING_DENO;
  if (explicit) return explicit;
  const exeName = platform() === 'win32' ? 'deno.exe' : 'deno';
  const packaged = denoPackageDir(root);
  if (packaged) {
    const exe = join2(packaged, exeName);
    if (!existsSync(exe)) await runToExit(process.execPath, [join2(packaged, 'bin.cjs'), '--version'], root);
    if (existsSync(exe)) return exe;
  }
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (dir && existsSync(join2(dir, exeName))) return join2(dir, exeName);
  }
  throw new Error(
    `deno is not installed; \`bun add -d deno@${DENO_PACKAGE_VERSION}\` puts the runtime the desktop app uses into node_modules`,
  );
}
function denoPackageDir(root) {
  try {
    return dirname(createRequire2(join2(root, 'package.json')).resolve('deno/package.json'));
  } catch {
    return null;
  }
}
function runToExit(cmd, args, cwd) {
  return new Promise((res, rej) => {
    const child = spawn(cmd, args, { cwd, stdio: 'ignore' });
    child.on('exit', code => (code === 0 ? res() : rej(new Error(`${cmd} ${args.join(' ')} exited ${code}`))));
    child.on('error', rej);
  });
}
async function openInBrowser(url) {
  const os = platform();
  const [cmd, args] =
    os === 'darwin' ? ['open', [url]] : os === 'win32' ? ['cmd', ['/c', 'start', '', url]] : ['xdg-open', [url]];
  await new Promise((res, rej) => {
    const child = spawn(cmd, args, { stdio: 'ignore', detached: true });
    child.on('error', rej);
    child.on('spawn', () => {
      child.unref();
      res();
    });
  });
}
function readKv(path, log) {
  if (!existsSync(path)) return new Map();
  try {
    return new Map(Object.entries(JSON.parse(readFileSync(path, 'utf8'))));
  } catch (err) {
    log.warn(`extension store ${path} did not parse (${String(err)}); it starts empty`);
    return new Map();
  }
}
function writeKv(path, held) {
  mkdirSync(dirname(path), { recursive: true });
  const staging = `${path}.part`;
  writeFileSync(staging, JSON.stringify(Object.fromEntries(held)));
  renameSync(staging, path);
}
function toWire(message) {
  if (message.encoding === 'binary') {
    return { encoding: 'binary', data: Buffer.from(message.data).toString('base64') };
  }
  return message;
}
function fromWire(message) {
  if (message.encoding === 'binary')
    return { encoding: 'binary', data: new Uint8Array(Buffer.from(message.data, 'base64')) };
  return message;
}
function sleep(ms) {
  return new Promise(res => setTimeout(res, ms));
}

class ExtensionDevHost {
  opts;
  dataDir;
  kvPath;
  outfile;
  kv;
  log;
  id;
  deno = '';
  context = null;
  child = null;
  generation = 0;
  crashes = 0;
  restartTimer = null;
  link = null;
  device = null;
  waiting = null;
  closing = false;
  linkLoop = Promise.resolve();
  constructor(opts) {
    this.opts = opts;
    this.log = opts.log;
    this.id = opts.manifest.id.toLowerCase();
    this.dataDir = resolve(opts.root, EXTENSION_DATA_DIR);
    this.kvPath = join2(this.dataDir, 'kv.json');
    this.outfile = join2(this.dataDir, 'build', 'desktop.mjs');
    this.kv = readKv(this.kvPath, this.log);
  }
  async start() {
    mkdirSync(this.dataDir, { recursive: true });
    this.deno = await resolveDeno(this.opts.root);
    const esbuild = await import('esbuild');
    this.context = await esbuild.context({
      ...extensionBuildOptions(resolve(this.opts.root, EXTENSION_SOURCE), this.outfile),
      plugins: [
        {
          name: 'bridgething:extension-host',
          setup: build => {
            build.onEnd(result => {
              this.built(result);
            });
          },
        },
      ],
    });
    await this.context.watch();
    this.linkLoop = this.runLink();
  }
  get running() {
    return this.child?.ready ?? false;
  }
  get pendingAuthorize() {
    return this.waiting ? { url: this.waiting.url } : null;
  }
  settleAuthorize(callback) {
    const waiting = this.waiting;
    if (!waiting) return false;
    this.waiting = null;
    this.reply(waiting.generation, { t: 'reply', id: waiting.id, ok: true, value: callback });
    return true;
  }
  cancelAuthorize() {
    const waiting = this.waiting;
    if (!waiting) return false;
    this.waiting = null;
    this.reply(waiting.generation, { t: 'reply', id: waiting.id, ok: false, error: 'cancelled' });
    return true;
  }
  async close() {
    if (this.closing) return;
    this.closing = true;
    if (this.restartTimer) clearTimeout(this.restartTimer);
    await this.context?.dispose();
    const child = this.child;
    if (child) {
      this.stopChild(child, 'stop');
      await child.exited;
    }
    this.publishRunning([]);
    this.link?.close();
    await this.linkLoop;
  }
  async built(result) {
    if (this.closing) return;
    if (result.errors.length > 0) {
      const esbuild = await import('esbuild');
      const lines = await esbuild.formatMessages(result.errors, { kind: 'error', color: false });
      this.log.error(`extension build failed; the previous build keeps running
${lines.join(`
`)}`);
      return;
    }
    if (this.restartTimer) {
      clearTimeout(this.restartTimer);
      this.restartTimer = null;
    }
    this.crashes = 0;
    if (this.child) {
      this.log.info('extension rebuilt; restarting it');
      this.stopChild(this.child, 'restart');
      return;
    }
    this.spawnChild();
  }
  spawnChild() {
    if (this.closing || this.child) return;
    const generation = ++this.generation;
    const args = [
      'run',
      '--no-prompt',
      ...denoFlags(this.opts.manifest.extension.permissions ?? [], homedir()),
      this.outfile,
    ];
    const process_ = spawn(this.deno, args, {
      cwd: this.dataDir,
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, DENO_NO_PACKAGE_JSON: '1', DENO_NO_UPDATE_CHECK: '1', NO_COLOR: '1' },
    });
    const exited = new Promise(res => {
      process_.on('exit', (code, signal) => {
        this.exited(generation, code, signal);
        res();
      });
    });
    process_.on('error', err => this.log.error(`extension did not start: ${err.message}`));
    const child = { process: process_, generation, ready: false, intent: 'run', exited };
    this.child = child;
    createInterface({ input: process_.stdout }).on('line', line => this.fromChild(child, line));
    createInterface({ input: process_.stderr }).on('line', line => {
      if (line.trim()) this.log.warn(`[${this.appName()} stderr] ${line}`);
    });
    this.log.info(`extension starting (deno ${args.slice(1, -1).join(' ')})`);
    this.write(child, {
      t: 'hello',
      api: this.opts.manifest.extension.api,
      webapp: {
        id: this.opts.manifest.id,
        name: this.appName(),
        version: this.opts.manifest.version ?? '0.0.0',
      },
      dataDir: this.dataDir,
    });
    if (this.device) this.write(child, this.connected(this.device));
  }
  stopChild(child, intent) {
    child.intent = intent;
    this.write(child, { t: 'stop' });
    child.process.stdin?.end();
    const process_ = child.process;
    setTimeout(() => {
      if (process_.exitCode === null && process_.signalCode === null) process_.kill('SIGTERM');
      setTimeout(() => {
        if (process_.exitCode === null && process_.signalCode === null) process_.kill('SIGKILL');
      }, TERM_GRACE_MS).unref();
    }, STOP_GRACE_MS).unref();
  }
  exited(generation, code, signal) {
    const child = this.child;
    if (!child || child.generation !== generation) return;
    this.child = null;
    if (this.waiting?.generation === generation) this.waiting = null;
    if (child.ready) this.publishRunning([]);
    if (this.closing || child.intent === 'stop') return;
    if (child.intent === 'restart') {
      this.spawnChild();
      return;
    }
    const delay = Math.min(CRASH_BACKOFF_BASE_MS * 2 ** this.crashes, CRASH_BACKOFF_CEILING_MS);
    this.crashes += 1;
    this.log.error(
      `extension exited (${signal ?? `code ${code}`}); restarting in ${Math.round(delay / 1000)}s, or on the next save`,
    );
    this.restartTimer = setTimeout(() => {
      this.restartTimer = null;
      this.spawnChild();
    }, delay);
  }
  write(child, message) {
    const stdin = child.process.stdin;
    if (!stdin || stdin.destroyed || stdin.writableEnded) return;
    stdin.write(`${JSON.stringify(message)}
`);
  }
  reply(generation, message) {
    if (this.child?.generation === generation) this.write(this.child, message);
  }
  fromChild(child, line) {
    if (!line.trim()) return;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      this.log.warn(`[${this.appName()} stdout] ${line}`);
      return;
    }
    switch (message.t) {
      case 'ready':
        child.ready = true;
        this.crashes = 0;
        this.log.info('extension is ready');
        this.publishRunning([this.id]);
        return;
      case 'log':
        this.tap(message.level, message.message);
        return;
      case 'device.send':
        this.sendToDevice(message.device, message.message);
        return;
      case 'kv.get':
        this.write(child, { t: 'reply', id: message.id, ok: true, value: this.kv.get(message.key) ?? null });
        return;
      case 'kv.set':
        this.kv.set(message.key, message.value);
        this.persist(child, message.id);
        return;
      case 'kv.delete':
        this.kv.delete(message.key);
        this.persist(child, message.id);
        return;
      case 'kv.list':
        this.write(child, { t: 'reply', id: message.id, ok: true, value: [...this.kv.keys()] });
        return;
      case 'auth.authorize':
        this.authorize(child, message.id, message.url);
        return;
    }
  }
  persist(child, id) {
    try {
      writeKv(this.kvPath, this.kv);
      this.write(child, { t: 'reply', id, ok: true, value: null });
    } catch (err) {
      this.write(child, { t: 'reply', id, ok: false, error: `write failed: ${String(err)}` });
    }
  }
  async authorize(child, id, url) {
    if (this.waiting) {
      this.write(child, { t: 'reply', id, ok: false, error: 'busy: an authorization is already in flight' });
      return;
    }
    if (!/^https?:\/\//i.test(url)) {
      this.write(child, { t: 'reply', id, ok: false, error: 'unsupported: only http(s) urls open in a browser' });
      return;
    }
    this.waiting = { id, url, generation: child.generation };
    try {
      await (this.opts.openUrl ?? openInBrowser)(url);
    } catch (err) {
      this.waiting = null;
      this.write(child, { t: 'reply', id, ok: false, error: `unsupported: ${String(err)}` });
      return;
    }
    const page = this.opts.authorizePage ? ` at ${this.opts.authorizePage}` : '';
    this.log.info(`extension asked to authorize ${url}; paste the callback url the browser lands on${page}`);
  }
  tap(level, text) {
    const line = `[${this.appName()}] ${text}`;
    if (level === 'error') this.log.error(line);
    else if (level === 'warn') this.log.warn(line);
    else this.log.info(line);
  }
  appName() {
    return this.opts.manifest.name ?? this.opts.manifest.id;
  }
  connected(device) {
    return {
      t: 'device.connected',
      device: device.id,
      name: device.name,
      config: device.config,
      active: device.active,
    };
  }
  sendToDevice(device, message) {
    const target = this.device;
    if (!target || !this.link?.isOpen) return;
    if (device !== undefined ? device !== target.id : !target.active) return;
    this.link.event({
      type: 'forward',
      data: { event: 'routed', data: { webapp: parseUuid(this.id), message: fromWire(message) } },
    });
  }
  publishRunning(webapps) {
    if (!this.link?.isOpen) return;
    this.link.event({
      type: 'forward',
      data: { event: 'extensionsRunning', data: { webapps: webapps.map(parseUuid) } },
    });
  }
  async runLink() {
    let failures = 0;
    while (!this.closing) {
      let link;
      try {
        link = await GatewayLink.open(this.opts.target);
      } catch (err) {
        if (failures === 0) {
          this.log.warn(
            `no car thing at ${this.opts.target.name}:${this.opts.target.port} (${err instanceof Error ? err.message : String(err)}); the extension runs and links up when one appears`,
          );
        }
        failures += 1;
        await sleep(Math.min(LINK_BACKOFF_BASE_MS * 2 ** Math.min(failures, 8), LINK_BACKOFF_CEILING_MS));
        continue;
      }
      failures = 0;
      this.link = link;
      const closed = new Promise(res => link.onClose(res));
      link.onMessage(data => this.fromDevice(data));
      await this.linked(link);
      const reason = await closed;
      if (this.link === link) this.link = null;
      if (this.device) {
        const gone = this.device;
        this.device = null;
        if (this.child) this.write(this.child, { t: 'device.disconnected', device: gone.id });
      }
      if (this.closing) break;
      this.log.warn(`car thing link dropped (${reason}); reconnecting`);
      await sleep(LINK_BACKOFF_BASE_MS);
    }
  }
  async linked(link) {
    const results = await Promise.allSettled([getActive(link), listConfig(link, this.id), getNickname(link)]);
    if (!link.isOpen || this.closing) return;
    const [active, config, nickname] = results;
    if (active.status === 'rejected' || config.status === 'rejected') {
      const failure =
        active.status === 'rejected' ? active.reason : config.status === 'rejected' ? config.reason : null;
      this.log.error(`could not read the car thing's state (${String(failure)}); dropping the link to retry`);
      link.close();
      return;
    }
    if (!active.value.ok) this.log.warn(`active webapp unknown: ${active.value.reason}`);
    if (!config.value.ok) this.log.warn(`config unreadable: ${config.value.reason}`);
    const activeId = active.value.ok && active.value.value.id ? uuidToString(active.value.value.id) : null;
    const entries = config.value.ok ? config.value.value.entries : [];
    const device = {
      id: this.opts.target.name,
      name: (nickname.status === 'fulfilled' && nickname.value) || 'car thing',
      config: Object.fromEntries(entries.map(entry => [entry.key, entry.value])),
      active: activeId === this.id,
    };
    this.device = device;
    this.log.info(
      `car thing linked (${device.name}); ${device.active ? 'this app is active, forwards flow both ways' : 'this app is not active, so forwards will not route until it is'}`,
    );
    if (this.child) this.write(this.child, this.connected(device));
    if (this.child?.ready) this.publishRunning([this.id]);
  }
  fromDevice(data) {
    const outer = data;
    const device = this.device;
    if (!device) return;
    if (outer?.type === 'webapp' && outer.data?.event === 'activeChanged') {
      const changed = outer.data.data;
      const active = !!changed.id && uuidToString(changed.id) === this.id;
      if (active === device.active) return;
      device.active = active;
      this.log.info(
        active ? 'this app is now active on the car thing' : 'this app is no longer active on the car thing',
      );
      if (this.child) this.write(this.child, { t: 'device.active', device: device.id, active });
      return;
    }
    if (outer?.type === 'webapp' && outer.data?.event === 'configChanged') {
      const changed = outer.data.data;
      if (uuidToString(changed.id) !== this.id) return;
      if (changed.value === null) delete device.config[changed.key];
      else device.config[changed.key] = changed.value;
      if (this.child) {
        this.write(this.child, { t: 'config.changed', device: device.id, key: changed.key, value: changed.value });
      }
      return;
    }
    if (outer?.type === 'forward' && outer.data?.event === 'routed') {
      const routed = outer.data.data;
      if (uuidToString(routed.webapp) !== this.id || !this.child) return;
      this.write(this.child, { t: 'device.message', device: device.id, message: toWire(routed.message) });
    }
  }
}

// ../webapp-shared/src/dev.ts
var DEVICE_MODE = 'device';
var AUTHORIZE_PATH = '/__extension/authorize';
var DAEMON_PORT = 8891;
var SWITCH_SETTLE_MS = 1000;
async function daemonProxyTarget() {
  const explicit = process.env.BRIDGETHING_DAEMON_URL;
  if (explicit) return explicit.replace(/\/$/, '');
  return `ws://${await resolveHost(deviceHostName())}:${DAEMON_PORT}`;
}
async function daemonProxy() {
  return {
    [DAEMON_PROXY_PATH]: {
      target: await daemonProxyTarget(),
      ws: true,
      changeOrigin: true,
      rewrite: path => path.slice(DAEMON_PROXY_PATH.length) || '/',
    },
  };
}
function bridgething() {
  let command = 'serve';
  let mode = '';
  let logger;
  let root = process.cwd();
  let publicDir = '';
  let outDir = 'dist';
  let device = null;
  let host = null;
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
      let manifest;
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
        (async () => {
          const target = await resolveGatewayTarget();
          try {
            if (mode === DEVICE_MODE) device = await attachDevice(server, root, manifest, target, log);
            else if (extension) await makeActive(root, manifest, target, log);
          } catch (err) {
            log.error(`device mode: ${err instanceof Error ? err.message : String(err)}`);
          }
          if (!extension) return;
          const address = http.address();
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
        await stopping?.close().catch(err => {
          log.warn(`extension: did not stop cleanly: ${err instanceof Error ? err.message : String(err)}`);
        });
        if (ending) {
          await ending.release().catch(err => {
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
async function attachDevice(server, root, manifest, target, log) {
  if (!isIPv4(target.host)) throw new Error(`could not resolve ${target.name}; is the car thing plugged in?`);
  const local = hostAddressToward(target.host);
  if (!local) throw new Error(`no interface on this machine shares a subnet with ${target.name} (${target.host})`);
  const address = server.httpServer?.address();
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
async function makeActive(root, manifest, target, log) {
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
async function installOnce(root, manifest, target, log) {
  if (await installed(target, manifest.id)) return;
  log.info(`${manifest.name ?? manifest.id} is not on the car thing yet; installing it once so the daemon knows it`);
  await run('bun', ['run', 'push', '--no-switch', target.name], root);
}
async function installed(target, id) {
  const listed = await listWebapps(target);
  if (!listed.ok) throw new Error(listed.reason);
  const wanted = id.toLowerCase();
  return listed.value.webapps.some(webapp => uuidToString(webapp.id) === wanted);
}
async function activate(target, id) {
  const switched = await switchTo(target, id);
  if (!switched.ok) throw new Error(switched.reason);
}
function hostAddressToward(ip) {
  const wanted = ipv4Bits(ip);
  if (wanted === null) return null;
  for (const entries of Object.values(networkInterfaces())) {
    for (const entry of entries ?? []) {
      if (entry.family !== 'IPv4' || entry.internal || !entry.cidr) continue;
      const prefix = Number(entry.cidr.split('/')[1]);
      const own = ipv4Bits(entry.address);
      if (own === null || !Number.isFinite(prefix)) continue;
      const mask = prefix === 0 ? 0 : (4294967295 << (32 - prefix)) >>> 0;
      if ((own & mask) >>> 0 === (wanted & mask) >>> 0) return entry.address;
    }
  }
  return null;
}
function ipv4Bits(ip) {
  const parts = ip.split('.').map(Number);
  if (parts.length !== 4 || parts.some(part => !Number.isInteger(part) || part < 0 || part > 255)) return null;
  return ((parts[0] << 24) | (parts[1] << 16) | (parts[2] << 8) | parts[3]) >>> 0;
}
function run(cmd, args, cwd) {
  return new Promise((res, rej) => {
    const child = spawn2(cmd, args, { stdio: 'inherit', cwd });
    child.on('exit', code => (code === 0 ? res() : rej(new Error(`${cmd} ${args.join(' ')} exited ${code}`))));
    child.on('error', rej);
  });
}
function authorizePage(host) {
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
    req.on('data', chunk => {
      body += chunk;
    });
    req.on('end', () => {
      const form = new URLSearchParams(body);
      const current = host();
      let note;
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
function normalizeCallback(raw) {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  try {
    const url = new URL(trimmed);
    return `bridgething://oauth/callback${url.search}${url.hash}`;
  } catch {
    return null;
  }
}
function escapeHtml(text) {
  return text.replace(/[&<>"']/g, ch => `&#${ch.charCodeAt(0)};`);
}
function renderAuthorize(pending, note) {
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
export { AUTHORIZE_PATH, bridgething, buildExtension, daemonProxy, daemonProxyTarget, DEVICE_MODE };
