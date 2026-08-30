import { describe, expect, test } from 'bun:test';
import type { Device, DeviceEvent, ExtensionContext } from '../src/context.js';
import { asBinary, asJson, asText, binary, defineExtension, denoHost, ExtensionError, json } from '../src/index.js';
import type { ForwardMessage } from '../src/message.js';
import type { ExtensionMessage } from '../src/protocol.js';
import { connected, FakeHost, hello } from './fake-host.js';

function logsOf(written: ExtensionMessage[]): string[] {
  return written.filter(message => message.t === 'log').map(message => message.message);
}

function sendsOf(written: ExtensionMessage[]): Extract<ExtensionMessage, { t: 'device.send' }>[] {
  return written.filter(message => message.t === 'device.send');
}

describe('handshake', () => {
  test('start sees the hello payload and ready follows it', async () => {
    const host = new FakeHost();
    let seen: ExtensionContext | undefined;
    const run = defineExtension(
      {
        start(ctx) {
          seen = ctx;
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');

    expect(seen?.api).toBe(1);
    expect(seen?.webapp.name).toBe('test-app');
    expect(seen?.dataDir).toBe('/tmp/bridgething-extension-test');
    expect(host.written[0]).toEqual({ t: 'ready' });

    host.close();
    await run;
  });

  test('ready waits for an async start', async () => {
    const host = new FakeHost();
    let release: (() => void) | undefined;
    const gate = new Promise<void>(resolve => {
      release = resolve;
    });
    const run = defineExtension(
      {
        async start(ctx) {
          ctx.log.info('starting');
          await gate;
        },
      },
      host,
    );

    host.send(hello());
    await host.waitFor(1);
    expect(host.written.map(m => m.t)).toEqual(['log']);

    release?.();
    await host.expect('ready');

    host.close();
    await run;
  });

  test('a start that throws exits nonzero without ready', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start() {
          throw new Error('boom');
        },
      },
      host,
    );

    host.send(hello());
    await run;

    expect(host.exitCode).toBe(1);
    expect(host.written.some(m => m.t === 'ready')).toBe(false);
    expect(logsOf(host.written)[0]).toContain('start failed');
  });
});

describe('devices', () => {
  test('connect, active, disconnect all reach one listener', async () => {
    const host = new FakeHost();
    const events: DeviceEvent[] = [];
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => {
            events.push(event);
            ctx.log.info(event.type, event.device.id, String(ctx.devices.length));
          });
        },
      },
      host,
    );

    host.send(
      hello(),
      connected('serial-a'),
      { t: 'device.active', device: 'serial-a', active: false },
      {
        t: 'device.disconnected',
        device: 'serial-a',
      },
    );
    await host.waitFor(4);

    expect(logsOf(host.written)).toEqual(['connected serial-a 1', 'active serial-a 1', 'disconnected serial-a 0']);
    expect(events.map(e => e.type)).toEqual(['connected', 'active', 'disconnected']);

    host.close();
    await run;
  });

  test('a disconnected device keeps its handle, name, and config', async () => {
    const host = new FakeHost();
    let device: Device | undefined;
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => {
            device = ctx.device('serial-a');
            ctx.log.info(event.type);
          });
        },
      },
      host,
    );

    host.send(hello(), connected('serial-a', { name: 'garage', config: { room: 'garage' } }), {
      t: 'device.disconnected',
      device: 'serial-a',
    });
    await host.waitFor(3);

    expect(device?.name).toBe('garage');
    expect(device?.connected).toBe(false);
    expect(device?.config).toEqual({ room: 'garage' });

    host.close();
    await run;
  });

  test('disconnect for an unknown device is ignored', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => ctx.log.info(event.type));
        },
      },
      host,
    );

    host.send(hello(), { t: 'device.disconnected', device: 'ghost' }, connected('serial-a'));
    await host.waitFor(2);

    expect(logsOf(host.written)).toEqual(['connected']);

    host.close();
    await run;
  });

  test('config snapshots update in place and notify', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('config', (device, key, value) => {
            ctx.log.info(key, String(value), JSON.stringify(ctx.config(device)));
          });
        },
      },
      host,
    );

    host.send(hello(), connected('serial-a', { config: { room: 'kitchen' } }), {
      t: 'config.changed',
      device: 'serial-a',
      key: 'brightness',
      value: '80',
    });
    await host.waitFor(2);

    expect(logsOf(host.written)).toEqual(['brightness 80 {"room":"kitchen","brightness":"80"}']);

    host.close();
    await run;
  });

  test('a cleared setting arrives as null and leaves the snapshot', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('config', (device, key, value) => {
            ctx.log.info(key, String(value), JSON.stringify(ctx.config(device)));
          });
        },
      },
      host,
    );

    host.send(hello(), connected('serial-a', { config: { room: 'kitchen' } }), {
      t: 'config.changed',
      device: 'serial-a',
      key: 'room',
      value: null,
    });
    await host.waitFor(2);

    expect(logsOf(host.written)).toEqual(['room null {}']);

    host.close();
    await run;
  });

  test('unsubscribe drops the listener', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          const off = ctx.on('device', event => {
            ctx.log.info(event.type);
            off();
          });
          ctx.on('message', (_device, message) => ctx.log.info(asText(message) ?? ''));
        },
      },
      host,
    );

    host.send(hello(), connected('serial-a'), connected('serial-b'), {
      t: 'device.message',
      device: 'serial-a',
      message: { encoding: 'text', data: 'probe' },
    });
    await host.waitFor(3);

    expect(logsOf(host.written)).toEqual(['connected', 'probe']);

    host.close();
    await run;
  });
});

