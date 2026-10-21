import { decode as msgpackDecode, encode as msgpackEncode } from '@msgpack/msgpack';

import { Logger, LogVerbosity, walkUuidFields } from '@bridgething/lib';
import type { BridgeToClientMsg, ClientToBridgeMsg, ClientToBridgeMsgData } from '@bridgething/lib/client';
import { newUuid } from '@bridgething/lib/uuid';

import type { ClientSurfaces } from './dispatch.generated.js';

export * from '@bridgething/lib';
export * from '@bridgething/lib/client';
export { deviceName, type NamedDevice } from './device-name.js';
export type { ClientSurfaces } from './dispatch.generated.js';

const DEFAULT_URL = 'ws://127.0.0.1:8891/';
const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;
const RECONNECT_BACKOFF_MS = [500, 1_000, 2_000, 4_000, 8_000, 15_000];

export type ConnectionState = 'connecting' | 'open' | 'closing' | 'closed';

export type ClientEvent =
  | { type: 'open' }
  | { type: 'close'; code?: number; reason?: string }
  | { type: 'connecting'; attempt: number }
  | { type: 'message'; message: BridgeToClientMsg }
  | { type: 'decodeError'; description: string };

export type ClientListener = (event: ClientEvent) => void;

export class ClientError extends Error {
  constructor(
    message: string,
    public readonly kind:
      | 'not-running'
      | 'already-running'
      | 'request-timed-out'
      | 'shutdown'
      | 'send-failed'
      | 'not-connected',
    err?: unknown,
  ) {
    super(message);
    this.name = 'ClientError';
    if (err instanceof Error && err.stack) this.stack = `${this.name}: ${this.message}\nCaused by: ${err.stack}`;
  }
}

type PendingRequest = {
  resolve: (msg: BridgeToClientMsg) => void;
  reject: (err: Error) => void;
  timeout: ReturnType<typeof setTimeout>;
};

type QueuedSend = {
  frame: Uint8Array<ArrayBuffer>;
  resolve: () => void;
  reject: (err: Error) => void;
};

export type ClientOptions = {
  url?: string;
  logLevel?: LogVerbosity;
  autoConnect?: boolean;
  reconnect?: boolean;
  websocket?: typeof WebSocket;
};

export interface BridgethingClient extends ClientSurfaces {}

export class BridgethingClient {
  public readonly logger: Logger;
  public readonly url: string;
  private readonly listeners: Set<ClientListener> = new Set();
  private readonly pending: Map<string, PendingRequest> = new Map();
  private readonly sendQueue: QueuedSend[] = [];
  private readonly websocketCtor: typeof WebSocket;
  private readonly reconnectEnabled: boolean;

  private socket: WebSocket | null = null;
  private state: ConnectionState = 'closed';
  private reconnectAttempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionalClose = false;

  constructor(options: ClientOptions = {}) {
    this.logger = new Logger('Client', options.logLevel ?? LogVerbosity.Log);
    this.url = options.url ?? DEFAULT_URL;
    this.reconnectEnabled = options.reconnect ?? true;
    this.websocketCtor =
      options.websocket ?? (typeof WebSocket !== 'undefined' ? WebSocket : (undefined as unknown as typeof WebSocket));

    if (!this.websocketCtor) {
      throw new ClientError(
        'no global WebSocket available; pass `options.websocket` (e.g. from `ws` on Node)',
        'not-running',
      );
    }

    if (options.autoConnect ?? true) this.connect();
  }

  get connectionState(): ConnectionState {
    return this.state;
  }

