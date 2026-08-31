import type {
  ActiveWebapp,
  ArtifactDigest,
  CapabilityFlags,
  ConfigEntry,
  DeviceLogLine,
  DeviceMetaEntry,
  DocEntry,
  NowPlaying,
  OtaAvailable,
  OtaDiscoverManifest,
  OtaPollConfig,
  OtaPollStatus,
  OtaRun,
  ProviderInfo,
  ProviderTokens,
  SessionHostInfo,
  SessionPeer,
  SessionSnapshot,
  VoiceModelState,
  WebappInfo,
  WebappResourceKind,
  WebappSlot,
  WebappSlots,
} from '@bridgething/companion-types';

export type Tier = 'manager' | 'companion';

export type Topic =
  | 'session'
  | 'endpoints'
  | 'providers'
  | 'peers'
  | 'now-playing'
  | 'ancs'
  | 'device-meta'
  | 'webapps'
  | 'webapp-doc'
  | 'voice-model'
  | 'ota-runs'
  | 'ota-available'
  | 'ota-poll'
  | 'logs'
  | 'known-devices'
  | 'extensions';

export type Invalidation = { topic: Topic; id: string | null };

export type Endpoint = {
  id: string;
  url: string;
  host: string;
  nickname: string | null;
};

export type WebappResource = { digest: string; mime: string | null; bytes: number[] };

export type ResourceOrigin = { url: string; sha256: string; size: number; mime: string | null };

export interface DeviceSession {
  readonly tier: Tier;

  subscribe(listener: (event: Invalidation) => void): () => void;

  endpoints(): Promise<Endpoint[]>;
  connect(url?: string): Promise<string>;
  disconnect(): Promise<void>;

  snapshot(): Promise<SessionSnapshot>;
  hostInfo(): Promise<SessionHostInfo>;
  peers(): Promise<SessionPeer[]>;
  deviceMeta(): Promise<DeviceMetaEntry[]>;
  capabilities(): Promise<CapabilityFlags>;
  setCapabilityFlags(flags: CapabilityFlags): Promise<void>;
  setDeviceNickname(nickname: string): Promise<void>;
  setDeviceAutoResume(enabled: boolean): Promise<void>;

  webapps(): Promise<WebappInfo[]>;
  webappActive(): Promise<ActiveWebapp | null>;
  webappSlots(): Promise<WebappSlots>;
  setWebappSlot(slot: WebappSlot, id: string | null): Promise<WebappSlots>;
  switchWebapp(id: string): Promise<void>;
  uninstallWebapp(id: string): Promise<void>;
  installWebappFromUrl(url: string, provenance?: string, expected?: ArtifactDigest | null): Promise<WebappInfo>;
  webappResource(id: string, kind: WebappResourceKind, origin?: ResourceOrigin | null): Promise<WebappResource>;

  webappConfig(id: string): Promise<ConfigEntry[]>;
  setWebappConfigField(id: string, key: string, value: string): Promise<void>;
  deleteWebappConfigField(id: string, key: string): Promise<void>;
  webappDoc(id: string): Promise<DocEntry[]>;
  webappDocEntry(id: string, key: string): Promise<string | null>;
  setWebappDoc(id: string, key: string, value: string): Promise<void>;
  deleteWebappDoc(id: string, key: string): Promise<void>;

  voiceModel(): Promise<VoiceModelState>;

  otaRuns(): Promise<OtaRun[]>;
  otaAvailable(): Promise<OtaAvailable[]>;
  otaPoll(): Promise<OtaPollStatus>;
  otaManifest(rootUrl: string): Promise<OtaDiscoverManifest>;
  setOtaPollConfig(config: OtaPollConfig | null): Promise<void>;
  applyOtaUpdate(channel: string, version: string, rootUrl: string): Promise<void>;
  checkForOtaUpdate(rootUrl: string): Promise<void>;
  dismissOtaRun(): Promise<void>;

  deviceLogs(limit: number): Promise<DeviceLogLine[]>;
  setDeviceLogStreaming(enabled: boolean): Promise<void>;
}

export interface CompanionSession extends DeviceSession {
  readonly tier: 'companion';

  nowPlaying(): Promise<NowPlaying | null>;
  providers(): Promise<ProviderInfo[]>;
  providerPriority(): Promise<string[]>;
  setProviderPriority(ids: string[]): Promise<void>;
  libraryProvider(): Promise<string | null>;
  connectProvider(id: string): Promise<void>;
  disconnectProvider(id: string): Promise<void>;
  completeProviderAuth(id: string, tokens: ProviderTokens): Promise<void>;
  cancelProviderAuth(id: string): Promise<void>;
}

export function isCompanion(session: DeviceSession): session is CompanionSession {
  return session.tier === 'companion';
}
