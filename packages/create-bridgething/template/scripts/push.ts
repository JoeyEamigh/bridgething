#!/usr/bin/env bun
// Generated from @bridgething/webapp-shared.
// ../webapp-shared/src/push.ts
import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';

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
function bundleDirName(id) {
  const hex = id.replace(/-/g, '').toLowerCase();
  if (!/^[0-9a-f]{32}$/.test(hex)) throw new Error(`manifest id is not a uuid: ${id}`);
  return hex;
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
async function setSlot(target, slot, id) {
  const data = {
    type: 'webapp',
    data: { event: 'setSlot', data: { slot, id: id === null ? null : parseUuid(id) } },
  };
  return interpret(await exchange(target, 'request', data), 'slots');
}

// ../webapp-shared/src/push.ts
var WEBAPP_ROOT = '/var/bridgething/webapps';
var SSH_OPTS = ['-o', 'UserKnownHostsFile=/dev/null', '-o', 'StrictHostKeyChecking=no', '-o', 'LogLevel=ERROR'];
function run(cmd, args, label, cwd) {
  return new Promise((res, rej) => {
    const child = spawn(cmd, args, { stdio: 'inherit', cwd });
    child.on('exit', code => (code === 0 ? res() : rej(new Error(`${label} exited ${code}`))));
    child.on('error', rej);
  });
}
function pipeThrough(producer, consumer, label) {
  return new Promise((res, rej) => {
    const source = spawn(producer.cmd, producer.args, { stdio: ['ignore', 'pipe', 'inherit'] });
    const sink = spawn(consumer.cmd, consumer.args, { stdio: ['pipe', 'inherit', 'inherit'] });
    source.stdout.pipe(sink.stdin);
    source.on('error', rej);
    sink.on('error', rej);
    source.on('exit', code => {
      if (code !== 0) rej(new Error(`${producer.cmd} exited ${code}`));
    });
    sink.on('exit', code => (code === 0 ? res() : rej(new Error(`${label} exited ${code}`))));
  });
}
async function pushBundle(localDir, host, dirName) {
  const staged = `.push.${dirName}`;
  console.log(`copying ${localDir} -> root@${host}:${WEBAPP_ROOT}/${dirName}/`);
  const receive = [
    'set -e',
    `cd ${WEBAPP_ROOT}`,
    `rm -rf ${staged} .old.${dirName}`,
    `mkdir -p ${staged}`,
    `tar -xzf - -C ${staged}`,
    `if [ -d ${dirName} ]; then mv ${dirName} .old.${dirName}; fi`,
    `mv ${staged} ${dirName}`,
    `rm -rf .old.${dirName}`,
  ].join('; ');
  await pipeThrough(
    { cmd: 'tar', args: ['-czf', '-', '-C', localDir, '.'] },
    { cmd: 'ssh', args: [...SSH_OPTS, `root@${host}`, receive] },
    'ssh',
  );
}
function buildBundle(repoDir) {
  console.log('bun run build');
  return run('bun', ['run', 'build'], 'bun run build', repoDir);
}
function declaredSlots(manifest) {
  const slots = [];
  if (manifest.role === 'launcher') slots.push('launcher');
  if (manifest.overlay) slots.push('overlay');
  return slots;
}
function parseArgs(argv) {
  const args = {
    host: process.env.SUPERBIRD_HOST ?? 'bridgething.local',
    skipBuild: process.env.SKIP_BUILD === '1',
    claimSlots: true,
    release: false,
    switchAfter: null,
  };
  for (const arg of argv) {
    if (arg === '--skip-build') args.skipBuild = true;
    else if (arg === '--no-switch') args.switchAfter = false;
    else if (arg === '--switch') args.switchAfter = true;
    else if (arg === '--no-slot') args.claimSlots = false;
    else if (arg === '--release') args.release = true;
    else if (arg === '--help' || arg === '-h') {
      printHelp();
      process.exit(0);
    } else if (arg.startsWith('--')) throw new Error(`unknown flag: ${arg}`);
    else args.host = arg;
  }
  return args;
}
function printHelp() {
  console.log(`Usage: bun run push [host] [options]

Build, copy dist/ onto a connected Car Thing, and make it visible.

A plain webapp becomes the active app. A launcher also takes the home-screen
slot; an overlay takes the overlay slot and the daemon reloads the kiosk so it
is injected into whatever app is showing. Overlay-only bundles are not switched
to by default, since switching away is the opposite of what you want to test.

Options:
  --release      hand this bundle's slots back to the built-in ones and stop.
                 the recovery path when a build wedges the screen.
  --no-slot      push without claiming any slot.
  --switch       switch to this bundle even if it is overlay-only.
  --no-switch    push without switching.
  --skip-build   copy whatever is already in dist/.

Env: SUPERBIRD_HOST, BRIDGETHING_GATEWAY_PORT, SKIP_BUILD=1
`);
}
async function bridgethingPush({ scriptUrl }) {
  const args = parseArgs(process.argv.slice(2));
  const target = await resolveGatewayTarget(args.host);
  const repoDir = resolve(dirname(new URL(scriptUrl).pathname), '..');
  const distDir = resolve(repoDir, 'dist');
  const manifestPath = resolve(distDir, 'manifest.json');
  const readManifest = () => {
    if (!existsSync(manifestPath)) {
      throw new Error(`no manifest.json at ${manifestPath}; run 'bun run build' first or drop --skip-build`);
    }
    const parsed = JSON.parse(readFileSync(manifestPath, 'utf8'));
    if (!parsed.id) throw new Error(`${manifestPath} has no 'id' field`);
    return parsed;
  };
  if (args.release) {
    const manifest2 = readManifest();
    for (const slot of declaredSlots(manifest2)) {
      const result = await setSlot(target, slot, null);
      if (!result.ok) throw new Error(result.reason);
      console.log(`${slot} slot: reverted to the built-in one`);
    }
    if (declaredSlots(manifest2).length === 0) console.log('this bundle declares no slots; nothing to release');
    return;
  }
  if (!args.skipBuild) await buildBundle(repoDir);
  const manifest = readManifest();
  const id = manifest.id;
  const slots = declaredSlots(manifest);
  await pushBundle(distDir, args.host, bundleDirName(id));
  if (args.claimSlots) {
    for (const slot of slots) {
      const result = await setSlot(target, slot, id);
      if (!result.ok) throw new Error(result.reason);
      console.log(`${slot} slot: ${manifest.name ?? id}`);
    }
  }
  const overlayOnly = slots.length > 0 && slots.every(s => s === 'overlay');
  const shouldSwitch = args.switchAfter ?? !overlayOnly;
  if (!shouldSwitch) {
    console.log(overlayOnly ? 'overlay pushed; the kiosk reloaded with it injected' : 'skipping switch');
    return;
  }
  const switched = await switchTo(target, id);
  if (!switched.ok) throw new Error(switched.reason);
  const active = switched.value;
  if (!active) {
    console.log('switched (the daemon dropped the push connection reloading the kiosk)');
    return;
  }
  const activeStr = active.id ? uuidToString(active.id) : '(none)';
  console.log(`active webapp: ${active.name ?? '(unnamed)'} ${activeStr}`);
}
export { bridgethingPush };

bridgethingPush({ scriptUrl: import.meta.url }).catch(err => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
