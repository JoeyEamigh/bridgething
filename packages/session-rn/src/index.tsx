import type {
  BridgethingActiveWebapp,
  BridgethingAncsAuthStatus,
  BridgethingAncsSetupResult,
  BridgethingBtDevice,
  BridgethingCapabilityFlags,
  BridgethingCompanionDebug,
  BridgethingConfigEntry,
  BridgethingDeviceLogLine,
  BridgethingDeviceMeta,
  BridgethingDeviceWebappsEntry,
  BridgethingDocEntry,
  BridgethingLogArchive,
  BridgethingNowPlaying,
  BridgethingOtaAvailable,
  BridgethingOtaManifest,
  BridgethingOtaPollConfig,
  BridgethingOtaPollStatus,
  BridgethingOtaProgress,
  BridgethingOtaRun,
  BridgethingProviderInfo,
  BridgethingResourceOrigin,
  BridgethingResumeTarget,
  BridgethingSessionPeer,
  BridgethingSessionSnapshot,
  BridgethingVoiceModelState,
  BridgethingVoiceTurn,
  BridgethingWebappIcon,
  BridgethingWebappInfo,
  BridgethingWebappSlot,
  BridgethingWebappSlots,
  BridgethingSession as NativeBridgethingSession,
} from './specs/BridgethingSession.nitro';

export type {
  BridgethingActiveWebapp,
  BridgethingAncsAuthStatus,
  BridgethingAncsSetupKind,
  BridgethingAncsSetupResult,
  BridgethingAuthKind,
  BridgethingAuthState,
  BridgethingBtBondState,
  BridgethingBtDevice,
  BridgethingCapabilityFlags,
  BridgethingCompanionDebug,
  BridgethingConfigEntry,
  BridgethingConfigField,
  BridgethingDeviceAutoResume,
  BridgethingDeviceLogLine,
  BridgethingDeviceMeta,
  BridgethingDeviceMetaEntry,
  BridgethingDocEntry,
  BridgethingHostInfo,
  BridgethingLogArchive,
  BridgethingNowPlaying,
  BridgethingNowPlayingPlayback,
  BridgethingNowPlayingTrack,
  BridgethingOtaAvailable,
  BridgethingOtaChannelInfo,
  BridgethingOtaKind,
  BridgethingOtaManifest,
  BridgethingOtaOutcome,
  BridgethingOtaPhase,
  BridgethingOtaPollConfig,
  BridgethingOtaPollStatus,
  BridgethingOtaProgress,
  BridgethingOtaRelease,
  BridgethingOtaRun,
  BridgethingOtaStep,
  BridgethingOtaStepKind,
  BridgethingPeerLinkStatus,
  BridgethingProviderInfo,
  BridgethingRepeatMode,
  BridgethingResourceOrigin,
  BridgethingResumeTarget,
  BridgethingServiceHealth,
  BridgethingServiceHealthKind,
  BridgethingSessionPeer,
  BridgethingSessionSnapshot,
  BridgethingVoiceDebug,
  BridgethingVoiceModelState,
  BridgethingVoiceModelStatus,
  BridgethingVoiceTurn,
  BridgethingVoiceTurnPhase,
  BridgethingVoiceTurnTrigger,
  BridgethingWebappIcon,
  BridgethingWebappInfo,
  BridgethingWebappSlot,
  BridgethingWebappSlots,
} from './specs/BridgethingSession.nitro';

export type SessionEvent =
  | { type: 'providersChanged'; providers: BridgethingProviderInfo[] }
  | { type: 'peerConnected'; peer: BridgethingSessionPeer }
  | { type: 'peerDisconnected'; peerId: string }
  | { type: 'peerLinkFailed'; peer: BridgethingSessionPeer }
  | { type: 'nowPlayingChanged'; nowPlaying: BridgethingNowPlaying | null }
  | {
      type: 'ancsAuthStatusChanged';
      deviceId: string;
      status: BridgethingAncsAuthStatus;
    }
  | { type: 'webappsChanged'; entry: BridgethingDeviceWebappsEntry }
  | { type: 'webappDocChanged'; deviceId: string; webappId: string; key: string; value: string | null }
  | { type: 'deviceMetaChanged'; deviceId: string; meta: BridgethingDeviceMeta }
  | { type: 'voiceModelStateChanged'; state: BridgethingVoiceModelState }
  | { type: 'voiceTurnChanged'; turn: BridgethingVoiceTurn }
  | { type: 'otaRunChanged'; run: BridgethingOtaRun }
  | { type: 'otaAvailableChanged'; available: BridgethingOtaAvailable }
  | { type: 'otaPollChanged'; status: BridgethingOtaPollStatus }
  | { type: 'companionUpdateProgress'; received: number; total: number }
  | { type: 'resumed'; snapshot: BridgethingSessionSnapshot }
  | { type: 'log'; origin: string; level: string; message: string };

