import type {
  BridgethingOtaAvailable,
  BridgethingOtaPollConfig,
  BridgethingOtaPollStatus,
  BridgethingOtaProgress,
  BridgethingOtaRelease,
  BridgethingOtaRun,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useCallback, useLayoutEffect, useState } from 'react';
import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';

import { getSession, registerDomain } from './bridge';
import { useAppActiveInterval } from './poll';
import { DEFAULT_OTA_ROOT_URL } from './storage';
import { relativeTime } from './utils';

const NOW_TICK_MS = 500;

export function rootUrlOf(
  config: BridgethingOtaPollConfig | null | undefined,
): string {
  const held = config?.rootUrl?.trim();
  return held && held.length > 0 ? held : DEFAULT_OTA_ROOT_URL;
}

type OtaState = {
  poll: BridgethingOtaPollStatus;
  available: Record<string, BridgethingOtaAvailable>;
  runs: Record<string, BridgethingOtaRun>;
};

const empty: OtaState = { poll: {}, available: {}, runs: {} };

export const useOtaStore = create<OtaState>(() => ({ ...empty }));

export function registerOtaDomain(): void {
  registerDomain({
    name: 'ota',
    apply: event => {
      switch (event.type) {
        case 'otaRunChanged':
          useOtaStore.setState(s => ({
            runs: { ...s.runs, [event.run.deviceId]: event.run },
          }));
          return;
        case 'otaAvailableChanged':
          useOtaStore.setState(s => ({
            available: {
              ...s.available,
              [event.available.deviceId]: event.available,
            },
          }));
          return;
        case 'otaPollChanged':
          useOtaStore.setState({ poll: event.status });
          return;
        default:
          return;
      }
    },
    reconcile: snapshot =>
      useOtaStore.setState({
        poll: snapshot.otaPoll,
        available: Object.fromEntries(
          snapshot.otaAvailable.map(a => [a.deviceId, a]),
        ),
        runs: Object.fromEntries(snapshot.otaRuns.map(r => [r.deviceId, r])),
      }),
  });
}

export function isRunning(
  run: BridgethingOtaRun | undefined,
): run is BridgethingOtaRun {
  return run !== undefined && run.outcome === undefined;
}

export function useOta<T>(selector: (state: OtaState) => T): T {
  return useOtaStore(useShallow(selector));
}

export function useOtaRun(
  deviceId: string | null,
): BridgethingOtaRun | undefined {
  return useOtaStore(s => (deviceId ? s.runs[deviceId] : undefined));
}

function sampleOtaProgress(deviceId: string): BridgethingOtaProgress | null {
  try {
    return getSession().otaRunProgress(deviceId, Date.now()) ?? null;
  } catch {
    return null;
  }
}

export function useOtaProgress(
  deviceId: string | null,
): (BridgethingOtaProgress & { run: BridgethingOtaRun }) | null {
  const run = useOtaRun(deviceId);
  const coasting =
    run !== undefined && run.outcome === undefined && run.phase === 'reboot';
  const [progress, setProgress] = useState<BridgethingOtaProgress | null>(null);

  const sample = useCallback(() => {
    setProgress(deviceId && run ? sampleOtaProgress(deviceId) : null);
  }, [deviceId, run]);

  useLayoutEffect(sample, [sample]);
  useAppActiveInterval(sample, NOW_TICK_MS, coasting);

  if (!run || !progress) return null;
  return { ...progress, run };
}

export function dismissOtaRun(deviceId: string): void {
  void getSession()
    .dismissOtaRun(deviceId)
    .catch(() => {});
}

export type OtaInstallCopy = {
  title: string;
  body: string;
  detail: string;
  warning: string | null;
};

export function describeOtaInstall(
  release: BridgethingOtaRelease,
  target: string,
  deviceChannel: string | undefined,
): OtaInstallCopy {
  const crossing = deviceChannel != null && deviceChannel !== target;
  return {
    title: `install ${release.version}?`,
    body: 'your car thing will restart and be unavailable for a few minutes.',
    detail: `detail: daemon ${release.daemonVersion} · image ${release.imageVersion}`,
    warning: crossing
      ? `${release.version} is a ${target} release. your car thing is on ${deviceChannel}.`
      : null,
  };
}

export type OtaOfferInput = {
  available?: BridgethingOtaAvailable;
  lastCheckedAt: number | null;
  error?: unknown;
  now?: number;
};

export type OtaOffer = {
  version: string | null;
  value: string;
  detail: string | null;
};

export function describeOtaOffer(input: OtaOfferInput): OtaOffer {
  const version = input.available?.releaseVersion?.trim() || null;
  const offered =
    version != null ||
    input.available?.daemonVersion != null ||
    input.available?.imageVersion != null;

  return {
    version,
    value: version
      ? `${version} available`
      : offered
        ? 'update available'
        : 'up to date',
    detail: input.error
      ? describeError(input.error)
      : input.lastCheckedAt
        ? `checked ${relativeTime(input.lastCheckedAt, input.now ?? Date.now())}`
        : null,
  };
}

export function lastCheckedAt(
  poll: BridgethingOtaPollStatus,
  local: number | null,
): number | null {
  const polled = poll.lastPolledAt ? Date.parse(poll.lastPolledAt) : NaN;
  const best = Math.max(local ?? 0, Number.isNaN(polled) ? 0 : polled);
  return best > 0 ? best : null;
}

export async function installLatestOta(
  deviceId: string,
  channel: string,
  rootUrl: string,
): Promise<void> {
  const session = getSession();
  const manifest = await session.fetchOtaManifest(rootUrl);
  const found = manifest.channels.find(c => c.slug === channel);
  if (!found) throw new Error(`no ${channel} channel in the update manifest`);
  if (!found.latest)
    throw new Error(`the ${channel} channel has no release yet`);
  await session.applyOtaUpdate(deviceId, channel, found.latest, rootUrl);
}