describe('forward messages', () => {
  test('every encoding decodes on the way in', async () => {
    const host = new FakeHost();
    const seen: ForwardMessage[] = [];
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('message', (device, message) => {
            seen.push(message);
            ctx.log.info(device.id);
          });
        },
      },
      host,
    );

    host.send(
      hello(),
      connected('serial-a'),
      { t: 'device.message', device: 'serial-a', message: { encoding: 'text', data: 'hi' } },
      { t: 'device.message', device: 'serial-a', message: { encoding: 'json', data: { n: 1 } } },
      { t: 'device.message', device: 'serial-a', message: { encoding: 'binary', data: 'AAECA/8=' } },
    );
    await host.waitFor(4);

    expect(asText(seen[0])).toBe('hi');
    expect(asJson<{ n: number }>(seen[1])).toEqual({ n: 1 });
    expect(asBinary(seen[2])).toEqual(new Uint8Array([0, 1, 2, 3, 255]));
    expect(asText(seen[1])).toBeUndefined();

    host.close();
    await run;
  });

  test('send and broadcast encode binary as base64 and address correctly', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.device('serial-a').send('plain text');
          ctx.device('serial-a').send(new Uint8Array([0, 1, 2, 3, 255]));
          ctx.device('serial-a').send(json({ hello: true }));
          ctx.broadcast(binary(new Uint8Array([255])));
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');

    expect(sendsOf(host.written)).toEqual([
      { t: 'device.send', device: 'serial-a', message: { encoding: 'text', data: 'plain text' } },
      { t: 'device.send', device: 'serial-a', message: { encoding: 'binary', data: 'AAECA/8=' } },
      { t: 'device.send', device: 'serial-a', message: { encoding: 'json', data: { hello: true } } },
      { t: 'device.send', message: { encoding: 'binary', data: '/w==' } },
    ]);

    host.close();
    await run;
  });

  test('a payload larger than one base64 chunk survives the round trip', async () => {
    const host = new FakeHost();
    const payload = new Uint8Array(200_000);
    for (let i = 0; i < payload.length; i++) payload[i] = i % 256;

    let echoed: Uint8Array | undefined;
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('message', (_device, message) => {
            echoed = asBinary(message);
            ctx.log.info('echoed');
          });
          ctx.broadcast(payload);
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');

    const sent = sendsOf(host.written)[0];
    expect(sent.message.encoding).toBe('binary');
    host.send({ t: 'device.message', device: 'serial-a', message: sent.message });
    await host.expect('log');

    expect(echoed).toEqual(payload);

    host.close();
    await run;
  });
});

