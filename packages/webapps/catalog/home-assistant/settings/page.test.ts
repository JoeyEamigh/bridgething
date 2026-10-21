import type { ConfigField, SettingsHost } from '@bridgething/client/settings-protocol';
import { settingsPage, type SettingsPage } from '@bridgething/client/testing';
import { beforeAll, describe, expect, test } from 'bun:test';
import { Window } from 'happy-dom';
import { resolve } from 'node:path';
import { build, type Rollup } from 'vite';

const PACKAGE = resolve(import.meta.dir, '..');
const SELECTION = 'selected_entities';

const FIELDS = [
  { type: 'string', data: { key: 'base_url', label: 'Home Assistant URL' } },
  { type: 'secret', data: { key: 'token', label: 'Long-Lived Access Token' } },
] as ConfigField[];

const STATES = [
  { entity_id: 'light.kitchen', state: 'on', attributes: { friendly_name: 'Kitchen' } },
  { entity_id: 'switch.fan', state: 'off', attributes: { friendly_name: 'Fan' } },
];

let html: string;

beforeAll(async () => {
  const output = (await build({
    configFile: resolve(PACKAGE, 'vite.settings.config.ts'),
    root: resolve(PACKAGE, 'settings'),
    logLevel: 'silent',
    build: { write: false },
  })) as Rollup.RollupOutput;
  const page = output.output.find(file => file.fileName === 'settings.html');
  if (!page || page.type !== 'asset') throw new Error('the settings build produced no settings.html');
  html = String(page.source);
});

type Device = { config: Map<string, string>; doc: Map<string, string>; events: string[] };

function device(): Device {
  return { config: new Map(), doc: new Map(), events: [] };
}

function host(held: Device): SettingsHost {
  const unused = () => Promise.reject(new Error('unused'));
  return {
    context: () => Promise.resolve({ webappId: 'ha', name: 'Home Assistant', version: '0.9.0', deviceId: 'thing' }),
    'config.fields': () => Promise.resolve(FIELDS),
    'config.list': () => Promise.resolve([...held.config].map(([key, value]) => ({ key, value }))),
    'config.set': ({ key, value }) => {
      held.config.set(key, value);
      held.events.push(`set ${key}=${value}`);
      return Promise.resolve({ key, value });
    },
    'config.delete': unused,
    'doc.get': ({ key }) => {
      held.events.push(`read ${key}`);
      return Promise.resolve({ key, value: held.doc.get(key) ?? null });
    },
    'doc.list': unused,
    'doc.set': ({ key, value }) => {
      held.doc.set(key, value);
      return Promise.resolve({ key, value });
    },
    'doc.delete': unused,
    fetch: unused,
    'auth.authorize': unused,
    pick: unused,
    'blob.create': unused,
    'blob.append': unused,
    'blob.put': unused,
    'blob.delete': unused,
    'blob.list': unused,
    done: () => void held.events.push('done'),
  };
}

type Opened = SettingsPage<string, Window> & { sockets: Array<{ url: string; sent: unknown[] }> };

async function open(held: Device): Promise<Opened> {
  const window = new Window({
    url: 'about:blank',
    settings: { enableJavaScriptEvaluation: true, suppressInsecureJavaScriptEnvironmentWarning: true },
  });
  const sockets: Opened['sockets'] = [];
  class HomeAssistant {
    onmessage: ((event: { data: string }) => void) | null = null;
    onerror: (() => void) | null = null;
    onclose: (() => void) | null = null;
    private readonly record: { url: string; sent: unknown[] };

    constructor(url: string) {
      this.record = { url, sent: [] };
      sockets.push(this.record);
      setTimeout(() => this.reply({ type: 'auth_required' }), 0);
    }

    send(json: string): void {
      const message = JSON.parse(json) as { type: string; id?: number };
      this.record.sent.push(message);
      if (message.type === 'auth') this.reply({ type: 'auth_ok' });
      if (message.type === 'get_states') this.reply({ type: 'result', id: message.id, success: true, result: STATES });
    }

    close(): void {}

    private reply(message: object): void {
      setTimeout(() => this.onmessage?.({ data: JSON.stringify(message) }), 0);
    }
  }
  Object.assign(window, { WebSocket: HomeAssistant });

  const page = settingsPage<string, Window>(window, { target: 'ha', bind: () => host(held) });
  page.load(html);
  await page.until(
    () => page.frame.document.querySelector('#bt-field-token') !== null && page.text().includes('on thing'),
  );
  return Object.assign(page, { sockets });
}

describe('the home assistant settings page', () => {
  test('connects with what is typed even while a field still has focus', async () => {
    const held = device();
    const page = await open(held);

    page.enter('#bt-field-base_url', 'http://ha.local:8123');
    page.enter('#bt-field-token', 'long-lived');
    page.press('connect');
    await page.until(() => page.text().includes('Kitchen'));

    expect(held.events.filter(event => event.startsWith('set '))).toEqual([
      'set base_url=http://ha.local:8123',
      'set token=long-lived',
    ]);
    expect(page.sockets.map(socket => socket.url)).toEqual(['ws://ha.local:8123/api/websocket']);
    expect(page.sockets[0]!.sent[0]).toEqual({ type: 'auth', access_token: 'long-lived' });
    expect(page.text()).toContain('Fan');

    await page.close();
  });

  test('rereads the saved selection when the host shows the page again', async () => {
    const held = device();
    held.doc.set(SELECTION, 'light.kitchen');
    const page = await open(held);
    expect(page.text()).toContain('1 selected');

    held.doc.set(SELECTION, 'light.kitchen,switch.fan');
    page.session.notify({
      event: 'refresh',
      context: { webappId: 'ha', name: 'Home Assistant', version: '0.9.1', deviceId: 'thing' },
    });
    await page.until(() => page.text().includes('2 selected'));

    expect(held.events.filter(event => event === `read ${SELECTION}`)).toHaveLength(2);
    expect(page.text()).toContain('2 selected');

    await page.close();
  });

  test('follows the saved selection when the device changes it, and ignores other keys', async () => {
    const held = device();
    const page = await open(held);
    expect(page.text()).toContain('0 selected');

    page.session.notify({ event: 'docChanged', key: 'layout', value: 'a,b,c,d' });
    page.session.notify({ event: 'docChanged', key: SELECTION, value: 'light.kitchen,switch.fan,climate.hall' });
    await page.until(() => page.text().includes('3 selected'));

    expect(page.text()).toContain('3 selected');

    await page.close();
  });

  test('done writes what is still being typed before it closes', async () => {
    const held = device();
    const page = await open(held);

    page.enter('#bt-field-token', 'long-lived');
    page.press('Done');
    await page.until(() => held.events.includes('done'));

    expect(held.events.slice(-2)).toEqual(['set token=long-lived', 'done']);

    await page.close();
  });
});
