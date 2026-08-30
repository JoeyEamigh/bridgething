import type { ExtensionMessage, HostMessage, WireForwardMessage } from '@bridgething/extension';
import type { BuildContext, BuildOptions, BuildResult } from 'esbuild';
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { homedir, platform } from 'node:os';
import { delimiter, dirname, join, resolve } from 'node:path';
import { createInterface } from 'node:readline';
import {
  GatewayLink,
  getActive,
  getNickname,
  listConfig,
  parseUuid,
  uuidToString,
  type GatewayTarget,
} from './gateway.js';
import { denoFlags } from './permissions.js';

export const EXTENSION_SOURCE = 'extension/main.ts';
export const EXTENSION_DATA_DIR = '.dev-extension';
export const DENO_PACKAGE_VERSION = '2.9.6';

const STOP_GRACE_MS = 1_500;
const TERM_GRACE_MS = 1_500;
const CRASH_BACKOFF_BASE_MS = 1_000;
const CRASH_BACKOFF_CEILING_MS = 60_000;
const LINK_BACKOFF_BASE_MS = 1_000;
const LINK_BACKOFF_CEILING_MS = 10_000;

export type ExtensionHostLog = {
  info(message: string): void;
  warn(message: string): void;
  error(message: string): void;
};

export type ExtensionManifest = { entry: string; permissions?: string[]; api: number };

export type WebappManifest = {
  id: string;
  name?: string;
  version?: string;
  extension?: ExtensionManifest;
};

export function readManifest(publicDir: string): WebappManifest {
  const manifest = JSON.parse(readFileSync(join(publicDir, 'manifest.json'), 'utf8')) as WebappManifest;
  if (!manifest.id) throw new Error('public/manifest.json has no id');
  return manifest;
}