describe('requests', () => {
  test('kv round trips by correlation id', async () => {
    const host = new FakeHost();
    let value: unknown;
    let keys: string[] = [];
    const run = defineExtension(
      {
        async start(ctx) {
          const pending = ctx.kv.get<{ token: string }>('creds');
          const listing = ctx.kv.list();
          await host.waitFor(2);
          host.send({ t: 'reply', id: '2', ok: true, value: ['creds', 'other'] });
          host.send({ t: 'reply', id: '1', ok: true, value: { token: 'abc' } });
          value = await pending;
          keys = await listing;
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');

    expect(host.written.slice(0, 2)).toEqual([
      { t: 'kv.get', id: '1', key: 'creds' },
      { t: 'kv.list', id: '2' },
    ]);
    expect(value).toEqual({ token: 'abc' });
    expect(keys).toEqual(['creds', 'other']);

    host.close();
    await run;
  });

  test('a missing key resolves undefined and set/delete resolve void', async () => {
    const host = new FakeHost();
    const results: unknown[] = [];
    const run = defineExtension(
      {
        async start(ctx) {
          const missing = ctx.kv.get('nope');
          await host.waitFor(1);
          host.send({ t: 'reply', id: '1', ok: true, value: null });
          results.push(await missing);

          const stored = ctx.kv.set('k', { a: 1 });
          await host.waitFor(2);
          host.send({ t: 'reply', id: '2', ok: true });
          results.push(await stored);

          const removed = ctx.kv.delete('k');
          await host.waitFor(3);
          host.send({ t: 'reply', id: '3', ok: true });
          results.push(await removed);
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');

    expect(results).toEqual([undefined, undefined, undefined]);
    expect(host.written[1]).toEqual({ t: 'kv.set', id: '2', key: 'k', value: { a: 1 } });
    expect(host.written[2]).toEqual({ t: 'kv.delete', id: '3', key: 'k' });

    host.close();
    await run;
  });

  test('an error reply rejects with ExtensionError', async () => {
    const host = new FakeHost();
    let caught: unknown;
    const run = defineExtension(
      {
        async start(ctx) {
          const pending = ctx.auth.authorize('https://accounts.example.com/authorize');
          await host.waitFor(1);
          host.send({ t: 'reply', id: '1', ok: false, error: 'the user closed the browser' });
          caught = await pending.catch((err: unknown) => err);
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');

    expect(host.written[0]).toEqual({
      t: 'auth.authorize',
      id: '1',
      url: 'https://accounts.example.com/authorize',
    });
    expect(caught).toBeInstanceOf(ExtensionError);
    expect((caught as ExtensionError).kind).toBe('host-error');
    expect((caught as ExtensionError).message).toBe('the user closed the browser');

    host.close();
    await run;
  });

  test('authorize resolves with the callback url', async () => {
    const host = new FakeHost();
    let callback = '';
    const run = defineExtension(
      {
        async start(ctx) {
          const pending = ctx.auth.authorize('https://accounts.example.com/authorize');
          await host.waitFor(1);
          host.send({ t: 'reply', id: '1', ok: true, value: 'bridgething://cb?code=xyz' });
          callback = await pending;
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');
    expect(callback).toBe('bridgething://cb?code=xyz');

    host.close();
    await run;
  });

  test('pending requests reject when the stream ends', async () => {
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

    host.send(hello());
    await host.waitFor(1);
    host.close();
    await run;
    await Promise.resolve();

    expect(caught).toBeInstanceOf(ExtensionError);
    expect((caught as ExtensionError).kind).toBe('disconnected');
    expect(host.exitCode).toBe(0);
  });
});

describe('lifecycle', () => {
  test('stop runs the hook then exits zero', async () => {
    const host = new FakeHost();
    const order: string[] = [];
    const run = defineExtension(
      {
        start() {
          order.push('start');
        },
        stop() {
          order.push('stop');
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');
    host.send({ t: 'stop' });
    await run;

    expect(order).toEqual(['start', 'stop']);
    expect(host.exitCode).toBe(0);
  });

  test('a stop hook that throws still exits and reports', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start() {
          return;
        },
        stop() {
          throw new Error('cleanup failed');
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');
    host.send({ t: 'stop' });
    await run;

    expect(host.exitCode).toBe(0);
    expect(logsOf(host.written).some(line => line.includes('stop failed'))).toBe(true);
  });

  test('stream end runs stop and exits zero', async () => {
    const host = new FakeHost();
    let stopped = false;
    const run = defineExtension(
      {
        start() {
          return;
        },
        stop() {
          stopped = true;
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');
    host.close();
    await run;

    expect(stopped).toBe(true);
    expect(host.exitCode).toBe(0);
  });

  test('a kv write raised from stop rejects instead of hanging the shutdown', async () => {
    const host = new FakeHost();
    const order: string[] = [];
    let caught: unknown;
    let context!: ExtensionContext;
    const run = defineExtension(
      {
        start(ctx) {
          context = ctx;
        },
        async stop() {
          order.push('stop');
          await context.kv.set('state', { a: 1 }).catch((err: unknown) => {
            caught = err;
          });
          order.push('flushed');
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');
    host.send({ t: 'stop' });
    await run;

    expect(order).toEqual(['stop', 'flushed']);
    expect(caught).toBeInstanceOf(ExtensionError);
    expect((caught as ExtensionError).kind).toBe('disconnected');
    expect((caught as ExtensionError).message).toContain('persist eagerly rather than from stop');
    expect(host.exitCode).toBe(0);
  }, 2000);

  test('an authorize raised from stop rejects with the same advice', async () => {
    const host = new FakeHost();
    let caught: unknown;
    let context!: ExtensionContext;
    const run = defineExtension(
      {
        start(ctx) {
          context = ctx;
        },
        async stop() {
          await context.auth.authorize('https://example.test/authorize').catch((err: unknown) => {
            caught = err;
          });
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');
    host.send({ t: 'stop' });
    await run;

    expect(caught).toBeInstanceOf(ExtensionError);
    expect((caught as ExtensionError).message).toContain('persist eagerly rather than from stop');
    expect(host.written.some(message => message.t === 'auth.authorize')).toBe(false);
  }, 2000);

  test('an extension with no stop hook still exits', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start() {
          return;
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');
    host.send({ t: 'stop' });
    await run;

    expect(host.exitCode).toBe(0);
  });
});

describe('resilience', () => {
  test('a malformed line is reported and the loop survives', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => ctx.log.info(event.type));
        },
      },
      host,
    );

    host.send(hello());
    host.sendRaw('{ not json');
    host.send(connected('serial-a'));
    await host.waitFor(3);

    const logs = logsOf(host.written);
    expect(logs[0]).toContain('unparseable line from host');
    expect(logs[1]).toBe('connected');

    host.close();
    await run;
  });

  test('a throwing listener does not stop delivery', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', () => {
            throw new Error('listener exploded');
          });
          ctx.on('device', event => ctx.log.info(event.type));
        },
      },
      host,
    );

    host.send(hello(), connected('serial-a'));
    await host.waitFor(3);

    const logs = logsOf(host.written);
    expect(logs[0]).toContain('listener exploded');
    expect(logs[1]).toBe('connected');

    host.close();
    await run;
  });

  test('log levels and argument formatting', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.log.debug('a', 1, { b: 2 });
          ctx.log.warn('careful');
          ctx.log.error(new Error('nope'));
        },
      },
      host,
    );

    host.send(hello());
    await host.expect('ready');

    const written = host.written.filter(m => m.t === 'log');
    expect(written[0]).toEqual({ t: 'log', level: 'debug', message: 'a 1 {"b":2}' });
    expect(written[1]).toEqual({ t: 'log', level: 'warn', message: 'careful' });
    expect(written[2].level).toBe('error');
    expect(written[2].message).toContain('nope');

    host.close();
    await run;
  });

  test('an unknown reply id is ignored', async () => {
    const host = new FakeHost();
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => ctx.log.info(event.type));
        },
      },
      host,
    );

    host.send(hello(), { t: 'reply', id: '99', ok: true, value: 1 }, connected('serial-a'));
    await host.waitFor(2);

    expect(logsOf(host.written)).toEqual(['connected']);

    host.close();
    await run;
  });
});

describe('transport', () => {
  test('a batch split mid-line reassembles into whole messages', async () => {
    const host = new FakeHost();
    const events: string[] = [];
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => events.push(`${event.type}:${event.device.id}`));
        },
      },
      host,
    );

    const raw = [JSON.stringify(hello()), JSON.stringify(connected('aa')), JSON.stringify(connected('bb'))].join('\n');
    host.sendChunked(`${raw}\n`, [20, 35, 9, 120]);
    await host.expect('ready');
    host.send({ t: 'stop' });
    await run;

    expect(events).toEqual(['connected:aa', 'connected:bb']);
  });

  test('a chunk that cuts a multibyte character still decodes it', async () => {
    const host = new FakeHost();
    const seen: string[] = [];
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => seen.push(event.device.name));
        },
      },
      host,
    );

    const raw = `${JSON.stringify(hello())}\n${JSON.stringify(connected('aa', { name: 'auto \u2014 salon' }))}\n`;
    const cut = new TextEncoder().encode(raw).indexOf(0xe2) + 1;
    expect(cut).toBeGreaterThan(0);

    host.sendChunked(raw, [cut]);
    host.close();
    await run;

    expect(seen).toEqual(['auto \u2014 salon']);
  });

  test('a final line with no trailing newline is still delivered', async () => {
    const host = new FakeHost();
    const events: string[] = [];
    const run = defineExtension(
      {
        start(ctx) {
          ctx.on('device', event => events.push(event.device.id));
        },
      },
      host,
    );

    host.sendChunked(`${JSON.stringify(hello())}\n${JSON.stringify(connected('cc'))}`, [30]);
    host.close();
    await run;

    expect(events).toEqual(['cc']);
    expect(host.exitCode).toBe(0);
  });

  test('denoHost refuses to run anywhere there is no Deno global', () => {
    expect((globalThis as { Deno?: unknown }).Deno).toBeUndefined();

    let failure: unknown;
    try {
      denoHost();
    } catch (err) {
      failure = err;
    }

    expect(failure).toBeInstanceOf(ExtensionError);
    expect((failure as ExtensionError).kind).toBe('no-runtime');
  });
});
