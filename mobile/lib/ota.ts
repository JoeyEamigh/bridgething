import type {
  BridgethingOtaAvailable,
  BridgethingOtaLane,
  BridgethingOtaPollConfig,
  BridgethingOtaPollStatus,
  BridgethingOtaProgress,
  BridgethingOtaQueuedInstall,
  BridgethingOtaQueue,
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
import { formatBytes, relativeTime } from './utils';

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
  queues: Record<string, BridgethingOtaQueue>;
};

const empty: OtaState = { poll: {}, available: {}, runs: {}, queues: {} };

export const useOtaStore = create<OtaState>(() => ({ ...empty }));

export function registerOtaDomain(): void {
  registerDomain({
    name: 'ota',
    apply: event => {
      switch (event.type) {
        case 'otaRunChanged':
          useOtaStore.setState(s => ({
            runs: { ...s.runs, [keyOf(event.run)]: event.run },
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
        case 'otaQueueChanged':
          useOtaStore.setState(s => ({
            queues: { ...s.queues, [event.queue.deviceId]: event.queue },
          }));
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
        runs: Object.fromEntries(snapshot.otaRuns.map(r => [keyOf(r), r])),
        queues: Object.fromEntries(
          snapshot.otaQueues.map(q => [q.deviceId, q]),
        ),
      }),
  });
}

export function runKey(deviceId: string, lane: BridgethingOtaLane): string {
  return `${deviceId}:${lane}`;
}

function keyOf(run: BridgethingOtaRun): string {
  return runKey(run.deviceId, run.lane);
}

export function runOf(
  state: Pick<OtaState, 'runs'>,
  deviceId: string,
  lane: BridgethingOtaLane,
): BridgethingOtaRun | undefined {
  return state.runs[runKey(deviceId, lane)];
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
  lane: BridgethingOtaLane,
): BridgethingOtaRun | undefined {
  return useOtaStore(s => (deviceId ? runOf(s, deviceId, lane) : undefined));
}

export function useDeviceBusy(deviceId: string | null): boolean {
  return useOtaStore(
    s =>
      deviceId != null &&
      (isRunning(runOf(s, deviceId, 'firmware')) ||
        isRunning(runOf(s, deviceId, 'webapp'))),
  );
}

const NO_QUEUED: BridgethingOtaQueuedInstall[] = [];

export function useQueuedInstall(
  deviceId: string | null,
  webappId: string,
): BridgethingOtaQueuedInstall | null {
  const installs = useOtaStore(s =>
    deviceId ? (s.queues[deviceId]?.installs ?? NO_QUEUED) : NO_QUEUED,
  );
  return installs.find(i => i.webappId === webappId) ?? null;
}

function sampleOtaProgress(
  deviceId: string,
  lane: BridgethingOtaLane,
): BridgethingOtaProgress | null {
  try {
    return getSession().otaRunProgress(deviceId, lane, Date.now()) ?? null;
  } catch {
    return null;
  }
}

export function useOtaProgress(
  deviceId: string | null,
  lane: BridgethingOtaLane,
): (BridgethingOtaProgress & { run: BridgethingOtaRun }) | null {
  const run = useOtaRun(deviceId, lane);
  const coasting =
    run !== undefined && run.outcome === undefined && run.phase === 'reboot';
  const [progress, setProgress] = useState<BridgethingOtaProgress | null>(null);

  const sample = useCallback(() => {
    setProgress(deviceId && run ? sampleOtaProgress(deviceId, lane) : null);
  }, [deviceId, lane, run]);

  useLayoutEffect(sample, [sample]);
  useAppActiveInterval(sample, NOW_TICK_MS, coasting);

  if (!run || !progress) return null;
  return { ...progress, run };
}

function formatEta(seconds: number): string {
  const s = Math.round(seconds);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  const r = s % 60;
  return r ? `${m}m ${r}s` : `${m}m`;
}

export function otaPhaseLabel(run: BridgethingOtaRun): string {
  const app = run.lane === 'webapp';
  switch (run.phase) {
    case 'downloading':
      return 'downloading';
    case 'streaming':
      return 'sending to your car thing';
    case 'verifying':
      return 'verifying';
    case 'writing':
      return app ? 'installing' : 'writing';
    case 'confirming':
      return 'confirming';
    case 'reboot':
      return run.otaKind === 'image' ? 'rebooting' : 'restarting';
    default:
      return 'starting';
  }
}

function transferring(run: BridgethingOtaRun): boolean {
  return run.phase === 'downloading' || run.phase === 'streaming';
}

export function otaRunDetail(
  run: BridgethingOtaRun,
  etaSeconds?: number,
): string | null {
  const parts: string[] = [];
  if (transferring(run) && run.stageTotal != null && run.stageTotal > 0) {
    parts.push(
      `${formatBytes(run.stageReceived ?? 0)} / ${formatBytes(run.stageTotal)}`,
    );
    if (run.ratePerSec != null && run.ratePerSec > 0)
      parts.push(`${formatBytes(run.ratePerSec)}/s`);
  }
  if (etaSeconds != null && etaSeconds >= 1)
    parts.push(`${formatEta(etaSeconds)} left`);
  return parts.length ? parts.join(' · ') : null;
}

export function dismissOtaRun(
  deviceId: string,
  lane: BridgethingOtaLane,
): void {
  void getSession()
    .dismissOtaRun(deviceId, lane)
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
        : input.error
          ? 'check failed'
          : 'up to date',
    detail: input.error
      ? describeError(input.error)
      : input.lastCheckedAt
        ? `checked ${relativeTime(input.lastCheckedAt, input.now ?? Date.now())}`
        : null,
  };
}

export function lastCheckedAt(poll: BridgethingOtaPollStatus): number | null {
  const polled = poll.lastPolledAt ? Date.parse(poll.lastPolledAt) : NaN;
  return Number.isNaN(polled) ? null : polled;
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