export class BridgethingSession {
  private readonly native: NativeBridgethingSession;
  private readonly listeners: Set<(event: SessionEvent) => void> = new Set();
  private logStreamingEnabled = false;
  private localLogStreamingEnabled = false;

  constructor(options: { native?: NativeBridgethingSession } = {}) {
    this.native = options.native ?? createNativeSession();
    this.wire();
  }

  subscribe(listener: (event: SessionEvent) => void): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  setLogStreamingEnabled(enabled: boolean): void {
    if (enabled === this.logStreamingEnabled) return;
    this.logStreamingEnabled = enabled;
    this.native.setLogStreamingEnabled(enabled);
  }

  setLocalLogStreamingEnabled(enabled: boolean): void {
    if (enabled === this.localLogStreamingEnabled) return;
    this.localLogStreamingEnabled = enabled;
    this.native.setLocalLogStreamingEnabled(enabled);
  }

  async start(): Promise<void> {
    await this.native.start();
  }

  async stop(): Promise<void> {
    await this.native.stop();
  }

  async availableProviders(): Promise<BridgethingProviderInfo[]> {
    return this.native.availableProviders();
  }

  async connectProvider(id: string): Promise<void> {
    await this.native.connectProvider(id);
  }

  async disconnectProvider(id: string): Promise<void> {
    await this.native.disconnectProvider(id);
  }

  async cancelAuth(id: string): Promise<void> {
    await this.native.cancelAuth(id);
  }

  async setProviderPriority(ids: string[]): Promise<void> {
    await this.native.setProviderPriority(ids);
  }

  async snapshot(): Promise<BridgethingSessionSnapshot> {
    return this.native.snapshot();
  }

  async deviceLogSnapshot(limit: number): Promise<BridgethingDeviceLogLine[]> {
    return this.native.deviceLogSnapshot(limit);
  }

  async companionDebug(): Promise<BridgethingCompanionDebug> {
    return this.native.companionDebug();
  }

  async persistedLogSize(): Promise<number> {
    return this.native.persistedLogSize();
  }

  async logArchives(): Promise<BridgethingLogArchive[]> {
    return this.native.logArchives();
  }

  async logArchiveLines(archiveId: string, limit: number): Promise<BridgethingDeviceLogLine[]> {
    return this.native.logArchiveLines(archiveId, limit);
  }

  async exportLogs(archiveId: string | null = null): Promise<string> {
    return this.native.exportLogs(archiveId);
  }

  async shareLogs(archiveId: string | null = null): Promise<boolean> {
    return this.native.shareLogs(archiveId);
  }

  async deleteLogArchive(archiveId: string): Promise<void> {
    return this.native.deleteLogArchive(archiveId);
  }

  async clearPersistedLogs(): Promise<void> {
    return this.native.clearPersistedLogs();
  }

  async enableAncsNotifications(deviceId: string): Promise<BridgethingAncsSetupResult> {
    return this.native.enableAncsNotifications(deviceId);
  }

  async ancsAuthStatus(deviceId: string): Promise<BridgethingAncsAuthStatus> {
    return this.native.ancsAuthStatus(deviceId);
  }

  async listWebapps(deviceId: string): Promise<BridgethingWebappInfo[]> {
    return this.native.listWebapps(deviceId);
  }

  async currentWebapp(deviceId: string): Promise<BridgethingActiveWebapp | null> {
    return this.native.currentWebapp(deviceId);
  }

  async installWebappFromUri(deviceId: string, sourceUri: string): Promise<BridgethingWebappInfo> {
    return this.native.installWebapp(deviceId, sourceUri);
  }

  async uninstallWebapp(deviceId: string, id: string): Promise<void> {
    await this.native.uninstallWebapp(deviceId, id);
  }

  async switchWebapp(deviceId: string, id: string): Promise<void> {
    await this.native.switchWebapp(deviceId, id);
  }

  async getWebappSlots(deviceId: string): Promise<BridgethingWebappSlots> {
    return this.native.getWebappSlots(deviceId);
  }

  async setWebappSlot(deviceId: string, slot: BridgethingWebappSlot, id?: string): Promise<BridgethingWebappSlots> {
    return this.native.setWebappSlot(deviceId, slot, id);
  }

  async webappIcon(deviceId: string, id: string): Promise<BridgethingWebappIcon | null> {
    return this.native.webappIcon(deviceId, id);
  }

  async webappSettingsMarkup(deviceId: string, id: string, origin?: BridgethingResourceOrigin): Promise<string> {
    return this.native.webappSettingsMarkup(deviceId, id, origin);
  }

