import type * as api from '@bridgething/companion-types';
import { isCompanion, useSession, type CompanionSession } from '@bridgething/ui';

import type { InstallOutcome, OtaOutcome } from './tauri-session.ts';

export type KnownDevice = { id: string; url: string; name: string; lastConnectedAt: string | null };

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

  deviceAutoResume(): Promise<boolean>;
  deviceLogStreaming(): Promise<boolean>;

  deviceResumeTarget(): Promise<api.ResumeTarget>;

  debugLogging(): Promise<boolean>;
  setDebugLogging(enabled: boolean): Promise<void>;

  exportLogs(path: string, body: string): Promise<void>;

  otaPushDaemon(artifact: string): Promise<OtaOutcome>;
  otaInstallWebapp(bundle: string, provenance?: string): Promise<InstallOutcome>;

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
