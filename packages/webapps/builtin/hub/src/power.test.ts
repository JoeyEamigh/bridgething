import { type BridgeToClientMsg, BridgethingClient, type ClientToBridgeMsg, walkUuidFields } from '@bridgething/client';
import { decode } from '@msgpack/msgpack';
import { describe, expect, test } from 'bun:test';
import { factoryReset } from './power.ts';

type Answer = (request: ClientToBridgeMsg) => BridgeToClientMsg['data'];

function daemonAnswering(answer: Answer) {
  const asked: ClientToBridgeMsg[] = [];
  class Daemon extends EventTarget {
    binaryType = 'arraybuffer';

    constructor() {
      super();
      queueMicrotask(() => this.dispatchEvent(new Event('open')));
    }

    send(frame: Uint8Array) {
      const request = walkUuidFields(decode(frame), 'decode') as ClientToBridgeMsg;
      asked.push(request);
      const reply: BridgeToClientMsg = {
        id: crypto.randomUUID(),
        meta: { kind: 'response', data: { requestId: request.id } },
        data: answer(request),
      };
      queueMicrotask(() => this.dispatchEvent(new MessageEvent('message', { data: JSON.stringify(reply) })));
    }

    close() {}
  }
  const client = new BridgethingClient({ websocket: Daemon as unknown as typeof WebSocket, reconnect: false });
  return { client, asked };
}

describe('factoryReset', () => {
  test('asks the daemon for a factory reset as a request it answers', async () => {
    const { client, asked } = daemonAnswering(() => ({
      type: 'system',
      data: { event: 'factoryResetStarted', data: {} },
    }));

    expect(await factoryReset(client)).toBeUndefined();

    expect(asked.map(({ meta, data }) => ({ meta, data }))).toEqual([
      { meta: { kind: 'request' }, data: { type: 'system', data: { event: 'factoryReset' } } },
    ]);
  });

  test('a refusal from the daemon comes back as its reason', async () => {
    const reason = 'this system image cannot erase the device; update the system image, then reset again';
    const { client } = daemonAnswering(() => ({
      type: 'system',
      data: { event: 'factoryResetRefused', data: { refusal: 'noWipeStep', reason } },
    }));

    expect(await factoryReset(client)).toBe(reason);
  });

  test('a daemon that cannot take the request says so instead of passing for a reset', async () => {
    const { client } = daemonAnswering(() => ({ type: 'error', data: { type: 'unsupported' } }));

    expect(await factoryReset(client)).toBe('failed: unsupported');
  });
});