  async listWebappConfig(deviceId: string, id: string): Promise<BridgethingConfigEntry[]> {
    return this.native.listWebappConfig(deviceId, id);
  }

  async setWebappConfigField(deviceId: string, id: string, key: string, value: string): Promise<void> {
    await this.native.setWebappConfigField(deviceId, id, key, value);
  }

  async deleteWebappConfigField(deviceId: string, id: string, key: string): Promise<void> {
    await this.native.deleteWebappConfigField(deviceId, id, key);
  }

  async getWebappDoc(deviceId: string, id: string, key: string): Promise<string | null> {
    return this.native.getWebappDoc(deviceId, id, key);
  }

  async listWebappDoc(deviceId: string, id: string): Promise<BridgethingDocEntry[]> {
    return this.native.listWebappDoc(deviceId, id);
  }

  async setWebappDoc(deviceId: string, id: string, key: string, value: string): Promise<void> {
    await this.native.setWebappDoc(deviceId, id, key, value);
  }

  async deleteWebappDoc(deviceId: string, id: string, key: string): Promise<void> {
    await this.native.deleteWebappDoc(deviceId, id, key);
  }

  async setCapabilityFlags(flags: BridgethingCapabilityFlags): Promise<void> {
    await this.native.setCapabilityFlags(flags);
  }

  async voiceModelState(): Promise<BridgethingVoiceModelState> {
    return this.native.voiceModelState();
  }

  async downloadVoiceModel(): Promise<void> {
    await this.native.downloadVoiceModel();
  }

  async setDeviceAutoResume(deviceId: string, enabled: boolean): Promise<void> {
    await this.native.setDeviceAutoResume(deviceId, enabled);
  }

  async isDeviceAutoResumeEnabled(deviceId: string): Promise<boolean> {
    return this.native.isDeviceAutoResumeEnabled(deviceId);
  }

  async setDeviceResumeTarget(deviceId: string, target: BridgethingResumeTarget): Promise<void> {
    await this.native.setDeviceResumeTarget(deviceId, target);
  }

  async deviceResumeTarget(deviceId: string): Promise<BridgethingResumeTarget> {
    return this.native.deviceResumeTarget(deviceId);
  }

  async setOtaPollConfig(config: BridgethingOtaPollConfig | null): Promise<void> {
    await this.native.setOtaPollConfig(config);
  }

  async checkForOtaUpdate(rootUrl: string): Promise<void> {
    await this.native.checkForOtaUpdate(rootUrl);
  }

  async fetchOtaManifest(rootUrl: string): Promise<BridgethingOtaManifest> {
    return this.native.fetchOtaManifest(rootUrl);
  }

  async applyOtaUpdate(deviceId: string, channel: string, version: string, rootUrl: string): Promise<void> {
    await this.native.applyOtaUpdate(deviceId, channel, version, rootUrl);
  }

  otaRunProgress(deviceId: string, nowMs: number): BridgethingOtaProgress | null {
    return this.native.otaRunProgress(deviceId, nowMs);
  }

  async dismissOtaRun(deviceId: string): Promise<void> {
    await this.native.dismissOtaRun(deviceId);
  }

  async installWebappFromUrl(
    deviceId: string,
    url: string,
    sha256: string,
    size: number,
    provenance: string | null = null,
    webappId: string | null = null,
    webappName: string | null = null,
  ): Promise<BridgethingWebappInfo> {
    return this.native.installWebappFromUrl(deviceId, url, sha256, size, provenance, webappId, webappName);
  }

  async reconnectPeer(deviceId: string): Promise<void> {
    await this.native.reconnectPeer(deviceId);
  }

  async deviceSetNickname(deviceId: string, nickname: string): Promise<void> {
    await this.native.deviceSetNickname(deviceId, nickname);
  }

  async presentPairPicker(): Promise<BridgethingBtDevice | null> {
    return this.native.presentPairPicker();
  }

  async isNotificationAccessGranted(): Promise<boolean> {
    return this.native.isNotificationAccessGranted();
  }

  async requestNotificationAccess(): Promise<void> {
    await this.native.requestNotificationAccess();
  }

  async isDefaultDialer(): Promise<boolean> {
    return this.native.isDefaultDialer();
  }

  async requestDefaultDialer(): Promise<void> {
    await this.native.requestDefaultDialer();
  }

  async installCompanionUpdate(url: string, filename: string, size: number, sha256: string): Promise<void> {
    await this.native.installCompanionUpdate(url, filename, size, sha256);
  }

  async forgetCompanionDevice(mac: string): Promise<void> {
    await this.native.forgetCompanionDevice(mac);
  }

