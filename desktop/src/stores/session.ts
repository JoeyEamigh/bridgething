import type * as api from '@bridgething/companion-types';
import type { Endpoint, Topic } from '@bridgething/ui';
import { computed, signal, type ReadonlySignal } from '@preact/signals';

import type { DesktopSession, ExtensionEntry, KnownDevice } from '../desktop.ts';
import { keyed, resource, type Store } from './resource.ts';

let attached: DesktopSession | null = null;

function bound(): DesktopSession {
  if (!attached) throw new Error('a store was read before the session was attached');
  return attached;
}

export const snapshot = resource<api.SessionSnapshot | null>(null, () => bound().snapshot());
export const endpoints = resource<Endpoint[]>([], () => bound().endpoints());
export const capabilitySupport = resource<api.CapabilityFlags | null>(null, () => bound().capabilitySupport());
export const defaultGateway = resource<string | null>(null, () => bound().defaultGateway());
export const keptRoute = resource<string | null>(null, () => bound().route());
export const catalogSources = resource<string[]>([], () => bound().catalogSources());
export const knownDevices = resource<KnownDevice[]>([], () => bound().knownDevices());
export const selectedDevice = resource<string | null>(null, () => bound().selectedDevice());
export const debugLogging = resource(false, () => bound().debugLogging());
export const extensions = resource<ExtensionEntry[]>([], () => bound().extensions());

export const webapps = resource<api.WebappInfo[]>([], () => bound().webapps());
export const webappActive = resource<api.ActiveWebapp | null>(null, () => bound().webappActive());
export const webappSlots = resource<api.WebappSlots>({ launcher: null, overlay: null }, () => bound().webappSlots());
export const autoResume = resource(true, () => bound().deviceAutoResume());
export const logStreaming = resource(false, () => bound().deviceLogStreaming());
export const resumeTarget = resource<api.ResumeTarget>('anySpeaker', () => bound().deviceResumeTarget());

export const hostInfo: ReadonlySignal<api.SessionHostInfo | null> = computed(
  () => snapshot.data.value?.hostInfo ?? null,
);
export const providers: ReadonlySignal<api.ProviderInfo[]> = computed(() => snapshot.data.value?.providers ?? []);
export const providerPriority: ReadonlySignal<string[]> = computed(() => snapshot.data.value?.providerPriority ?? []);
export const libraryProvider: ReadonlySignal<string | null> = computed(
  () => snapshot.data.value?.libraryProvider ?? null,
);
export const peers: ReadonlySignal<api.SessionPeer[]> = computed(() => snapshot.data.value?.peers ?? []);
export const deviceMeta: ReadonlySignal<api.DeviceMetaEntry[]> = computed(() => snapshot.data.value?.deviceMeta ?? []);
export const selectedMeta: ReadonlySignal<api.DeviceMeta | null> = computed(() => {
  const chosen = selectedDevice.data.value;
  if (chosen === null) return null;
  return deviceMeta.value.find(entry => entry.deviceId === chosen)?.meta ?? null;
});
export const capabilities: ReadonlySignal<api.CapabilityFlags | null> = computed(
  () => snapshot.data.value?.capabilityFlags ?? null,
);
export const voiceModel: ReadonlySignal<api.VoiceModelState | null> = computed(
  () => snapshot.data.value?.voiceModel ?? null,
);
export const otaPollConfig: ReadonlySignal<api.OtaPollConfig | null> = computed(
  () => snapshot.data.value?.otaPollConfig ?? null,
);
export const otaRuns: ReadonlySignal<api.OtaRun[]> = computed(() => snapshot.data.value?.otaRuns ?? []);
export const otaAvailable: ReadonlySignal<api.OtaAvailable[]> = computed(() => snapshot.data.value?.otaAvailable ?? []);
export const otaPoll: ReadonlySignal<api.OtaPollStatus | null> = computed(() => snapshot.data.value?.otaPoll ?? null);

const configs = keyed<api.ConfigEntry[]>([]);
const docs = keyed<api.DocEntry[]>([]);
const manifests = keyed<api.OtaDiscoverManifest | null>(null);
const logs = keyed<api.DeviceLogLine[]>([]);

export const logLimit = signal(2000);

export function webappConfigFor(id: string): Store<api.ConfigEntry[]> {
  return configs.at(id, () => bound().webappConfig(id));
}

export function webappDocFor(id: string): Store<api.DocEntry[]> {
  return docs.at(id, () => bound().webappDoc(id));
}

export function otaManifestFor(rootUrl: string): Store<api.OtaDiscoverManifest | null> {
  return manifests.at(rootUrl, () => bound().otaManifest(rootUrl));
}

export function deviceLogsFor(limit: number): Store<api.DeviceLogLine[]> {
  return logs.at(String(limit), () => bound().deviceLogs(limit));
}

type Refreshable = { refresh: () => void };

const ROUTED: Record<Topic, Refreshable[]> = {
  session: [snapshot],
  endpoints: [endpoints],
  providers: [snapshot],
  peers: [
    snapshot,
    webapps,
    webappActive,
    webappSlots,
    autoResume,
    resumeTarget,
    logStreaming,
    knownDevices,
    selectedDevice,
  ],
  'now-playing': [snapshot],
  ancs: [snapshot],
  'device-meta': [snapshot, autoResume, resumeTarget],
  webapps: [webapps, webappActive, webappSlots, { refresh: configs.refreshAll }],
  'webapp-doc': [{ refresh: docs.refreshAll }],
  'known-devices': [knownDevices],
  'voice-model': [snapshot],
  'ota-runs': [snapshot],
  'ota-available': [snapshot],
  'ota-poll': [snapshot],
  logs: [logStreaming, { refresh: logs.refreshAll }],
  extensions: [extensions],
};

export async function seed(session: DesktopSession): Promise<void> {
  attached = session;

  await Promise.all([
    snapshot.refresh(),
    keptRoute.refresh(),
    endpoints.refresh(),
    catalogSources.refresh(),
    capabilitySupport.refresh(),
    defaultGateway.refresh(),
    knownDevices.refresh(),
    selectedDevice.refresh(),
    extensions.refresh(),
  ]);

  void Promise.all([
    webapps.refresh(),
    webappActive.refresh(),
    webappSlots.refresh(),
    autoResume.refresh(),
    resumeTarget.refresh(),
    logStreaming.refresh(),
    debugLogging.refresh(),
  ]);

  session.subscribe(event => {
    for (const store of ROUTED[event.topic]) store.refresh();
  });
}
