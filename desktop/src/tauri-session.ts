import type * as api from '@bridgething/companion-types';
import type { CompanionSession, Endpoint, Invalidation, Topic, WebappResource } from '@bridgething/ui';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

import type { KnownDevice } from './desktop.ts';

const HINTS: Record<string, Topic> = {
  'invalidate:session': 'session',
  'invalidate:endpoints': 'endpoints',
  'invalidate:providers': 'providers',
  'invalidate:peers': 'peers',
  'invalidate:now-playing': 'now-playing',
  'invalidate:ancs': 'ancs',
  'invalidate:device-meta': 'device-meta',
  'invalidate:webapps': 'webapps',
  'invalidate:webapp-doc': 'webapp-doc',
  'invalidate:voice-model': 'voice-model',
  'invalidate:ota-runs': 'ota-runs',
  'invalidate:ota-available': 'ota-available',
  'invalidate:ota-poll': 'ota-poll',
  'invalidate:logs': 'logs',
  'invalidate:known-devices': 'known-devices',
};

const RESYNC = 'invalidate:all';

export type OtaOutcome = { kind: 'completed' } | { kind: 'failed'; reason: string } | { kind: 'interrupted' };

export type InstallOutcome = { kind: 'installed'; id: string } | { kind: 'failed'; reason: string };

export class TauriSession implements CompanionSession {
  readonly tier = 'companion' as const;
  readonly host = 'desktop' as const;

  private readonly listeners = new Set<(event: Invalidation) => void>();
  private unlisten: (() => void)[] = [];

  static async start(): Promise<TauriSession> {
    const session = new TauriSession();
    await session.watch();
    return session;
  }

  private async watch(): Promise<void> {
    const stops = await Promise.all([
      ...Object.entries(HINTS).map(([event, topic]) =>
        listen<{ id: string | null }>(event, message => this.fan({ topic, id: message.payload?.id ?? null })),
      ),
      listen(RESYNC, () => {
        for (const topic of Object.values(HINTS)) this.fan({ topic, id: null });
      }),
    ]);
    this.unlisten = stops;
  }

  private fan(event: Invalidation): void {
    for (const listener of this.listeners) listener(event);
  }

