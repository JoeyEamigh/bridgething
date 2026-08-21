import { compareVersions } from '@bridgething/catalog';
import { describeError } from '@bridgething/ui/errors';
import { Platform } from 'react-native';
import { create } from 'zustand';

import { getSession, registerDomain } from './bridge';
import { useAppActiveInterval } from './poll';
import { DEFAULT_OTA_ROOT_URL, storage } from './storage';

const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;
const DISMISSED_KEY = 'companionUpdate.dismissed';

export type CompanionRelease = {
  version: string;
  url: string;
  size: number;
  sha256: string;
};

export type CompanionUpdatePhase =
  | { kind: 'idle' }
  | { kind: 'downloading'; received: number; total: number }
  | { kind: 'failed'; reason: string };

type CompanionUpdateState = {
  installed: string | null;
  release: CompanionRelease | null;
  dismissed: string | null;
  phase: CompanionUpdatePhase;
};

export const useCompanionUpdateStore = create<CompanionUpdateState>(() => ({
  installed: null,
  release: null,
  dismissed: storage.getString(DISMISSED_KEY) ?? null,
  phase: { kind: 'idle' },
}));

export function registerCompanionUpdateDomain(): void {
  registerDomain({
    name: 'companionUpdate',
    apply: event => {
      if (event.type !== 'companionUpdateProgress') return;
      useCompanionUpdateStore.setState({
        phase: {
          kind: 'downloading',
          received: event.received,
          total: event.total,
        },
      });
    },
    reconcile: snapshot =>
      useCompanionUpdateStore.setState({
        installed: snapshot.hostInfo.appVersion,
      }),
  });
}

export function companionUpdatesSupported(): boolean {
  return Platform.OS === 'android';
}

export function companionManifestUrl(root = DEFAULT_OTA_ROOT_URL): string {
  return `${root.replace(/\/+$/, '')}/companion.json`;
}

export function manifestHost(root: string): string {
  return root.replace(/^[a-z]+:\/\//i, '').replace(/[/:].*$/, '');
}

export function releaseFrom(
  manifest: unknown,
  platform: string,
): CompanionRelease | null {
  if (typeof manifest !== 'object' || manifest === null) return null;
  const entry = (manifest as Record<string, unknown>)[platform];
  if (typeof entry !== 'object' || entry === null) return null;
  const { version, url, size, sha256 } = entry as Record<string, unknown>;
  if (typeof version !== 'string' || version.length === 0) return null;
  if (typeof url !== 'string' || !/^https?:\/\//.test(url)) return null;
  if (typeof size !== 'number' || !Number.isFinite(size) || size <= 0)
    return null;
  if (typeof sha256 !== 'string' || !/^[0-9a-fA-F]{64}$/.test(sha256))
    return null;
  return { version, url, size, sha256: sha256.toLowerCase() };
}

export function isNewer(candidate: string, installed: string | null): boolean {
  return installed !== null && compareVersions(candidate, installed) > 0;
}

export async function checkCompanionUpdate(
  root = DEFAULT_OTA_ROOT_URL,
): Promise<CompanionRelease | null> {
  const response = await fetch(companionManifestUrl(root), {
    headers: { accept: 'application/json' },
  });
  if (!response.ok)
    throw new Error(`companion manifest: HTTP ${response.status}`);
  const release = releaseFrom(await response.json(), Platform.OS);
  const { installed } = useCompanionUpdateStore.getState();
  const next = release && isNewer(release.version, installed) ? release : null;
  useCompanionUpdateStore.setState({ release: next });
  return next;
}

export async function startCompanionUpdate(
  release: CompanionRelease,
): Promise<void> {
  useCompanionUpdateStore.setState({
    phase: { kind: 'downloading', received: 0, total: release.size },
  });
  try {
    await getSession().installCompanionUpdate(
      release.url,
      `bridgething-${release.version}.apk`,
      release.size,
      release.sha256,
    );
    useCompanionUpdateStore.setState({ phase: { kind: 'idle' } });
  } catch (err) {
    useCompanionUpdateStore.setState({
      phase: { kind: 'failed', reason: describeError(err) },
    });
  }
}

export function dismissCompanionUpdate(version: string): void {
  storage.set(DISMISSED_KEY, version);
  useCompanionUpdateStore.setState({ dismissed: version });
}

export function useCompanionUpdateCheck(root: string): void {
  const installed = useCompanionUpdateStore(s => s.installed);
  useAppActiveInterval(
    () => {
      void checkCompanionUpdate(root).catch(() => undefined);
    },
    CHECK_INTERVAL_MS,
    companionUpdatesSupported() && installed !== null,
    root,
  );
}

export function usePendingCompanionUpdate(): {
  release: CompanionRelease;
  phase: CompanionUpdatePhase;
} | null {
  const release = useCompanionUpdateStore(s => s.release);
  const dismissed = useCompanionUpdateStore(s => s.dismissed);
  const phase = useCompanionUpdateStore(s => s.phase);
  if (!release || release.version === dismissed) return null;
  return { release, phase };
}
