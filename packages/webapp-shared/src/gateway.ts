import { decode as msgpackDecode, encode as msgpackEncode } from '@msgpack/msgpack';
import { randomUUID } from 'node:crypto';
import { lookup } from 'node:dns/promises';
import { isIP } from 'node:net';

export type GatewayTarget = { name: string; host: string; port: number };

export const DEFAULT_DEVICE_HOST = 'bridgething.local';

export function deviceHostName(host?: string): string {
  return host ?? process.env.SUPERBIRD_HOST ?? DEFAULT_DEVICE_HOST;
}

export async function resolveGatewayTarget(host?: string): Promise<GatewayTarget> {
  const name = deviceHostName(host);
  const port = Number(process.env.BRIDGETHING_GATEWAY_PORT ?? 8892);
  return { name, host: await resolveHost(name), port };
}

export async function resolveHost(name: string): Promise<string> {
  if (isIP(name)) return name;
  try {
    return (await lookup(name, { family: 4 })).address;
  } catch {
    return name;
  }
}

export function parseUuid(s: string): Uint8Array {
  const hex = s.replace(/-/g, '').toLowerCase();
  if (hex.length !== 32 || !/^[0-9a-f]+$/.test(hex)) {
    throw new Error(`invalid uuid: ${s}`);
  }
  const out = new Uint8Array(16);
  for (let i = 0; i < 16; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

export function freshMsgId(): Uint8Array {
  return parseUuid(randomUUID());
}

export function uuidToString(bytes: Uint8Array): string {
  const hex = Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function bundleDirName(id: string): string {
  const hex = id.replace(/-/g, '').toLowerCase();
  if (!/^[0-9a-f]{32}$/.test(hex)) throw new Error(`manifest id is not a uuid: ${id}`);
  return hex;
}

const FRAME_HEADER_LENGTH = 16;
const FRAME_MAGIC = 0xdead;
const FRAME_VERSION = 2;
const COMPRESSION_NONE = 0x00;
const ENCODING_MSGPACK = 0x00;
const PRIORITY_NORMAL = 0x00;

function writeFrameHeader(payloadLength: number): Uint8Array<ArrayBuffer> {
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

export function frame(message: unknown): Uint8Array<ArrayBuffer> {
  const body = msgpackEncode(message);
  const header = writeFrameHeader(body.length);
  const out = new Uint8Array(header.length + body.length);
  out.set(header, 0);
  out.set(body, header.length);
  return out;
}

export type GatewayMsg = { id: Uint8Array; meta: GatewayMeta; data: unknown };

export type GatewayMeta =
  | { kind: 'event' }
  | { kind: 'request' }
  | { kind: 'command' }
  | { kind: 'response'; data: { requestId: Uint8Array } };

export class FrameAccumulator {
  private buffer = new Uint8Array(0);

  append(chunk: Uint8Array): void {
    if (chunk.length === 0) return;
    const merged = new Uint8Array(this.buffer.length + chunk.length);
    merged.set(this.buffer, 0);
    merged.set(chunk, this.buffer.length);
    this.buffer = merged;
  }

  next(): GatewayMsg | null {
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
    const decoded = msgpackDecode(body) as GatewayMsg;
    this.buffer = this.buffer.slice(total);
    return decoded;
  }
}

export function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i]! ^ b[i]!;
  return diff === 0;
}

export type Kind = 'request' | 'command';

const OPEN_TIMEOUT_MS = 15_000;
const REQUEST_TIMEOUT_MS = 15_000;

type Pending = { resolve: (data: unknown) => void; reject: (err: Error) => void; timer: ReturnType<typeof setTimeout> };

export class GatewayLink {
  private readonly acc = new FrameAccumulator();
  private readonly pending = new Map<string, Pending>();
  private readonly handlers = new Set<(data: unknown) => void>();
  private readonly closers = new Set<(reason: string) => void>();
  private closed: string | null = null;

  private constructor(
    private readonly ws: WebSocket,
    readonly url: string,
  ) {
    ws.addEventListener('message', (event: MessageEvent) => this.receive(event.data as unknown));
    ws.addEventListener('close', (event: CloseEvent) => this.finish(`gateway closed (code ${event.code})`));
    ws.addEventListener('error', () => this.finish('gateway socket error'));
  }

  static open(target: GatewayTarget): Promise<GatewayLink> {
    const url = `ws://${target.host}:${target.port}/`;
    return new Promise<GatewayLink>((res, rej) => {
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
      ws.addEventListener('close', (event: CloseEvent) => {
        clearTimeout(timer);
        rej(new Error(`gateway closed before opening (code ${event.code})`));
      });
    });
  }

  get isOpen(): boolean {
    return this.closed === null;
  }

  onMessage(handler: (data: unknown) => void): () => void {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  onClose(handler: (reason: string) => void): () => void {
    this.closers.add(handler);
    return () => this.closers.delete(handler);
  }

  event(data: unknown): void {
    this.write({ id: freshMsgId(), meta: { kind: 'event' }, data });
  }

  request(kind: Kind, data: unknown): Promise<unknown> {
    if (this.closed !== null) return Promise.reject(new Error(this.closed));
    const id = freshMsgId();
    const key = uuidToString(id);
    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(key);
        reject(new Error(`gateway ${kind} timed out (${REQUEST_TIMEOUT_MS / 1000}s) against ${this.url}`));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(key, { resolve, reject, timer });
      this.write({ id, meta: { kind }, data });
    });
  }

  close(): void {
    try {
      this.ws.close();
    } catch {}
    this.finish('gateway link closed');
  }

  private write(message: unknown): void {
    if (this.closed !== null) return;
    this.ws.send(frame(message));
  }

  private receive(raw: unknown): void {
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

  private dispatch(msg: GatewayMsg): void {
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

  private finish(reason: string): void {
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

async function exchange(target: GatewayTarget, kind: Kind, data: unknown): Promise<unknown> {
  const link = await GatewayLink.open(target);
  try {
    return await link.request(kind, data);
  } finally {
    link.close();
  }
}

export type Outcome<T> = { ok: true; value: T } | { ok: false; reason: string };

function interpret<T>(data: unknown, expected: string): Outcome<T> {
  const outer = data as { type?: string; data?: unknown };
  if (outer?.type !== 'webapp') {
    return { ok: false, reason: `unexpected response type ${JSON.stringify(outer?.type)}` };
  }
  const inner = outer.data as { event?: string; data?: unknown };
  if (inner?.event === expected) return { ok: true, value: inner.data as T };
  if (inner?.event === 'webappError') {
    const err = inner.data as { type?: string; data?: Record<string, unknown> } | undefined;
    return { ok: false, reason: `daemon refused: ${err?.type} ${JSON.stringify(err?.data ?? {})}` };
  }
  return { ok: false, reason: `unexpected webapp response variant ${JSON.stringify(inner?.event)}` };
}

export type ActiveWebapp = { id?: Uint8Array | null; name?: string | null };
export type Slots = { launcher?: Uint8Array | null; overlay?: Uint8Array | null };
export type Slot = 'launcher' | 'overlay';
export type ListedWebapp = { id: Uint8Array; name: string; version: string };
export type ConfigEntry = { key: string; value: string };

export async function switchTo(target: GatewayTarget, id: string): Promise<Outcome<ActiveWebapp>> {
  const data = { type: 'webapp', data: { event: 'switchTo', data: { id: parseUuid(id) } } };
  return interpret<ActiveWebapp>(await exchange(target, 'request', data), 'switched');
}

export async function setSlot(target: GatewayTarget, slot: Slot, id: string | null): Promise<Outcome<Slots>> {
  const data = {
    type: 'webapp',
    data: { event: 'setSlot', data: { slot, id: id === null ? null : parseUuid(id) } },
  };
  return interpret<Slots>(await exchange(target, 'request', data), 'slots');
}

export async function listWebapps(target: GatewayTarget): Promise<Outcome<{ webapps: ListedWebapp[] }>> {
  const data = { type: 'webapp', data: { event: 'list' } };
  return interpret<{ webapps: ListedWebapp[] }>(await exchange(target, 'request', data), 'webapps');
}

export async function navigateKiosk(target: GatewayTarget, url: string): Promise<void> {
  await exchange(target, 'command', { type: 'chrome', data: { event: 'navigate', data: { url } } });
}

export async function getActive(link: GatewayLink): Promise<Outcome<ActiveWebapp>> {
  return interpret<ActiveWebapp>(
    await link.request('request', { type: 'webapp', data: { event: 'getActive' } }),
    'active',
  );
}

export async function listConfig(link: GatewayLink, id: string): Promise<Outcome<{ entries: ConfigEntry[] }>> {
  const data = { type: 'webapp', data: { event: 'configList', data: { id: parseUuid(id) } } };
  return interpret<{ entries: ConfigEntry[] }>(await link.request('request', data), 'configList');
}

export async function getNickname(link: GatewayLink): Promise<string | null> {
  const reply = (await link.request('request', { type: 'system', data: { event: 'deviceGetNickname' } })) as {
    type?: string;
    data?: { event?: string; data?: { nickname?: string | null } };
  };
  if (reply?.type !== 'system' || reply.data?.event !== 'deviceNickname') return null;
  return reply.data.data?.nickname ?? null;
}
