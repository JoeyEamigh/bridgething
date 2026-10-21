import {
  Device,
  compositeVersion,
  fetchManifest,
  otaArtifactUrls,
  type UpdateEvent,
  type WebappDocChange,
} from '@bridgething/browser';
import type { BridgeThingMeta } from '@bridgething/lib';

export { DEFAULT_HOST, gatewayUrl } from '@bridgething/browser';
export const OTA_ROOT = 'https://ota.bridgething.com';

const CONNECT_TIMEOUT_MS = 20_000;

export type WiredSession = {
  device: Device;
  deviceId: string;
  meta: BridgeThingMeta | null;
  close(): Promise<void>;
};

export class LocalNetworkError extends Error {}

const LOCAL_NETWORK_HINT = 'check the cable, and that you allowed local network access when the browser asked.';

export async function connectWired(host: string): Promise<WiredSession> {
  const device = await reach(host);

  return {
    device,
    deviceId: device.id,
    meta: await device.meta(),
    async close() {
      await device.close().catch(() => {});
    },
  };
}

function reach(host: string): Promise<Device> {
  return new Promise<Device>((resolve, reject) => {
    const timer = setTimeout(
      () =>
        reject(
          new LocalNetworkError(`no answer from ${host} after ${CONNECT_TIMEOUT_MS / 1000}s. ${LOCAL_NETWORK_HINT}`),
        ),
      CONNECT_TIMEOUT_MS,
    );
    Device.overNetwork(host).then(
      device => {
        clearTimeout(timer);
        resolve(device);
      },
      () => {
        clearTimeout(timer);
        reject(new LocalNetworkError(`could not reach ${host}. ${LOCAL_NETWORK_HINT}`));
      },
    );
  });
}

export type UpdatePlan = {
  kind: 'image' | 'daemon';
  from: { daemon: string; image: string };
  to: { daemon: string; image: string };
  version: string;
  channel: string;
};

export async function planFor(meta: BridgeThingMeta, channel: string, version: string): Promise<UpdatePlan | null> {
  const to = await compositeVersion(version);
  if (!to) throw new Error(`could not parse ${version}`);

  const from = { daemon: meta.appVersion, image: meta.imageVersion };
  if (from.daemon === to.daemon && from.image === to.image) return null;

  return { kind: from.image === to.image ? 'daemon' : 'image', from, to, version, channel };
}

export async function resolveUpdate(
  meta: BridgeThingMeta,
  channel: string,
  root = OTA_ROOT,
): Promise<UpdatePlan | null> {
  const manifest = await fetchManifest(root);
  const target = manifest.channels[channel];
  if (!target) throw new Error(`channel ${channel} is not in the manifest`);

  const release = manifest.releases[target.latest];
  if (release?.yanked != null) throw new Error(`${target.latest} was withdrawn; not offering it`);
  if (release?.deprecated) throw new Error(`${target.latest} was deprecated; not offering it`);

  return planFor(meta, channel, target.latest);
}

async function fetchBytes(url: string, onProgress?: (received: number, total: number) => void): Promise<Uint8Array> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`fetch ${url}: ${res.status}`);
  if (!res.body || !onProgress) return new Uint8Array(await res.arrayBuffer());

  const total = Number(res.headers.get('content-length') ?? 0);
  const reader = res.body.getReader();
  const chunks: Uint8Array[] = [];
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    received += value.length;
    onProgress(received, total);
  }

  const out = new Uint8Array(received);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.byteLength;
  }
  return out;
}

export type UpdateHooks = {
  log(message: string, kind?: 'info' | 'ok' | 'warn' | 'err'): void;
  download(received: number, total: number): void;
};

export async function applyUpdate(
  device: Device,
  plan: UpdatePlan,
  hooks: UpdateHooks,
  root = OTA_ROOT,
): Promise<void> {
  const urls = await otaArtifactUrls({
    rootUrl: root,
    channel: plan.channel,
    daemonVersion: plan.to.daemon,
    imageVersion: plan.to.image,
    imageVariant: plan.channel === 'dev' ? 'dev' : 'prod',
  });

  if (plan.kind === 'daemon') {
    hooks.log(`downloading daemon ${plan.to.daemon}`);
    const binary = await fetchBytes(urls.daemonBinary, hooks.download);
    hooks.log(`pushing daemon ${plan.to.daemon}`);
    const phase = await device.push('daemon', binary);
    if (phase.kind === 'failed') throw new Error(phase.reason ?? 'daemon push failed');
    hooks.log('daemon staged; the device restarts its service', 'ok');
    return;
  }

  hooks.log(`downloading image ${plan.to.image}`);
  const swu = await fetchBytes(urls.imageSwu, hooks.download);
  const zcks = new Map([
    ['system.img.zck', await fetchBytes(urls.imageZck)],
    ['boot.vfat.zck', await fetchBytes(urls.imageBootZck)],
  ]);

  hooks.log(`pushing image ${plan.to.image}`);
  const phase = await device.pushImage(swu, zcks, root);
  if (phase.kind === 'failed') throw new Error(phase.reason ?? 'image push failed');
  hooks.log('image applied; the device reboots into the new slot', 'ok');
}

export type Feed = { stop(): void };

export function follow<T>(next: () => Promise<T | null>, on: (item: T) => void): Feed {
  let running = true;
  void (async () => {
    while (running) {
      const item = await next().catch(() => null);
      if (!item || !running) return;
      on(item);
    }
  })();
  return {
    stop() {
      running = false;
    },
  };
}

export function watchUpdateFeed(device: Device, onEvent: (event: UpdateEvent) => void): Feed {
  return follow(() => device.nextEvent(), onEvent);
}

export function watchDocFeed(device: Device, onChange: (change: WebappDocChange) => void): Feed {
  return follow(() => device.nextDocChange(), onChange);
}

export function watchProgress(device: Device, onProgress: (percent: number, phase: string) => void): Feed {
  return watchUpdateFeed(device, event => {
    if (event.kind !== 'progress' || !event.phase) return;
    const phase = event.phase;
    const done = phase.received ?? phase.sent;
    const percent =
      phase.writePercent ?? (done !== undefined && phase.total ? Math.floor((done * 100) / phase.total) : 0);
    onProgress(percent, phase.asset ? `${phase.kind} ${phase.asset}` : phase.kind);
  });
}
