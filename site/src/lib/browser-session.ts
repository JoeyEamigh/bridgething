import { Device, fetchManifest, type InstalledWebapp, type UpdateEvent } from '@bridgething/browser';
import type * as api from '@bridgething/companion-types';
import type { BridgeThingMeta, ConfigField as WireConfigField, WebappInfo as WireWebappInfo } from '@bridgething/lib';
import type { Endpoint, Invalidation, Topic, WebappResource } from '@bridgething/ui';

import type { BrowserBackend } from './browser-tier';
import { fetchBundle } from './catalog-source';
import { OTA_ROOT, applyUpdate, connectWired, planFor, resolveUpdate, watchUpdateFeed } from './wired';

export const SERIAL_URL = 'serial:';

const EVERYTHING: Topic[] = ['session', 'peers', 'device-meta', 'webapps', 'ota-available', 'ota-poll', 'ota-runs'];

const RUN_PHASE: Record<string, api.OtaRunPhase> = {
  idle: 'idle',
  downloading: 'downloading',
  streaming: 'streaming',
  applying: 'writing',
  staged: 'confirming',
  completed: 'completed',
  failed: 'failed',
};

export class BrowserSession implements BrowserBackend {
  readonly tier = 'manager' as const;
  readonly host = 'browser' as const;

  private readonly listeners = new Set<(event: Invalidation) => void>();
  private link: Device | null = null;
  private info: BridgeThingMeta | null = null;
  private feed: { stop(): void } | null = null;
  private run: api.OtaRun | null = null;
  private offered: api.OtaAvailable | null = null;
  private polled: api.OtaPollStatus = { lastPolledAt: null, error: null };
  private runs = 0;

