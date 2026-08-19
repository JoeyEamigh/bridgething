import type { HybridObject } from 'react-native-nitro-modules';

export type BridgethingPeerLinkStatus = 'connected' | 'linkFailed';

export type BridgethingResumeTarget = 'phoneOnly' | 'anySpeaker';

export type BridgethingSessionPeer = {
  id: string;
  name: string;
  status: BridgethingPeerLinkStatus;
  linkError?: string;
};

export type BridgethingAuthKind = 'idle' | 'pending' | 'authenticated' | 'failed';

export type BridgethingAuthState = {
  kind: BridgethingAuthKind;
  userCode?: string;
  verificationUrl?: string;
  verificationUrlComplete?: string;
  message?: string;
};

export type BridgethingServiceHealthKind = 'ok' | 'rateLimited' | 'unreachable';

export type BridgethingServiceHealth = {
  kind: BridgethingServiceHealthKind;
  retryAfterSeconds?: number;
};

export type BridgethingProviderInfo = {
  id: string;
  displayName: string;
  available: boolean;
  connected: boolean;
  authState: BridgethingAuthState;
  serviceHealth: BridgethingServiceHealth;
};

export type BridgethingRepeatMode = 'off' | 'one' | 'all';

export type BridgethingNowPlayingTrack = {
  id?: string;
  title?: string;
  artist?: string;
  album?: string;
  artworkUrl?: string;
  durationMs?: number;
};

export type BridgethingNowPlayingPlayback = {
  playing: boolean;
  positionMs: number;
  shuffle: boolean;
  repeatMode: BridgethingRepeatMode;
};

export type BridgethingNowPlaying = {
  track?: BridgethingNowPlayingTrack;
  playback: BridgethingNowPlayingPlayback;
  appName?: string;
};

export type BridgethingAncsAuthStatus = 'unknown' | 'probing' | 'authorized' | 'unauthorized';

export type BridgethingAncsSetupKind = 'paired' | 'alreadyPaired' | 'cancelled' | 'unsupported' | 'failed';
export type BridgethingAncsSetupResult = {
  kind: BridgethingAncsSetupKind;
  authStatus: BridgethingAncsAuthStatus;
  message?: string;
};

export type BridgethingWebappSource = 'builtin' | 'installed';

export type BridgethingWebappRole = 'standard' | 'launcher';

export type BridgethingWebappInfo = {
  id: string;
  name: string;
  source: BridgethingWebappSource;
  role: BridgethingWebappRole;
  version: string;
  provenance?: string;
  description?: string;
  iconHash?: string;
  settingsHash?: string;
  overlayHash?: string;
  config: BridgethingConfigField[];
  permissions: string[];
};

export type BridgethingActiveWebapp = {
  id: string;
  name?: string;
};

export type BridgethingWebappSlot = 'launcher' | 'overlay';

export type BridgethingWebappSlots = {
  launcher?: string;
  overlay?: string;
};

export type BridgethingConfigKind = 'string' | 'number' | 'boolean' | 'enum' | 'secret';

export type BridgethingConfigField = {
  kind: BridgethingConfigKind;
  key: string;
  label: string;
  pattern?: string;
  minLength?: number;
  maxLength?: number;
  min?: number;
  max?: number;
  step?: number;
  choices?: string[];
  defaultValue?: string;
};

export type BridgethingConfigEntry = {
  key: string;
  value: string;
};

export type BridgethingWebappIcon = {
  fileUri?: string;
  svg?: string;
  mime?: string;
};

export type BridgethingDocEntry = {
  key: string;
  value: string;
};

export type BridgethingCapabilityFlags = {
  geo: boolean;
  notifications: boolean;
  netFetch: boolean;
  netWs: boolean;
  audioTts: boolean;
  voiceModel: boolean;
};

export type BridgethingVoiceModelStatus = 'absent' | 'downloading' | 'ready' | 'failed';

export type BridgethingVoiceModelState = {
  status: BridgethingVoiceModelStatus;
  receivedBytes: number;
  totalBytes: number;
  version?: string;
  error?: string;
};

export type BridgethingVoiceTurnTrigger = 'pushToTalk' | 'assistant' | 'wakeWord';

