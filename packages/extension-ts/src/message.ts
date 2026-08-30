import type { ForwardMessage } from '@bridgething/lib/shared';
import type { WireForwardMessage } from './protocol.js';

export type { ForwardMessage };

/**
 * Anything `send` and `broadcast` accept. The runtime sends a string as `text` and a `Uint8Array`
 * as `binary`. Wrap any other value with {@link json}.
 */
export type SendableMessage = ForwardMessage | string | Uint8Array;

/** Builds a `text` forward message. */
export function text(data: string): ForwardMessage {
  return { encoding: 'text', data };
}

/** Builds a `json` forward message from a JSON-serializable value. */
export function json(data: unknown): ForwardMessage {
  return { encoding: 'json', data };
}

/** Builds a `binary` forward message. */
export function binary(data: Uint8Array): ForwardMessage {
  return { encoding: 'binary', data };
}

/** The payload if this message is `text`, otherwise `undefined`. */
export function asText(message: ForwardMessage): string | undefined {
  return message.encoding === 'text' ? message.data : undefined;
}

/** The payload if this message is `json`, otherwise `undefined`. `T` is an unchecked cast. */
export function asJson<T = unknown>(message: ForwardMessage): T | undefined {
  return message.encoding === 'json' ? (message.data as T) : undefined;
}

/** The payload if this message is `binary`, otherwise `undefined`. */
export function asBinary(message: ForwardMessage): Uint8Array | undefined {
  return message.encoding === 'binary' ? message.data : undefined;
}

const CHUNK = 0x8000;

function toBase64(bytes: Uint8Array): string {
  let latin1 = '';
  for (let i = 0; i < bytes.length; i += CHUNK) {
    latin1 += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(latin1);
}

function fromBase64(data: string): Uint8Array {
  const latin1 = atob(data);
  const bytes = new Uint8Array(latin1.length);
  for (let i = 0; i < latin1.length; i++) bytes[i] = latin1.charCodeAt(i);
  return bytes;
}

export function intoWire(message: SendableMessage): WireForwardMessage {
  if (typeof message === 'string') return { encoding: 'text', data: message };
  if (message instanceof Uint8Array) return { encoding: 'binary', data: toBase64(message) };
  if (message.encoding === 'binary') return { encoding: 'binary', data: toBase64(message.data) };
  return message;
}

export function fromWire(message: WireForwardMessage): ForwardMessage {
  if (message.encoding === 'binary') return { encoding: 'binary', data: fromBase64(message.data) };
  return message;
}