  async isIgnoringBatteryOptimizations(): Promise<boolean> {
    return this.native.isIgnoringBatteryOptimizations();
  }

  async requestIgnoreBatteryOptimizations(): Promise<void> {
    await this.native.requestIgnoreBatteryOptimizations();
  }

  async revokeRuntimePermissions(permissions: string[]): Promise<boolean> {
    return this.native.revokeRuntimePermissions(permissions);
  }

  async killApp(): Promise<void> {
    await this.native.killApp();
  }

  device(deviceId: string): BridgethingDevice {
    return new BridgethingDevice(this, deviceId);
  }

  private dispatch(event: SessionEvent): void {
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch (err) {
        console.error('[bridgething] session listener threw', err);
      }
    }
  }

  private wire(): void {
    this.native.setOnProvidersChanged(providers => {
      this.dispatch({ type: 'providersChanged', providers });
    });
    this.native.setOnPeerConnected(peer => {
      this.dispatch({ type: 'peerConnected', peer });
    });
    this.native.setOnPeerDisconnected(peerId => {
      this.dispatch({ type: 'peerDisconnected', peerId });
    });
    this.native.setOnPeerLinkFailed(peer => {
      this.dispatch({ type: 'peerLinkFailed', peer });
    });
    this.native.setOnNowPlayingChanged(nowPlaying => {
      this.dispatch({ type: 'nowPlayingChanged', nowPlaying });
    });
    this.native.setOnAncsAuthStatusChanged((deviceId, status) => {
      this.dispatch({ type: 'ancsAuthStatusChanged', deviceId, status });
    });
    this.native.setOnWebappsChanged(entry => {
      this.dispatch({ type: 'webappsChanged', entry });
    });
    this.native.setOnWebappDocChanged((deviceId, webappId, key, value) => {
      this.dispatch({ type: 'webappDocChanged', deviceId, webappId, key, value: value ?? null });
    });
    this.native.setOnDeviceMetaChanged((deviceId, meta) => {
      this.dispatch({ type: 'deviceMetaChanged', deviceId, meta });
    });
    this.native.setOnVoiceModelStateChanged(state => {
      this.dispatch({ type: 'voiceModelStateChanged', state });
    });
    this.native.setOnVoiceTurnChanged(turn => {
      this.dispatch({ type: 'voiceTurnChanged', turn });
    });
    this.native.setOnOtaRunChanged(run => {
      this.dispatch({ type: 'otaRunChanged', run });
    });
    this.native.setOnOtaAvailableChanged(available => {
      this.dispatch({ type: 'otaAvailableChanged', available });
    });
    this.native.setOnOtaPollChanged(status => {
      this.dispatch({ type: 'otaPollChanged', status });
    });
    this.native.setOnCompanionUpdateProgress((received, total) => {
      this.dispatch({ type: 'companionUpdateProgress', received, total });
    });
    this.native.setOnResumed(snapshot => {
      this.dispatch({ type: 'resumed', snapshot });
    });
    this.native.setOnLog((origin, level, message) => {
      this.dispatch({ type: 'log', origin, level, message });
    });
  }
}

export class BridgethingDevice {
  constructor(
    private readonly session: BridgethingSession,
    public readonly id: string,
  ) {}

  listWebapps() {
    return this.session.listWebapps(this.id);
  }
  currentWebapp() {
    return this.session.currentWebapp(this.id);
  }
  installFromUri(sourceUri: string) {
    return this.session.installWebappFromUri(this.id, sourceUri);
  }
  uninstall(webappId: string) {
    return this.session.uninstallWebapp(this.id, webappId);
  }
  switchTo(webappId: string) {
    return this.session.switchWebapp(this.id, webappId);
  }
  icon(webappId: string) {
    return this.session.webappIcon(this.id, webappId);
  }
  listConfig(webappId: string) {
    return this.session.listWebappConfig(this.id, webappId);
  }
  setConfigField(webappId: string, key: string, value: string) {
    return this.session.setWebappConfigField(this.id, webappId, key, value);
  }
  deleteConfigField(webappId: string, key: string) {
    return this.session.deleteWebappConfigField(this.id, webappId, key);
  }
  installAppFromUrl(
    url: string,
    sha256: string,
    size: number,
    provenance: string | null,
    webappId: string | null = null,
    webappName: string | null = null,
  ) {
    return this.session.installWebappFromUrl(this.id, url, sha256, size, provenance, webappId, webappName);
  }
}

function createNativeSession(): NativeBridgethingSession {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const { NitroModules } = require('react-native-nitro-modules') as typeof import('react-native-nitro-modules');
  return NitroModules.createHybridObject<NativeBridgethingSession>('BridgethingSession');
}