export type BridgethingVoiceTurnPhase = 'listening' | 'resolved' | 'cancelled';

export type BridgethingVoiceTurn = {
  deviceId: string;
  streamId: string;
  trigger: BridgethingVoiceTurnTrigger;
  phase: BridgethingVoiceTurnPhase;
  transcript?: string;
  intent?: string;
};

export type BridgethingOtaPollConfig = {
  intervalSeconds: number;
  autoPush: boolean;
  rootUrl?: string;
};

export type BridgethingOtaKind = 'image' | 'daemon' | 'builtinWebapp' | 'installedWebapp' | 'wakewordModel';

export type BridgethingOtaPhase =
  | 'idle'
  | 'downloading'
  | 'streaming'
  | 'verifying'
  | 'writing'
  | 'confirming'
  | 'reboot'
  | 'completed'
  | 'failed';

export type BridgethingOtaStepKind = 'download' | 'stream' | 'apply' | 'reboot';

export type BridgethingOtaStep = {
  id: number;
  kind: BridgethingOtaStepKind;
  label: string;
  bytes: number;
};

export type BridgethingOtaOutcome = 'succeeded' | 'failed' | 'cancelled';

export type BridgethingOtaRun = {
  runId: string;
  deviceId: string;
  otaKind: BridgethingOtaKind;
  phase: BridgethingOtaPhase;
  steps: BridgethingOtaStep[];
  stepId: number;
  startedAt: number;
  phaseStartedAt: number;
  stageReceived?: number;
  stageTotal?: number;
  ratePerSec?: number;
  dwlPercent?: number;
  outcome?: BridgethingOtaOutcome;
  error?: string;
  releaseVersion?: string;
  daemonVersion?: string;
  imageVersion?: string;
  resumable: boolean;
  webappId?: string;
  webappName?: string;
};

export type BridgethingOtaProgress = {
  percent: number;
  stepIndex: number;
  stepCount: number;
  stepLabel?: string;
  etaSeconds?: number;
};

export type BridgethingOtaAvailable = {
  deviceId: string;
  releaseVersion?: string;
  daemonVersion?: string;
  imageVersion?: string;
};

export type BridgethingOtaPollStatus = {
  lastPolledAt?: string;
  error?: string;
};

export type BridgethingOtaRelease = {
  version: string;
  daemonVersion: string;
  imageVersion: string;
  yanked: boolean;
  deprecated: boolean;
};

export type BridgethingOtaChannelInfo = {
  slug: string;
  name: string;
  stability: string;
  isDefault: boolean;
  latest: string;
  releases: BridgethingOtaRelease[];
};

export type BridgethingOtaManifest = {
  updatedAt: string;
  channels: BridgethingOtaChannelInfo[];
};

export type BridgethingBtBondState = 'none' | 'bonding' | 'bonded';

export type BridgethingBtDevice = {
  address: string;
  name?: string;
  bondState: BridgethingBtBondState;
  isCarThing: boolean;
};

export type BridgethingDeviceMeta = {
  daemonVersion: string;
  libbridgethingVersion: string;
  imageVersion: string;
  appName: string;
  osName: string;
  osVersion: string;
  channel: string;
  modelName: string;
  serialNumber: string;
  nickname?: string;
};

export type BridgethingHostInfo = {
  appName: string;
  appVersion: string;
  osName: string;
  osVersion: string;
  hostIdentifier: string;
  libVersion: string;
  libbridgethingVersion: string;
  adapterVersion: string;
};

export type BridgethingDeviceMetaEntry = {
  deviceId: string;
  meta: BridgethingDeviceMeta;
};

export type BridgethingAncsAuthStatusEntry = {
  deviceId: string;
  status: BridgethingAncsAuthStatus;
};

export type BridgethingDeviceWebappsEntry = {
  deviceId: string;
  webapps: BridgethingWebappInfo[];
  active?: BridgethingActiveWebapp;
};