  subscribe(listener: (event: Invalidation) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private fan(...topics: Topic[]): void {
    for (const topic of topics) for (const listener of this.listeners) listener({ topic, id: null });
  }

  private get device(): Device {
    if (!this.link) throw new Error('nothing is connected');
    return this.link;
  }

  endpoints(): Promise<Endpoint[]> {
    return Promise.resolve([]);
  }

  connect(url?: string): Promise<string> {
    if (url === undefined || url === SERIAL_URL) return this.adopt(Device.overSerial());
    return this.adopt(connectWired(url).then(session => session.device));
  }

  private async adopt(opening: Promise<Device | null>): Promise<string> {
    const device = await opening;
    if (!device) throw new Error('no device was picked');

    await this.drop();
    this.link = device;
    this.info = await device.meta();
    this.feed = watchUpdateFeed(device, event => this.absorb(event));
    void device.closed().then(() => {
      if (this.link === device) void this.disconnect();
    });

    this.fan(...EVERYTHING);
    void this.checkForOtaUpdate(OTA_ROOT).catch(() => {});
    return device.id;
  }

  private async drop(): Promise<void> {
    this.feed?.stop();
    this.feed = null;
    const going = this.link;
    this.link = null;
    this.info = null;
    this.run = null;
    this.offered = null;
    if (going) await going.close().catch(() => {});
  }

  async disconnect(): Promise<void> {
    await this.drop();
    this.fan(...EVERYTHING);
  }

  snapshot(): Promise<api.SessionSnapshot> {
    return beyondTheLink('a whole session snapshot');
  }

  hostInfo(): Promise<api.SessionHostInfo> {
    return beyondTheLink('host info');
  }

  peers(): Promise<api.SessionPeer[]> {
    if (!this.link) return Promise.resolve([]);
    return Promise.resolve([
      {
        id: this.link.id,
        name: this.info?.nickname || this.info?.modelName || 'bridgething',
        status: 'connected',
        linkError: null,
      },
    ]);
  }

  deviceMeta(): Promise<api.DeviceMetaEntry[]> {
    if (!this.link || !this.info) return Promise.resolve([]);
    return Promise.resolve([{ deviceId: this.link.id, meta: toDeviceMeta(this.info) }]);
  }

  capabilities(): Promise<api.CapabilityFlags> {
    return beyondTheLink('capability flags');
  }

  setCapabilityFlags(): Promise<void> {
    return beyondTheLink('capability flags');
  }

  async setDeviceNickname(nickname: string): Promise<void> {
    const reply = await this.device.setNickname(nickname);
    if (this.info) this.info = { ...this.info, nickname: reply.nickname };
    this.fan('device-meta', 'peers');
  }

  setDeviceAutoResume(): Promise<void> {
    return beyondTheLink('auto-resume');
  }

  async webapps(): Promise<api.WebappInfo[]> {
    const listed = await this.device.webapps();
    return listed.map(toWebappInfo);
  }

  async webappActive(): Promise<api.ActiveWebapp | null> {
    const active = await this.device.activeWebapp();
    return active.id === null ? null : { id: active.id, name: active.name };
  }

  webappSlots(): Promise<api.WebappSlots> {
    return this.device.webappSlots();
  }

  async setWebappSlot(slot: api.WebappSlot, id: string | null): Promise<api.WebappSlots> {
    const slots = await this.device.setWebappSlot(slot, id);
    this.fan('webapps');
    return slots;
  }

  async switchWebapp(id: string): Promise<void> {
    await this.device.switchWebapp(id);
    this.fan('webapps');
  }

  async uninstallWebapp(id: string): Promise<void> {
    await this.device.uninstallWebapp(id);
    this.fan('webapps');
  }

  async installWebappFromUrl(
    url: string,
    provenance?: string,
    expected?: api.ArtifactDigest | null,
  ): Promise<api.WebappInfo> {
    let bytes: Uint8Array;
    if (expected) {
      const fetched = await fetchBundle({ url, size: expected.size, sha256: expected.sha256 });
      if (!fetched.ok) throw new Error(fetched.message);
      bytes = new Uint8Array(await fetched.blob.arrayBuffer());
    } else {
      const response = await fetch(url);
      if (!response.ok) throw new Error(`${url} returned ${response.status}`);
      bytes = new Uint8Array(await response.arrayBuffer());
    }
    const installed = await this.installWebappBytes(bytes, provenance);
    const listed = await this.webapps();
    const found = listed.find(webapp => webapp.id === installed.id);
    if (!found) throw new Error(`${installed.name} installed but the device does not list it`);
    return found;
  }

  async installWebappBytes(bundle: Uint8Array, provenance?: string): Promise<InstalledWebapp> {
    const device = this.device;
    this.open('installedWebapp', { phase: 'streaming', stageTotal: bundle.byteLength });
    try {
      const installed = await device.installWebapp(bundle, provenance);
      this.settle({ webappId: installed.id, webappName: installed.name });
      this.fan('webapps');
      return installed;
    } catch (reason) {
      this.fail(reason);
      throw reason;
    }
  }

  webappResource(): Promise<WebappResource> {
    return beyondTheLink('webapp resources');
  }

  webappConfig(): Promise<api.ConfigEntry[]> {
    return beyondTheLink('webapp config');
  }

  setWebappConfigField(): Promise<void> {
    return beyondTheLink('webapp config');
  }

  deleteWebappConfigField(): Promise<void> {
    return beyondTheLink('webapp config');
  }

  webappDoc(): Promise<api.DocEntry[]> {
    return beyondTheLink('the webapp document store');
  }

  webappDocEntry(): Promise<string | null> {
    return beyondTheLink('the webapp document store');
  }

  setWebappDoc(): Promise<void> {
    return beyondTheLink('the webapp document store');
  }

  deleteWebappDoc(): Promise<void> {
    return beyondTheLink('the webapp document store');
  }

  voiceModel(): Promise<api.VoiceModelState> {
    return beyondTheLink('the voice model');
  }

  otaRuns(): Promise<api.OtaRun[]> {
    return Promise.resolve(this.run ? [this.run] : []);
  }

  otaAvailable(): Promise<api.OtaAvailable[]> {
    return Promise.resolve(this.offered ? [this.offered] : []);
  }

  otaPoll(): Promise<api.OtaPollStatus> {
    return Promise.resolve(this.polled);
  }

  otaManifest(rootUrl: string): Promise<api.OtaDiscoverManifest> {
    return fetchManifest(rootUrl);
  }

  setOtaPollConfig(): Promise<void> {
    return beyondTheLink('background update polling');
  }

  async checkForOtaUpdate(rootUrl: string): Promise<void> {
    const info = this.info;
    const link = this.link;
    if (!info || !link) return;

    try {
      const plan = await resolveUpdate(info, info.channel, rootUrl);
      this.offered = {
        deviceId: link.id,
        releaseVersion: plan ? plan.version : null,
        daemonVersion: plan ? plan.to.daemon : null,
        imageVersion: plan ? plan.to.image : null,
      };
      this.polled = { lastPolledAt: new Date().toISOString(), error: null };
    } catch (reason) {
      this.offered = null;
      this.polled = { lastPolledAt: new Date().toISOString(), error: message(reason) };
    }
    this.fan('ota-available', 'ota-poll');
  }

  async applyOtaUpdate(channel: string, version: string, rootUrl: string): Promise<void> {
    const info = this.info;
    if (!info) throw new Error('the device never announced its version');
    const device = this.device;

    const plan = await planFor(info, channel, version);
    if (!plan) return;

    this.open(plan.kind === 'image' ? 'image' : 'daemon', {
      releaseVersion: plan.version,
      daemonVersion: plan.to.daemon,
      imageVersion: plan.to.image,
    });

    try {
      await applyUpdate(
        device,
        plan,
        {
          log: () => {},
          download: (received, total) => this.stage(received, total),
        },
        rootUrl,
      );
      this.settle({});
    } catch (reason) {
      this.fail(reason);
      throw reason;
    }
  }

  dismissOtaRun(): Promise<void> {
    this.run = null;
    this.fan('ota-runs');
    return Promise.resolve();
  }

  deviceLogs(): Promise<api.DeviceLogLine[]> {
    return beyondTheLink('device logs');
  }

  setDeviceLogStreaming(): Promise<void> {
    return beyondTheLink('device logs');
  }

  private open(kind: api.OtaKind, seed: Partial<api.OtaRun>): void {
    const now = Date.now();
    this.run = {
      runId: `browser-${++this.runs}`,
      deviceId: this.link?.id ?? '',
      kind,
      phase: 'downloading',
      steps: [],
      stepId: 0,
      startedAtMs: now,
      phaseStartedAtMs: now,
      stageReceived: null,
      stageTotal: null,
      ratePerSec: null,
      dwlPercent: null,
      outcome: null,
      error: null,
      releaseVersion: null,
      daemonVersion: null,
      imageVersion: null,
      channel: null,
      rootUrl: null,
      resumable: false,
      webappId: null,
      webappName: null,
      ...seed,
    };
    this.fan('ota-runs');
  }

  private stage(received: number, total: number): void {
    if (!this.run) return;
    this.run = { ...this.run, phase: 'downloading', stageReceived: received, stageTotal: total || null };
    this.fan('ota-runs');
  }

  private settle(seed: Partial<api.OtaRun>): void {
    if (!this.run) return;
    this.run = { ...this.run, phase: 'completed', outcome: 'succeeded', ...seed };
    this.fan('ota-runs');
  }

  private fail(reason: unknown): void {
    if (!this.run) return;
    this.run = { ...this.run, phase: 'failed', outcome: 'failed', error: message(reason) };
    this.fan('ota-runs');
  }

  private absorb(event: UpdateEvent): void {
    const held = this.run;
    if (!held || held.outcome !== null) return;
    this.run = advance(held, event);
    this.fan('ota-runs');
  }
}

export function advance(run: api.OtaRun, event: UpdateEvent): api.OtaRun {
  switch (event.kind) {
    case 'planned':
      return {
        ...run,
        steps: event.steps ?? [],
        stepId: run.stepId,
        phaseStartedAtMs: Date.now(),
        channel: event.channel ?? run.channel,
        rootUrl: event.rootUrl ?? run.rootUrl,
      };
    case 'progress': {
      const phase = event.phase;
      if (!phase) return run;
      return {
        ...run,
        phase: RUN_PHASE[phase.kind] ?? run.phase,
        stepId: event.stepId ?? run.stepId,
        stageReceived: phase.received ?? phase.sent ?? null,
        stageTotal: phase.total ?? null,
        ratePerSec: phase.ratePerSec ?? null,
        dwlPercent: phase.writePercent ?? null,
      };
    }
    case 'updated':
      return { ...run, phase: 'completed', outcome: 'succeeded', error: null };
    case 'failed':
      return { ...run, phase: 'failed', outcome: 'failed', error: event.reason ?? 'the push failed' };
    default:
      return run;
  }
}

export function toDeviceMeta(meta: BridgeThingMeta): api.DeviceMeta {
  return {
    daemonVersion: meta.appVersion,
    libbridgethingVersion: meta.libbridgethingVersion,
    imageVersion: meta.imageVersion,
    appName: meta.appName,
    osName: meta.osName,
    osVersion: meta.osVersion,
    channel: meta.channel,
    modelName: meta.modelName,
    serialNumber: meta.serialNumber,
    nickname: meta.nickname,
  };
}

export function toWebappInfo(webapp: WireWebappInfo): api.WebappInfo {
  return {
    id: webapp.id,
    name: webapp.name,
    source: webapp.source,
    role: webapp.role,
    version: webapp.version,
    provenance: webapp.provenance,
    description: webapp.description,
    iconHash: webapp.iconHash,
    settingsHash: webapp.settingsHash,
    overlayHash: webapp.overlayHash,
    config: webapp.config.map(toConfigField),
    permissions: webapp.permissions,
    extension: webapp.extension ?? null,
  };
}

export function toConfigField(field: WireConfigField): api.ConfigField {
  const flat = {
    key: field.data.key,
    label: field.data.label,
    pattern: null,
    minLength: null,
    maxLength: null,
    min: null,
    max: null,
    step: null,
    choices: [] as string[],
  };

  switch (field.type) {
    case 'string':
    case 'secret':
      return {
        ...flat,
        kind: field.type,
        pattern: field.data.pattern,
        minLength: field.data.minLength,
        maxLength: field.data.maxLength,
        defaultValue: field.data.default,
      };
    case 'number':
      return {
        ...flat,
        kind: 'number',
        min: field.data.min,
        max: field.data.max,
        step: field.data.step,
        defaultValue: field.data.default === null ? null : String(field.data.default),
      };
    case 'boolean':
      return {
        ...flat,
        kind: 'boolean',
        defaultValue: field.data.default === null ? null : String(field.data.default),
      };
    case 'enum':
      return { ...flat, kind: 'enum', choices: field.data.choices, defaultValue: field.data.default };
  }
}

export function message(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

function beyondTheLink(surface: string): never {
  throw new Error(`${surface} is not reachable from a browser; the desktop app has it`);
}