  subscribe(listener: (event: Invalidation) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  stop(): void {
    for (const stop of this.unlisten) stop();
    this.unlisten = [];
    this.listeners.clear();
  }

  endpoints = () => invoke<Endpoint[]>('endpoints');
  defaultGateway = () => invoke<string>('default_gateway');
  connect = (url?: string) => invoke<string>('connect', { url: url ?? null });
  disconnect = () => invoke<void>('disconnect');

  knownDevices = () => invoke<KnownDevice[]>('known_devices');
  setDeviceAutoConnect = (url: string, enabled: boolean) => invoke<void>('set_device_auto_connect', { url, enabled });
  forgetKnownDevice = (url: string) => invoke<void>('forget_known_device', { url });

  selectedDevice = () => invoke<string | null>('selected_device');
  selectDevice = (deviceId: string | null) => invoke<void>('select_device', { deviceId });

  route = () => invoke<string>('route');
  setRoute = (path: string) => invoke<void>('set_route', { path });
  catalogSources = () => invoke<string[]>('catalog_sources');
  addCatalogSource = (url: string) => invoke<string[]>('add_catalog_source', { url });
  removeCatalogSource = (url: string) => invoke<string[]>('remove_catalog_source', { url });

  snapshot = () => invoke<api.SessionSnapshot>('session_snapshot');
  hostInfo = () => invoke<api.SessionHostInfo>('host_info');
  peers = () => invoke<api.SessionPeer[]>('peers');
  deviceMeta = () => invoke<api.DeviceMetaEntry[]>('device_meta');
  capabilities = () => invoke<api.CapabilityFlags>('capabilities');
  capabilitySupport = () => invoke<api.CapabilityFlags>('capability_support');
  setCapabilityFlags = (flags: api.CapabilityFlags) => invoke<void>('set_capability_flags', { flags });
  setDeviceNickname = (nickname: string) => invoke<void>('set_device_nickname', { nickname });
  deviceAutoResume = () => invoke<boolean>('device_auto_resume');
  setDeviceAutoResume = (enabled: boolean) => invoke<void>('set_device_auto_resume', { enabled });
  deviceResumeTarget = () => invoke<api.ResumeTarget>('device_resume_target');
  setDeviceResumeTarget = (target: api.ResumeTarget) => invoke<void>('set_device_resume_target', { target });

  webapps = () => invoke<api.WebappInfo[]>('webapps');
  webappActive = () => invoke<api.ActiveWebapp | null>('webapp_active');
  webappSlots = () => invoke<api.WebappSlots>('webapp_slots');
  setWebappSlot = (slot: api.WebappSlot, id: string | null) => invoke<api.WebappSlots>('set_webapp_slot', { slot, id });
  switchWebapp = (id: string) => invoke<void>('switch_webapp', { id });
  uninstallWebapp = (id: string) => invoke<void>('uninstall_webapp', { id });
  installWebappFromUrl = (url: string, provenance?: string, expected?: api.ArtifactDigest | null) =>
    invoke<api.WebappInfo>('install_webapp_from_url', {
      url,
      expected: expected ?? null,
      provenance: provenance ?? null,
    });
  webappResource = (id: string, kind: api.WebappResourceKind) =>
    invoke<WebappResource>('webapp_resource', { id, kind });

  webappConfig = (id: string) => invoke<api.ConfigEntry[]>('webapp_config', { id });
  setWebappConfigField = (id: string, key: string, value: string) =>
    invoke<void>('set_webapp_config_field', { id, key, value });
  deleteWebappConfigField = (id: string, key: string) => invoke<void>('delete_webapp_config_field', { id, key });
  webappDoc = (id: string) => invoke<api.DocEntry[]>('webapp_doc', { id });
  webappDocEntry = (id: string, key: string) => invoke<string | null>('webapp_doc_entry', { id, key });
  setWebappDoc = (id: string, key: string, value: string) => invoke<void>('set_webapp_doc', { id, key, value });
  deleteWebappDoc = (id: string, key: string) => invoke<void>('delete_webapp_doc', { id, key });

  voiceModel = () => invoke<api.VoiceModelState>('voice_model');

  otaRuns = () => invoke<api.OtaRun[]>('ota_runs');
  otaAvailable = () => invoke<api.OtaAvailable[]>('ota_available');
  otaPoll = () => invoke<api.OtaPollStatus>('ota_poll');
  otaManifest = (rootUrl: string) => invoke<api.OtaDiscoverManifest>('ota_manifest', { rootUrl });
  setOtaPollConfig = (config: api.OtaPollConfig | null) => invoke<void>('set_ota_poll_config', { config });
  applyOtaUpdate = (channel: string, version: string, rootUrl: string) =>
    invoke<void>('apply_ota_update', { channel, version, rootUrl });
  checkForOtaUpdate = (rootUrl: string) => invoke<void>('ota_check_now', { rootUrl });
  dismissOtaRun = () => invoke<void>('ota_dismiss_run');

  otaPushDaemon = (artifact: string) => invoke<OtaOutcome>('ota_push_daemon', { artifact });
  otaInstallWebapp = (bundle: string, provenance?: string) =>
    invoke<InstallOutcome>('ota_install_webapp', { bundle, provenance: provenance ?? null });

  deviceLogs = (limit: number) => invoke<api.DeviceLogLine[]>('device_logs', { limit });
  deviceLogStreaming = () => invoke<boolean>('device_log_streaming');
  setDeviceLogStreaming = (enabled: boolean) => invoke<void>('set_device_log_streaming', { enabled });
  debugLogging = () => invoke<boolean>('debug_logging');
  setDebugLogging = (enabled: boolean) => invoke<void>('set_debug_logging', { enabled });
  exportLogs = (path: string, body: string) => invoke<void>('export_logs', { path, body });

  nowPlaying = () => invoke<api.NowPlaying | null>('now_playing');
  providers = () => invoke<api.ProviderInfo[]>('providers');
  providerPriority = () => invoke<string[]>('provider_priority');
  libraryProvider = () => invoke<string | null>('library_provider');
  setProviderPriority = (ids: string[]) => invoke<void>('set_provider_priority', { ids });
  connectProvider = (id: string) => invoke<void>('connect_provider', { id });
  disconnectProvider = (id: string) => invoke<void>('disconnect_provider', { id });
  completeProviderAuth = (id: string, tokens: api.ProviderTokens) =>
    invoke<void>('complete_provider_auth', { id, tokens });
  cancelProviderAuth = (id: string) => invoke<void>('cancel_provider_auth', { id });
}