export type BridgethingSessionSnapshot = {
  hostInfo: BridgethingHostInfo;
  providers: BridgethingProviderInfo[];
  providerPriority: string[];
  libraryProvider?: string;
  peers: BridgethingSessionPeer[];
  ancsAuthStatuses: BridgethingAncsAuthStatusEntry[];
  nowPlaying?: BridgethingNowPlaying;
  deviceMeta: BridgethingDeviceMetaEntry[];
  capabilityFlags: BridgethingCapabilityFlags;
  voiceModel: BridgethingVoiceModelState;
  otaPollConfig?: BridgethingOtaPollConfig;
  webapps: BridgethingDeviceWebappsEntry[];
  otaRuns: BridgethingOtaRun[];
  otaAvailable: BridgethingOtaAvailable[];
  otaPoll: BridgethingOtaPollStatus;
};

export type BridgethingDeviceLogLine = {
  seq: number;
  ts: number;
  origin: string;
  level: string;
  message: string;
};

export type BridgethingLogArchive = {
  id: string;
  startedAt: number;
  bytes: number;
  pinned: boolean;
  current: boolean;
};

export type BridgethingDeviceAutoResume = {
  deviceId: string;
  enabled: boolean;
};

export type BridgethingVoiceDebug = {
  hasModel: boolean;
  armedBundle?: string;
  transferAllowed: boolean;
  nluBundleDir?: string;
  asrWeights?: string;
};

export type BridgethingCompanionDebug = {
  authorityPlaybackHeld: boolean;
  authorityMetadataHeld: boolean;
  authorityVolumeHeld: boolean;
  authorityAppBundle?: string;
  arbitratedSource?: string;
  librarySource?: string;
  lastPlayedFrom?: string;
  attachedProviders: string[];
  attachedSchemes: string[];
  linkedDevices: string[];
  autoResume: BridgethingDeviceAutoResume[];
  voice: BridgethingVoiceDebug;
};

export interface BridgethingSession extends HybridObject<{ ios: 'swift'; android: 'kotlin' }> {
  start(): Promise<void>;
  stop(): Promise<void>;

  availableProviders(): Promise<BridgethingProviderInfo[]>;
  connectProvider(id: string): Promise<void>;
  disconnectProvider(id: string): Promise<void>;
  cancelAuth(id: string): Promise<void>;
  setProviderPriority(ids: string[]): Promise<void>;

  snapshot(): Promise<BridgethingSessionSnapshot>;

  deviceLogSnapshot(limit: number): Promise<BridgethingDeviceLogLine[]>;
  companionDebug(): Promise<BridgethingCompanionDebug>;

  persistedLogSize(): Promise<number>;
  logArchives(): Promise<BridgethingLogArchive[]>;
  logArchiveLines(archiveId: string, limit: number): Promise<BridgethingDeviceLogLine[]>;
  exportLogs(archiveId: string | null): Promise<string>;
  shareLogs(archiveId: string | null): Promise<boolean>;
  deleteLogArchive(archiveId: string): Promise<void>;
  clearPersistedLogs(): Promise<void>;

  enableAncsNotifications(deviceId: string): Promise<BridgethingAncsSetupResult>;
  ancsAuthStatus(deviceId: string): Promise<BridgethingAncsAuthStatus>;

  listWebapps(deviceId: string): Promise<BridgethingWebappInfo[]>;
  currentWebapp(deviceId: string): Promise<BridgethingActiveWebapp | null>;
  installWebapp(deviceId: string, sourceUri: string): Promise<BridgethingWebappInfo>;
  uninstallWebapp(deviceId: string, id: string): Promise<void>;
  switchWebapp(deviceId: string, id: string): Promise<void>;
  getWebappSlots(deviceId: string): Promise<BridgethingWebappSlots>;
  setWebappSlot(deviceId: string, slot: BridgethingWebappSlot, id?: string): Promise<BridgethingWebappSlots>;
  webappIcon(deviceId: string, id: string): Promise<BridgethingWebappIcon | null>;
  webappSettingsPage(deviceId: string, id: string): Promise<string>;
  listWebappConfig(deviceId: string, id: string): Promise<BridgethingConfigEntry[]>;
  setWebappConfigField(deviceId: string, id: string, key: string, value: string): Promise<void>;
  deleteWebappConfigField(deviceId: string, id: string, key: string): Promise<void>;
  getWebappDoc(deviceId: string, id: string, key: string): Promise<string | null>;
  listWebappDoc(deviceId: string, id: string): Promise<BridgethingDocEntry[]>;
  setWebappDoc(deviceId: string, id: string, key: string, value: string): Promise<void>;
  deleteWebappDoc(deviceId: string, id: string, key: string): Promise<void>;

