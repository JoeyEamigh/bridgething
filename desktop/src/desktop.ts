import type * as api from '@bridgething/companion-types';
import { isCompanion, useSession, type CompanionSession } from '@bridgething/ui';

import type { InstallOutcome, OtaOutcome } from './tauri-session.ts';

export type KnownDevice = { id: string; url: string; name: string; lastConnectedAt: string | null };

export type ExtensionStatus =
  | { kind: 'starting' }
  | { kind: 'running' }
  | { kind: 'crashed'; reason: string }
  | { kind: 'stopped' }
  | { kind: 'runtime-missing'; reason: string }
  | { kind: 'refused'; reason: string };

export type BundleExtension = { permissions: string[]; api: number };

export type ExtensionEntry = {
  id: string;
  name: string;
  version: string;
  permissions: string[];
  api: number;
  enabled: boolean;
  dataDir: string;
  status: ExtensionStatus;
  orphaned: boolean;
};

export type SelfUpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current'; checkedAt: string }
  | { kind: 'downloading'; version: string; received: number; total: number | null }
  | { kind: 'ready'; version: string; date: string | null; notes: string | null }
  | { kind: 'installing'; version: string }
  | { kind: 'unavailable'; reason: string };

export interface DesktopSession extends CompanionSession {
  readonly host: 'desktop';

  capabilitySupport(): Promise<api.CapabilityFlags>;
  defaultGateway(): Promise<string>;

  knownDevices(): Promise<KnownDevice[]>;
  forgetKnownDevice(id: string): Promise<void>;

  selectedDevice(): Promise<string | null>;
  selectDevice(deviceId: string | null): Promise<void>;

  route(): Promise<string>;
  setRoute(path: string): Promise<void>;

  catalogSources(): Promise<string[]>;
  addCatalogSource(url: string): Promise<string[]>;
  removeCatalogSource(url: string): Promise<string[]>;

  otaRunProgress(deviceId: string, lane: api.OtaLane): Promise<api.OtaRunProgress | null>;

  deviceAutoResume(): Promise<boolean>;
  deviceLogStreaming(): Promise<boolean>;

  deviceResumeTarget(): Promise<api.ResumeTarget>;

  debugLogging(): Promise<boolean>;
  setDebugLogging(enabled: boolean): Promise<void>;

  exportLogs(path: string, body: string): Promise<void>;

  otaPushDaemon(artifact: string): Promise<OtaOutcome>;
  otaInstallWebapp(bundle: string, provenance?: string, confirmed?: string[]): Promise<InstallOutcome>;
  webappBundleExtension(bundle: string): Promise<BundleExtension | null>;
  mediaImportTicket(): Promise<number>;
  importPickedMedia(
    ticket: number,
    path: string,
    kind: api.MediaPickKind,
    cover: api.MediaCover | null,
  ): Promise<api.PickedMedia>;
  cancelMediaImport(ticket: number): Promise<void>;

  installWebappFromUrl(
    url: string,
    provenance?: string,
    expected?: api.ArtifactDigest | null,
    confirmed?: string[],
    webapp?: { id: string; name: string; version: string },
  ): Promise<api.WebappInfo>;

  extensions(): Promise<ExtensionEntry[]>;
  setExtensionEnabled(id: string, enabled: boolean): Promise<void>;
  removeExtension(id: string): Promise<void>;
  openExtensionData(id: string): Promise<void>;
  retryExtensionRuntime(): Promise<void>;

  selfUpdate(): Promise<SelfUpdateState>;
  checkSelfUpdate(): Promise<void>;
  applySelfUpdate(): Promise<void>;

  setDeviceResumeTarget(target: api.ResumeTarget): Promise<void>;
}

export function isDesktop(session: CompanionSession): session is DesktopSession {
  return (session as Partial<DesktopSession>).host === 'desktop';
}

export function useDesktop(): DesktopSession {
  const session = useSession();
  if (!isCompanion(session)) throw new Error(`the shell needs a companion backend, not a ${session.tier} one`);
  if (!isDesktop(session)) throw new Error('the shell is mounted over a backend that is not the desktop one');
  return session;
}