  on(listener: ClientListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  off(listener: ClientListener): void {
    this.listeners.delete(listener);
  }

  connect(): void {
    if (this.state === 'open' || this.state === 'connecting') return;
    this.intentionalClose = false;
    this.openSocket();
  }

  close(): void {
    this.intentionalClose = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.socket && (this.state === 'open' || this.state === 'connecting')) {
      this.state = 'closing';
      this.socket.close();
    }
    const shutdown = new ClientError('client shutting down', 'shutdown');
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timeout);
      pending.reject(shutdown);
    }
    this.pending.clear();
    while (this.sendQueue.length > 0) this.sendQueue.shift()!.reject(shutdown);
  }

  async send(message: ClientToBridgeMsg): Promise<void> {
    this.logger.trace('send', message);
    const frame = new Uint8Array(msgpackEncode(walkUuidFields(message, 'encode')));
    if (this.socket && this.state === 'open') {
      try {
        this.socket.send(frame);
      } catch (err) {
        throw new ClientError('failed to send message', 'send-failed', err);
      }
      return;
    }
    if (this.intentionalClose || this.state === 'closing') {
      throw new ClientError('client not connected', 'not-connected');
    }
    return new Promise<void>((resolve, reject) => {
      this.sendQueue.push({ frame, resolve, reject });
    });
  }

  request(data: ClientToBridgeMsgData, timeoutMs: number = DEFAULT_REQUEST_TIMEOUT_MS): Promise<BridgeToClientMsg> {
    const id = newUuid();
    const message: ClientToBridgeMsg = { id, meta: { kind: 'request' }, data };

    return new Promise<BridgeToClientMsg>((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (this.pending.delete(id)) {
          reject(new ClientError(`request ${id} timed out`, 'request-timed-out'));
        }
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timeout });

      this.send(message).catch(err => {
        if (this.pending.delete(id)) {
          clearTimeout(timeout);
          reject(new ClientError(`failed to send request ${id}`, 'send-failed', err));
        }
      });
    });
  }

  private openSocket(): void {
    this.state = 'connecting';
    this.emit({ type: 'connecting', attempt: this.reconnectAttempt });

    let socket: WebSocket;
    try {
      socket = new this.websocketCtor(this.url);
    } catch (err) {
      this.logger.error('failed to construct WebSocket', err);
      this.scheduleReconnect();
      return;
    }
    this.socket = socket;
    socket.binaryType = 'arraybuffer';

    socket.addEventListener('open', () => {
      this.state = 'open';
      this.reconnectAttempt = 0;
      this.flushSendQueue();
      this.emit({ type: 'open' });
    });

    socket.addEventListener('message', event => this.handleMessage(event.data));

    socket.addEventListener('close', event => {
      this.socket = null;
      this.state = 'closed';
      this.emit({ type: 'close', code: event.code, reason: event.reason });
      if (!this.intentionalClose && this.reconnectEnabled) this.scheduleReconnect();
    });

    socket.addEventListener('error', () => {
      this.logger.warn('websocket error event (will close shortly)');
    });
  }

  private scheduleReconnect(): void {
    const idx = Math.min(this.reconnectAttempt, RECONNECT_BACKOFF_MS.length - 1);
    const delay = RECONNECT_BACKOFF_MS[idx];
    this.reconnectAttempt += 1;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.openSocket();
    }, delay);
  }

  private handleMessage(raw: unknown): void {
    let msg: BridgeToClientMsg;
    try {
      if (raw instanceof ArrayBuffer) {
        msg = walkUuidFields(msgpackDecode(new Uint8Array(raw)), 'decode') as BridgeToClientMsg;
      } else if (typeof raw === 'string') {
        msg = JSON.parse(raw) as BridgeToClientMsg;
      } else {
        this.emit({ type: 'decodeError', description: 'unknown message payload type' });
        return;
      }
    } catch (err) {
      this.emit({ type: 'decodeError', description: errorMessage(err) });
      return;
    }

    if (msg.meta.kind === 'response' && this.completePending(msg.meta.data.requestId, msg)) {
      return;
    }
    this.emit({ type: 'message', message: msg });
  }

  private flushSendQueue(): void {
    while (this.sendQueue.length > 0) {
      const queued = this.sendQueue.shift()!;
      if (!this.socket || this.state !== 'open') {
        queued.reject(new ClientError('client not connected', 'not-connected'));
        continue;
      }
      try {
        this.socket.send(queued.frame);
        queued.resolve();
      } catch (err) {
        queued.reject(new ClientError('failed to send message', 'send-failed', err));
      }
    }
  }

  private completePending(requestId: string, msg: BridgeToClientMsg): boolean {
    const pending = this.pending.get(requestId);
    if (!pending) return false;
    this.pending.delete(requestId);
    clearTimeout(pending.timeout);
    pending.resolve(msg);
    return true;
  }

  private emit(event: ClientEvent): void {
    this.logger.trace('event', event.type, event);
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch (err) {
        this.logger.error('client listener threw', err);
      }
    }
  }
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

import { applyDispatch } from './dispatch.generated.js';
applyDispatch();