  setCapabilityFlags(flags: BridgethingCapabilityFlags): Promise<void>;

  voiceModelState(): Promise<BridgethingVoiceModelState>;
  downloadVoiceModel(): Promise<void>;

  setDeviceAutoResume(deviceId: string, enabled: boolean): Promise<void>;
  isDeviceAutoResumeEnabled(deviceId: string): Promise<boolean>;
  setDeviceResumeTarget(deviceId: string, target: BridgethingResumeTarget): Promise<void>;
  deviceResumeTarget(deviceId: string): Promise<BridgethingResumeTarget>;

  setOtaPollConfig(config: BridgethingOtaPollConfig | null): Promise<void>;
  checkForOtaUpdate(rootUrl: string): Promise<void>;
  fetchOtaManifest(rootUrl: string): Promise<BridgethingOtaManifest>;
  applyOtaUpdate(deviceId: string, channel: string, version: string, rootUrl: string): Promise<void>;
  otaRunProgress(deviceId: string, nowMs: number): BridgethingOtaProgress | null;

  installWebappFromUrl(
    deviceId: string,
    url: string,
    sha256: string,
    size: number,
    provenance: string | null,
    webappId: string | null,
    webappName: string | null,
  ): Promise<BridgethingWebappInfo>;

  reconnectPeer(deviceId: string): Promise<void>;

  deviceSetNickname(deviceId: string, nickname: string): Promise<void>;

  presentPairPicker(): Promise<BridgethingBtDevice | null>;

  isNotificationAccessGranted(): Promise<boolean>;
  requestNotificationAccess(): Promise<void>;

  isDefaultDialer(): Promise<boolean>;
  requestDefaultDialer(): Promise<void>;

  forgetCompanionDevice(mac: string): Promise<void>;

  isIgnoringBatteryOptimizations(): Promise<boolean>;
  requestIgnoreBatteryOptimizations(): Promise<void>;

  revokeRuntimePermissions(permissions: string[]): Promise<boolean>;
  killApp(): Promise<void>;

  setOnProvidersChanged(callback: (providers: BridgethingProviderInfo[]) => void): void;
  setOnPeerConnected(callback: (peer: BridgethingSessionPeer) => void): void;
  setOnPeerDisconnected(callback: (peerId: string) => void): void;
  setOnPeerLinkFailed(callback: (peer: BridgethingSessionPeer) => void): void;
  setOnNowPlayingChanged(callback: (now: BridgethingNowPlaying | null) => void): void;
  setOnAncsAuthStatusChanged(callback: (deviceId: string, status: BridgethingAncsAuthStatus) => void): void;
  setOnLog(callback: (origin: string, level: string, message: string) => void): void;
  setLogStreamingEnabled(enabled: boolean): void;
  setLocalLogStreamingEnabled(enabled: boolean): void;

  setOnWebappsChanged(callback: (entry: BridgethingDeviceWebappsEntry) => void): void;
  setOnWebappDocChanged(
    callback: (deviceId: string, webappId: string, key: string, value: string | null) => void,
  ): void;
  setOnDeviceMetaChanged(callback: (deviceId: string, meta: BridgethingDeviceMeta) => void): void;
  setOnVoiceModelStateChanged(callback: (state: BridgethingVoiceModelState) => void): void;
  setOnVoiceTurnChanged(callback: (turn: BridgethingVoiceTurn) => void): void;

  dismissOtaRun(deviceId: string): Promise<void>;

  setOnOtaRunChanged(callback: (run: BridgethingOtaRun) => void): void;
  setOnOtaAvailableChanged(callback: (available: BridgethingOtaAvailable) => void): void;
  setOnOtaPollChanged(callback: (status: BridgethingOtaPollStatus) => void): void;

  setOnResumed(callback: (snapshot: BridgethingSessionSnapshot) => void): void;
}