export function extensionBuildOptions(source: string, outfile: string): BuildOptions {
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

export async function buildExtension(root: string, outDir: string, extension: ExtensionManifest): Promise<string> {
  const esbuild = await import('esbuild');
  const outfile = resolve(root, outDir, extension.entry);
  const result = await esbuild.build(extensionBuildOptions(resolve(root, EXTENSION_SOURCE), outfile));
  if (result.errors.length > 0) {
    const lines = await esbuild.formatMessages(result.errors, { kind: 'error', color: false });
    throw new Error(lines.join('\n'));
  }
  return outfile;
}

export async function resolveDeno(root: string): Promise<string> {
  const explicit = process.env.BRIDGETHING_DENO;
  if (explicit) return explicit;
  const exeName = platform() === 'win32' ? 'deno.exe' : 'deno';
  const packaged = denoPackageDir(root);
  if (packaged) {
    const exe = join(packaged, exeName);
    if (!existsSync(exe)) await runToExit(process.execPath, [join(packaged, 'bin.cjs'), '--version'], root);
    if (existsSync(exe)) return exe;
  }
  for (const dir of (process.env.PATH ?? '').split(delimiter)) {
    if (dir && existsSync(join(dir, exeName))) return join(dir, exeName);
  }
  throw new Error(
    `deno is not installed; \`bun add -d deno@${DENO_PACKAGE_VERSION}\` puts the runtime the desktop app uses into node_modules`,
  );
}

function denoPackageDir(root: string): string | null {
  try {
    return dirname(createRequire(join(root, 'package.json')).resolve('deno/package.json'));
  } catch {
    return null;
  }
}

function runToExit(cmd: string, args: string[], cwd: string): Promise<void> {
  return new Promise<void>((res, rej) => {
    const child = spawn(cmd, args, { cwd, stdio: 'ignore' });
    child.on('exit', code => (code === 0 ? res() : rej(new Error(`${cmd} ${args.join(' ')} exited ${code}`))));
    child.on('error', rej);
  });
}

export async function openInBrowser(url: string): Promise<void> {
  const os = platform();
  const [cmd, args] =
    os === 'darwin' ? ['open', [url]] : os === 'win32' ? ['cmd', ['/c', 'start', '', url]] : ['xdg-open', [url]];
  await new Promise<void>((res, rej) => {
    const child = spawn(cmd, args, { stdio: 'ignore', detached: true });
    child.on('error', rej);
    child.on('spawn', () => {
      child.unref();
      res();
    });
  });
}

export type ExtensionDevHostOptions = {
  root: string;
  manifest: WebappManifest & { extension: ExtensionManifest };
  target: GatewayTarget;
  log: ExtensionHostLog;
  authorizePage?: string;
  openUrl?: (url: string) => Promise<void>;
};

export type PendingAuthorize = { url: string };

type Intent = 'run' | 'restart' | 'stop';

type Child = {
  process: ChildProcess;
  generation: number;
  ready: boolean;
  intent: Intent;
  exited: Promise<void>;
};

type Device = {
  id: string;
  name: string;
  config: Record<string, string>;
  active: boolean;
};

type Waiting = { id: string; url: string; generation: number };

function readKv(path: string, log: ExtensionHostLog): Map<string, unknown> {
  if (!existsSync(path)) return new Map();
  try {
    return new Map(Object.entries(JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>));
  } catch (err) {
    log.warn(`extension store ${path} did not parse (${String(err)}); it starts empty`);
    return new Map();
  }
}

function writeKv(path: string, held: Map<string, unknown>): void {
  mkdirSync(dirname(path), { recursive: true });
  const staging = `${path}.part`;
  writeFileSync(staging, JSON.stringify(Object.fromEntries(held)));
  renameSync(staging, path);
}

function toWire(message: { encoding: string; data: unknown }): WireForwardMessage {
  if (message.encoding === 'binary') {
    return { encoding: 'binary', data: Buffer.from(message.data as Uint8Array).toString('base64') };
  }
  return message as WireForwardMessage;
}

function fromWire(message: WireForwardMessage): { encoding: string; data: unknown } {
  if (message.encoding === 'binary')
    return { encoding: 'binary', data: new Uint8Array(Buffer.from(message.data, 'base64')) };
  return message;
}

function sleep(ms: number): Promise<void> {
  return new Promise(res => setTimeout(res, ms));
}

export class ExtensionDevHost {
  readonly dataDir: string;
  private readonly kvPath: string;
  private readonly outfile: string;
  private readonly kv: Map<string, unknown>;
  private readonly log: ExtensionHostLog;
  private readonly id: string;
  private deno = '';
  private context: BuildContext | null = null;
  private child: Child | null = null;
  private generation = 0;
  private crashes = 0;
  private restartTimer: ReturnType<typeof setTimeout> | null = null;
  private link: GatewayLink | null = null;
  private device: Device | null = null;
  private waiting: Waiting | null = null;
  private closing = false;
  private linkLoop: Promise<void> = Promise.resolve();

  constructor(private readonly opts: ExtensionDevHostOptions) {
    this.log = opts.log;
    this.id = opts.manifest.id.toLowerCase();
    this.dataDir = resolve(opts.root, EXTENSION_DATA_DIR);
    this.kvPath = join(this.dataDir, 'kv.json');
    this.outfile = join(this.dataDir, 'build', 'desktop.mjs');
    this.kv = readKv(this.kvPath, this.log);
  }

  async start(): Promise<void> {
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
              void this.built(result);
            });
          },
        },
      ],
    });
    await this.context.watch();
    this.linkLoop = this.runLink();
  }

  get running(): boolean {
    return this.child?.ready ?? false;
  }

  get pendingAuthorize(): PendingAuthorize | null {
    return this.waiting ? { url: this.waiting.url } : null;
  }

  settleAuthorize(callback: string): boolean {
    const waiting = this.waiting;
    if (!waiting) return false;
    this.waiting = null;
    this.reply(waiting.generation, { t: 'reply', id: waiting.id, ok: true, value: callback });
    return true;
  }

  cancelAuthorize(): boolean {
    const waiting = this.waiting;
    if (!waiting) return false;
    this.waiting = null;
    this.reply(waiting.generation, { t: 'reply', id: waiting.id, ok: false, error: 'cancelled' });
    return true;
  }

  async close(): Promise<void> {
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

  private async built(result: BuildResult): Promise<void> {
    if (this.closing) return;
    if (result.errors.length > 0) {
      const esbuild = await import('esbuild');
      const lines = await esbuild.formatMessages(result.errors, { kind: 'error', color: false });
      this.log.error(`extension build failed; the previous build keeps running\n${lines.join('\n')}`);
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

  private spawnChild(): void {
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
    const exited = new Promise<void>(res => {
      process_.on('exit', (code, signal) => {
        this.exited(generation, code, signal);
        res();
      });
    });
    process_.on('error', err => this.log.error(`extension did not start: ${err.message}`));
    const child: Child = { process: process_, generation, ready: false, intent: 'run', exited };
    this.child = child;
    createInterface({ input: process_.stdout! }).on('line', line => this.fromChild(child, line));
    createInterface({ input: process_.stderr! }).on('line', line => {
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

  private stopChild(child: Child, intent: Intent): void {
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

  private exited(generation: number, code: number | null, signal: NodeJS.Signals | null): void {
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

  private write(child: Child, message: HostMessage): void {
    const stdin = child.process.stdin;
    if (!stdin || stdin.destroyed || stdin.writableEnded) return;
    stdin.write(`${JSON.stringify(message)}\n`);
  }

  private reply(generation: number, message: HostMessage): void {
    if (this.child?.generation === generation) this.write(this.child, message);
  }

  private fromChild(child: Child, line: string): void {
    if (!line.trim()) return;
    let message: ExtensionMessage;
    try {
      message = JSON.parse(line) as ExtensionMessage;
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
        void this.authorize(child, message.id, message.url);
        return;
    }
  }

  private persist(child: Child, id: string): void {
    try {
      writeKv(this.kvPath, this.kv);
      this.write(child, { t: 'reply', id, ok: true, value: null });
    } catch (err) {
      this.write(child, { t: 'reply', id, ok: false, error: `write failed: ${String(err)}` });
    }
  }

  private async authorize(child: Child, id: string, url: string): Promise<void> {
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

  private tap(level: 'debug' | 'info' | 'warn' | 'error', text: string): void {
    const line = `[${this.appName()}] ${text}`;
    if (level === 'error') this.log.error(line);
    else if (level === 'warn') this.log.warn(line);
    else this.log.info(line);
  }

  private appName(): string {
    return this.opts.manifest.name ?? this.opts.manifest.id;
  }

  private connected(device: Device): HostMessage {
    return {
      t: 'device.connected',
      device: device.id,
      name: device.name,
      config: device.config,
      active: device.active,
    };
  }

  private sendToDevice(device: string | undefined, message: WireForwardMessage): void {
    const target = this.device;
    if (!target || !this.link?.isOpen) return;
    if (device !== undefined ? device !== target.id : !target.active) return;
    this.link.event({
      type: 'forward',
      data: { event: 'routed', data: { webapp: parseUuid(this.id), message: fromWire(message) } },
    });
  }

  private publishRunning(webapps: string[]): void {
    if (!this.link?.isOpen) return;
    this.link.event({
      type: 'forward',
      data: { event: 'extensionsRunning', data: { webapps: webapps.map(parseUuid) } },
    });
  }

  private async runLink(): Promise<void> {
    let failures = 0;
    while (!this.closing) {
      let link: GatewayLink;
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
      const closed = new Promise<string>(res => link.onClose(res));
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

  private async linked(link: GatewayLink): Promise<void> {
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
    const device: Device = {
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

  private fromDevice(data: unknown): void {
    const outer = data as { type?: string; data?: { event?: string; data?: unknown } };
    const device = this.device;
    if (!device) return;
    if (outer?.type === 'webapp' && outer.data?.event === 'activeChanged') {
      const changed = outer.data.data as { id?: Uint8Array | null };
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
      const changed = outer.data.data as { id: Uint8Array; key: string; value: string | null };
      if (uuidToString(changed.id) !== this.id) return;
      if (changed.value === null) delete device.config[changed.key];
      else device.config[changed.key] = changed.value;
      if (this.child) {
        this.write(this.child, { t: 'config.changed', device: device.id, key: changed.key, value: changed.value });
      }
      return;
    }
    if (outer?.type === 'forward' && outer.data?.event === 'routed') {
      const routed = outer.data.data as { webapp: Uint8Array; message: { encoding: string; data: unknown } };
      if (uuidToString(routed.webapp) !== this.id || !this.child) return;
      this.write(this.child, { t: 'device.message', device: device.id, message: toWire(routed.message) });
    }
  }
}
