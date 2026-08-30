import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import type { Device, ExtensionContext } from '../src/context.js';
import { binary, defineExtension, ExtensionError, json } from '../src/index.js';
import type { ForwardMessage } from '../src/message.js';
import {
  EXTENSION_API_VERSION,
  EXTENSION_MESSAGE_TYPES,
  HOST_MESSAGE_TYPES,
  type ExtensionMessage,
  type HostMessage,
} from '../src/protocol.js';
import { FakeHost } from './fake-host.js';

type Fixture = {
  api: number;
  hostToExtension: Record<string, HostMessage>;
  extensionToHost: Record<string, ExtensionMessage>;
};

const fixture = JSON.parse(readFileSync(new URL('../fixtures/protocol.v1.json', import.meta.url), 'utf8')) as Fixture;

function inbound(name: string): HostMessage {
  const message = fixture.hostToExtension[name];
  if (!message) throw new Error(`the shared fixture has no host message named ${name}`);
  return message;
}

function outbound(name: string): ExtensionMessage {
  const message = fixture.extensionToHost[name];
  if (!message) throw new Error(`the shared fixture has no extension message named ${name}`);
  return message;
}

function discriminants(section: Record<string, { t: string }>): string[] {
  return [...new Set(Object.values(section).map(message => message.t))].sort();
}

describe('the shared protocol fixture describes the whole protocol', () => {
  test('every host message type the union is built from has a fixture entry, and the fixture invents none', () => {
    expect(discriminants(fixture.hostToExtension)).toEqual([...HOST_MESSAGE_TYPES].sort());
  });

  test('every extension message type the union is built from has a fixture entry, and the fixture invents none', () => {
    expect(discriminants(fixture.extensionToHost)).toEqual([...EXTENSION_MESSAGE_TYPES].sort());
  });
});

describe('the shared protocol fixture, host to extension', () => {
  test('the api revision both sides speak is the one the fixture pins', () => {
    expect(fixture.api).toBe(EXTENSION_API_VERSION);
  });

  test('hello lands as identity, and every device line lands as device state', async () => {
    const host = new FakeHost();
    const events: string[] = [];
    const messages: [string, ForwardMessage][] = [];
    const configs: [string, string, string | null][] = [];
    let ctx: ExtensionContext | undefined;
    let onConnect: Pick<Device, 'id' | 'name' | 'active' | 'config'> | undefined;

    const run = defineExtension(
      {
        start(seen) {
          ctx = seen;
          seen.on('device', event => {
            events.push(`${event.type}:${event.device.id}`);
            const { id, name, active, config } = event.device;
            if (event.type === 'connected') onConnect = { id, name, active, config };
          });
          seen.on('message', (device, message) => messages.push([device.id, message]));
          seen.on('config', (device, key, value) => configs.push([device.id, key, value]));
        },
      },
      host,
    );

    host.send(inbound('hello'));
    await host.expect('ready');

    expect(ctx?.api).toBe(1);
    expect(ctx?.webapp).toEqual({ id: '019e6701-13f8-71b5-ba04-85d326630e98', name: 'weather', version: '1.2.0' });
    expect(ctx?.dataDir).toBe('/data/weather');

    host.send(inbound('device.connected'));
    host.send(inbound('device.message.text'), inbound('device.message.json'), inbound('device.message.binary'));
    host.send(inbound('config.changed.set'), inbound('config.changed.reset'));
    host.send(inbound('device.active'), inbound('device.disconnected'));
    host.send(inbound('stop'));
    await run;

    expect(messages).toEqual([
      ['0f3ab21c', { encoding: 'text', data: 'pong' }],
      ['0f3ab21c', { encoding: 'json', data: { ok: true } }],
      ['0f3ab21c', { encoding: 'binary', data: new Uint8Array([0, 1, 2, 250, 255]) }],
    ]);
    expect(configs).toEqual([
      ['0f3ab21c', 'zip', '10001'],
      ['0f3ab21c', 'zip', null],
    ]);
    expect(events).toEqual(['connected:0f3ab21c', 'active:0f3ab21c', 'disconnected:0f3ab21c']);
    expect(onConnect).toEqual({ id: '0f3ab21c', name: 'car thing', active: true, config: { zip: '10001' } });
    expect(host.exitCode).toBe(0);
  });

  test('an ok reply settles the request that carries the same id', async () => {
    const host = new FakeHost();
    let answered: unknown;
    const run = defineExtension(
      {
        start(ctx) {
          void ctx.kv.get('creds').then(value => {
            answered = value;
          });
        },
      },
      host,
    );

    host.send(inbound('hello'));
    await host.expect('kv.get');
    host.send(inbound('reply.ok'));
    await host.expect('ready');
    host.send(inbound('stop'));
    await run;

    expect(answered).toEqual({ token: 'abc' });
  });

  test('a refusal reply rejects with the error the host wrote', async () => {
    const host = new FakeHost();
    let caught: unknown;
    const run = defineExtension(
      {
        start(ctx) {
          void ctx.kv.get('creds').catch((err: unknown) => {
            caught = err;
          });
        },
      },
      host,
    );

    host.send(inbound('hello'));
    await host.expect('kv.get');
    host.send(inbound('reply.error'));
    await host.expect('ready');
    host.send(inbound('stop'));
    await run;

    expect(caught).toBeInstanceOf(ExtensionError);
    expect((caught as ExtensionError).kind).toBe('host-error');
    expect((caught as ExtensionError).message).toBe('the host refused');
  });
});

describe('the shared protocol fixture, extension to host', () => {
  test('the runtime writes every fixture line, byte for byte', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.log.info('listening');
          ctx.device('0f3ab21c').send('ping');
          ctx.broadcast(json({ cmd: 'refresh' }));
          ctx.device('0f3ab21c').send(binary(new Uint8Array([0, 1, 2, 250, 255])));
          void ctx.kv.get('creds').catch(() => undefined);
          void ctx.kv.set('creds', { token: 'abc' }).catch(() => undefined);
          void ctx.kv.delete('creds').catch(() => undefined);
          void ctx.kv.list().catch(() => undefined);
          void ctx.auth.authorize('https://example.test/authorize').catch(() => undefined);
        },
      },
      host,
    );

    host.send(inbound('hello'));
    await host.expect('ready');
    host.send(inbound('stop'));
    await run;

    expect(host.written).toEqual([
      outbound('log'),
      outbound('device.send.text'),
      outbound('device.send.broadcast'),
      outbound('device.send.binary'),
      outbound('kv.get'),
      outbound('kv.set'),
      outbound('kv.delete'),
      outbound('kv.list'),
      outbound('auth.authorize'),
      outbound('ready'),
    ]);
  });
});
