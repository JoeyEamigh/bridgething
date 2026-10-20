import { Window } from 'happy-dom';

import { openSettingsSession, type SettingsHost, type SettingsMessage } from '../src/settings-protocol.js';

export type HostRequest = { id: number; verb: string; payload?: Record<string, unknown>; epoch?: number };
export type Handler = (request: HostRequest) => unknown;

type Page = Window & {
  ReactNativeWebView?: { postMessage: (json: string) => void };
  __bridgethingSettingsDeliver?: (json: string) => void;
};

function refuse(): never {
  throw new Error('the fake host has no handler for this test');
}

export const bridge = {
  seen: [] as HostRequest[],
  handle: refuse as Handler,
  host: null as SettingsHost | null,
  hosts: {} as Record<string, SettingsHost>,
  dones: 0,
};

export function resetBridge(): void {
  bridge.seen.length = 0;
  bridge.handle = refuse;
  bridge.host = null;
  bridge.hosts = {};
  bridge.dones = 0;
}

const page = new Window({ url: 'https://settings.test/' }) as Page;

export function deliver(message: SettingsMessage): void {
  page.__bridgethingSettingsDeliver?.(JSON.stringify(message));
}

function routed(target: string): SettingsHost {
  return new Proxy({} as SettingsHost, {
    get: (_, verb) => {
      const host = bridge.hosts[target] ?? bridge.host;
      if (!host) return refuse;
      return host[verb as keyof SettingsHost];
    },
  });
}

export const session = openSettingsSession<string>({ target: 'app', bind: routed, deliver });

page.ReactNativeWebView = {
  postMessage(json: string) {
    const request = JSON.parse(json) as HostRequest;
    bridge.seen.push(request);
    if (request.verb === 'done') {
      bridge.dones += 1;
      return;
    }
    if (bridge.host || Object.keys(bridge.hosts).length > 0) {
      session.receive(json);
      return;
    }
    void Promise.resolve()
      .then(() => bridge.handle(request))
      .then(
        value => deliver({ id: request.id, ok: true, value }),
        (err: unknown) =>
          deliver({ id: request.id, ok: false, error: err instanceof Error ? err.message : String(err) }),
      );
  },
};

Object.assign(globalThis, { window: page, document: page.document, navigator: page.navigator });

export { page };
