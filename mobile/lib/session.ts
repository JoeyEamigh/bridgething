import {
  type BridgethingAncsAuthStatus,
  type BridgethingDeviceMeta,
  type BridgethingHostInfo,
  type BridgethingNowPlaying,
  type BridgethingProviderInfo,
  type BridgethingSessionPeer,
  type BridgethingVoiceModelState,
  type BridgethingVoiceTurn,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { Platform } from 'react-native';
import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';

import {
  getSession,
  reconcileAll,
  registerDomain,
  startBridge,
} from './bridge';
import { startDiagnostics } from './diagnostics';
import { registerCompanionUpdateDomain } from './companion-update';
import { registerOtaDomain } from './ota';
import { requestBluetoothConnect } from './permissions';
import type { Tone } from './theme';
import { registerWebappsDomain } from './webapps';
import {
  DEFAULT_CAPABILITY_FLAGS,
  DEFAULT_OTA_POLL_CONFIG,
  type DeviceLedgerEntry,
  forgetDevice as persistForget,
  getLedger,
  recordDeviceMeta,
  recordDeviceSeen,
} from './storage';

export { getSession } from './bridge';

export type SessionState = {
  started: boolean;
  reconciled: boolean;

  providers: BridgethingProviderInfo[];
  providerPriority: string[];
  libraryProvider: string | null;
  peers: BridgethingSessionPeer[];
  ancsAuthStatus: Record<string, BridgethingAncsAuthStatus>;
  nowPlaying: BridgethingNowPlaying | null;
  deviceMeta: Record<string, BridgethingDeviceMeta>;
  hostInfo: BridgethingHostInfo | null;
  ledger: Record<string, DeviceLedgerEntry>;
  capabilityFlags: typeof DEFAULT_CAPABILITY_FLAGS;
  voiceModel: BridgethingVoiceModelState;
  lastVoiceTurn: BridgethingVoiceTurn | null;
  otaPollConfig: typeof DEFAULT_OTA_POLL_CONFIG | null;
};

const VOICE_MODEL_ABSENT: BridgethingVoiceModelState = {
  status: 'absent',
  receivedBytes: 0,
  totalBytes: 0,
};

const initial: SessionState = {
  started: false,
  reconciled: false,
  providers: [],
  providerPriority: [],
  libraryProvider: null,
  peers: [],
  ancsAuthStatus: {},
  nowPlaying: null,
  deviceMeta: {},
  hostInfo: null,
  ledger: getLedger(),
  capabilityFlags: { ...DEFAULT_CAPABILITY_FLAGS },
  voiceModel: VOICE_MODEL_ABSENT,
  lastVoiceTurn: null,
  otaPollConfig: null,
};

export const useSessionStore = create<SessionState>(() => ({ ...initial }));

const set = useSessionStore.setState;

export function registerSessionDomain(): void {
  registerDomain({
    name: 'session',
    apply: event => {
      switch (event.type) {
        case 'providersChanged':
          set({ providers: event.providers });
          return;
        case 'peerConnected':
          set(s => ({
            peers: [...s.peers.filter(p => p.id !== event.peer.id), event.peer],
            ledger: recordDeviceSeen(
              event.peer.id,
              event.peer.name,
              Date.now(),
            ),
          }));
          void resyncOnReconnect();
          return;
        case 'peerLinkFailed':
          set(s => ({
            peers: [...s.peers.filter(p => p.id !== event.peer.id), event.peer],
          }));
          return;
        case 'peerDisconnected':
          set(s => ({
            peers: s.peers.filter(p => p.id !== event.peerId),
            deviceMeta: omit(s.deviceMeta, event.peerId),
            ledger: recordDeviceSeen(event.peerId, null, Date.now()),
          }));
          return;
        case 'ancsAuthStatusChanged':
          set(s => ({
            ancsAuthStatus: {
              ...s.ancsAuthStatus,
              [event.deviceId]: event.status,
            },
          }));
          return;
        case 'nowPlayingChanged':
          set({ nowPlaying: event.nowPlaying });
          return;
        case 'voiceModelStateChanged':
          set({ voiceModel: event.state });
          return;
        case 'voiceTurnChanged':
          set({ lastVoiceTurn: event.turn });
          return;
        case 'deviceMetaChanged':
          set(s => ({
            deviceMeta: { ...s.deviceMeta, [event.deviceId]: event.meta },
            ledger: recordDeviceMeta(event.deviceId, {
              serialNumber: event.meta.serialNumber ?? null,
              nickname: event.meta.nickname ?? null,
              libVersion: event.meta.libbridgethingVersion ?? null,
            }),
          }));
          return;
        case 'webappsChanged':
        case 'webappDocChanged':
        case 'otaRunChanged':
        case 'otaAvailableChanged':
        case 'otaPollChanged':
        case 'resumed':
        case 'log':
          return;
      }
    },
    reconcile: snapshot => {
      const now = Date.now();
      let ledger = getLedger();
      for (const peer of snapshot.peers) {
        if (peer.status === 'connected')
          ledger = recordDeviceSeen(peer.id, peer.name, now);
      }
      for (const entry of snapshot.deviceMeta) {
        ledger = recordDeviceMeta(entry.deviceId, {
          serialNumber: entry.meta.serialNumber ?? null,
          nickname: entry.meta.nickname ?? null,
          libVersion: entry.meta.libbridgethingVersion ?? null,
        });
      }
      set({
        providers: snapshot.providers,
        providerPriority: snapshot.providerPriority,
        libraryProvider: snapshot.libraryProvider ?? null,
        peers: snapshot.peers,
        ancsAuthStatus: Object.fromEntries(
          snapshot.ancsAuthStatuses.map(e => [e.deviceId, e.status]),
        ),
        nowPlaying: snapshot.nowPlaying ?? null,
        deviceMeta: Object.fromEntries(
          snapshot.deviceMeta.map(e => [e.deviceId, e.meta]),
        ),
        hostInfo: snapshot.hostInfo,
        capabilityFlags: snapshot.capabilityFlags,
        voiceModel: snapshot.voiceModel,
        otaPollConfig: snapshot.otaPollConfig ?? null,
        ledger,
      });
    },
  });
}

let resyncInFlight: Promise<void> | null = null;

function resyncOnReconnect(): Promise<void> {
  resyncInFlight ??= reconcileAll()
    .catch((err: unknown) => {
      console.warn('[bridgething] reconcile on peer connect failed', err);
    })
    .finally(() => {
      resyncInFlight = null;
    });
  return resyncInFlight;
}

export async function bootstrapSession(): Promise<void> {
  registerSessionDomain();
  registerWebappsDomain();
  registerOtaDomain();
  registerCompanionUpdateDomain();
  startBridge();
  if (useSessionStore.getState().started) return;
  await getSession().start();
  useSessionStore.setState({ started: true });
  try {
    await reconcileAll();
  } catch (err) {
    console.warn('[bridgething] initial reconcile failed', err);
  }
  useSessionStore.setState({ reconciled: true });
  await startDiagnostics();
}

export async function updateCapabilityFlags(
  flags: typeof DEFAULT_CAPABILITY_FLAGS,
): Promise<void> {
  useSessionStore.setState({ capabilityFlags: flags });
  await getSession().setCapabilityFlags(flags);
}

export async function downloadVoiceModel(): Promise<void> {
  await getSession().downloadVoiceModel();
}

export async function updateOtaPollConfig(
  config: typeof DEFAULT_OTA_POLL_CONFIG | null,
): Promise<void> {
  useSessionStore.setState({ otaPollConfig: config });
  await getSession().setOtaPollConfig(config);
}

export async function patchOtaPollConfig(
  partial: Partial<typeof DEFAULT_OTA_POLL_CONFIG>,
): Promise<void> {
  const held = useSessionStore.getState().otaPollConfig;
  await updateOtaPollConfig({
    intervalSeconds:
      partial.intervalSeconds ??
      held?.intervalSeconds ??
      DEFAULT_OTA_POLL_CONFIG.intervalSeconds,
    autoPush:
      partial.autoPush ?? held?.autoPush ?? DEFAULT_OTA_POLL_CONFIG.autoPush,
    rootUrl: 'rootUrl' in partial ? partial.rootUrl : held?.rootUrl,
  });
}

export async function setDeviceName(
  deviceId: string,
  name: string | null,
): Promise<void> {
  await getSession().deviceSetNickname(deviceId, name ?? '');
}

export function forgetKnownDevice(deviceId: string): void {
  void getSession()
    .forgetCompanionDevice(deviceId)
    .catch(() => {});
  useSessionStore.setState({ ledger: persistForget(deviceId) });
}

export type PairAction = { kind: 'openSettings'; label: string };

export type PairNotice = {
  tone: Tone;
  title: string;
  body: string;
  action?: PairAction;
};

export type PairPickerResult = { picked: boolean; notice: PairNotice | null };

export function describePairPickerDismissed(): PairNotice | null {
  if (Platform.OS !== 'ios') return null;
  return {
    tone: 'warn',
    title: 'pairing did not finish',
    body: 'if this car thing was paired to this phone before, forget it under settings > bluetooth, then pair again.',
    action: { kind: 'openSettings', label: 'open settings' },
  };
}

export async function presentPairWithGuidance(): Promise<PairPickerResult> {
  const picked = await getSession().presentPairPicker();
  if (picked != null) return { picked: true, notice: null };
  return { picked: false, notice: describePairPickerDismissed() };
}

export type PairOutcome =
  | { kind: 'connected' }
  | { kind: 'cancelled' }
  | { kind: 'permissionDenied' }
  | { kind: 'pairingFailed' }
  | { kind: 'timeout' }
  | { kind: 'notificationsFailed'; message?: string }
  | { kind: 'error'; message: string };

export async function runPairFlow(): Promise<PairOutcome> {
  try {
    if (Platform.OS === 'android') {
      const bt = await requestBluetoothConnect();
      if (bt !== 'granted') return { kind: 'permissionDenied' };
      const picked = await getSession().presentPairPicker();
      if (picked == null) return { kind: 'cancelled' };
      if (picked.bondState !== 'bonded') return { kind: 'pairingFailed' };
      return (await waitForPeer(45000))
        ? { kind: 'connected' }
        : { kind: 'timeout' };
    }
    if (!(await presentPairWithGuidance()).picked) return { kind: 'cancelled' };
    if (!(await waitForPeer(20000))) return { kind: 'timeout' };
    const paired = useSessionStore
      .getState()
      .peers.find(p => p.status === 'connected');
    if (paired == null) return { kind: 'timeout' };
    const ancs = await getSession().enableAncsNotifications(paired.id);
    if (ancs.kind === 'failed') {
      return {
        kind: 'notificationsFailed',
        message: ancs.message ?? undefined,
      };
    }
    return { kind: 'connected' };
  } catch (err) {
    return {
      kind: 'error',
      message: err instanceof Error ? err.message : String(err),
    };
  }
}

export function describePairOutcome(outcome: PairOutcome): PairNotice | null {
  switch (outcome.kind) {
    case 'permissionDenied':
      return {
        tone: 'warn',
        title: 'bluetooth permission needed',
        body: 'bridgething reaches your car thing over bluetooth. allow it in settings, then pair again.',
        action: { kind: 'openSettings', label: 'open settings' },
      };
    case 'pairingFailed':
      return {
        tone: 'err',
        title: 'pairing failed',
        body: 'your car thing did not finish pairing. make sure it is powered on and nearby, then try again.',
      };
    case 'timeout':
      return {
        tone: 'warn',
        title: 'not connected yet',
        body: 'pairing finished but your car thing has not connected. make sure it is powered on and nearby, then try again.',
      };
    case 'notificationsFailed':
      return {
        tone: 'warn',
        title: 'notifications not set up',
        body: outcome.message
          ? describeError(outcome.message)
          : 'pairing worked, but turning on notifications did not.',
      };
    case 'error':
      return {
        tone: 'err',
        title: 'pairing failed',
        body: describeError(outcome.message),
      };
    case 'connected':
    case 'cancelled':
      return null;
  }
}

export function waitForPeer(timeoutMs: number): Promise<boolean> {
  const isConnected = () =>
    useSessionStore.getState().peers.some(p => p.status === 'connected');
  if (isConnected()) return Promise.resolve(true);
  return new Promise(resolve => {
    let unsub: (() => void) | null = null;
    const done = (ok: boolean) => {
      unsub?.();
      unsub = null;
      clearTimeout(timer);
      resolve(ok);
    };
    const timer = setTimeout(() => done(false), timeoutMs);
    unsub = useSessionStore.subscribe(state => {
      if (state.peers.some(p => p.status === 'connected')) done(true);
    });
  });
}

export function connectedPeers(
  peers: BridgethingSessionPeer[],
): BridgethingSessionPeer[] {
  return peers.filter(p => p.status === 'connected');
}

export type VoiceIntroState = 'waiting' | 'listening' | 'heard' | 'missed';

const INERT_INTENTS = ['NO_INTENT', 'CLARIFY'];

export function voiceIntroState(
  turn: BridgethingVoiceTurn | null,
): VoiceIntroState {
  if (turn == null || turn.trigger !== 'wakeWord') return 'waiting';
  switch (turn.phase) {
    case 'listening':
      return 'listening';
    case 'cancelled':
      return 'missed';
    case 'resolved':
      return turn.intent != null && !INERT_INTENTS.includes(turn.intent)
        ? 'heard'
        : 'missed';
  }
}

export function peerDisplayName(
  peer: BridgethingSessionPeer,
  ledger: Record<string, DeviceLedgerEntry>,
): string {
  return ledger[peer.id]?.nickname ?? peer.name;
}

export type KnownDevice = {
  id: string;
  displayName: string;
  nickname: string | null;
  lastConnectedAt: number;
  serialNumber: string | null;
  peer: BridgethingSessionPeer | null;
};

export function knownDevices(
  ledger: Record<string, DeviceLedgerEntry>,
  peers: BridgethingSessionPeer[],
): KnownDevice[] {
  const byId = new Map<string, KnownDevice>();
  for (const entry of Object.values(ledger)) {
    byId.set(entry.id, {
      id: entry.id,
      displayName: entry.nickname ?? entry.lastName,
      nickname: entry.nickname,
      lastConnectedAt: entry.lastConnectedAt,
      serialNumber: entry.serialNumber,
      peer: null,
    });
  }
  for (const peer of peers) {
    const prior = byId.get(peer.id);
    byId.set(peer.id, {
      id: peer.id,
      displayName: prior?.nickname ?? peer.name,
      nickname: prior?.nickname ?? null,
      lastConnectedAt: prior?.lastConnectedAt ?? 0,
      serialNumber: prior?.serialNumber ?? null,
      peer,
    });
  }
  return [...byId.values()].sort((a, b) => {
    const aConn = a.peer?.status === 'connected' ? 1 : 0;
    const bConn = b.peer?.status === 'connected' ? 1 : 0;
    if (aConn !== bConn) return bConn - aConn;
    return b.lastConnectedAt - a.lastConnectedAt;
  });
}

export function useSession<T>(selector: (state: SessionState) => T): T {
  return useSessionStore(useShallow(selector));
}

export function usePeer(
  deviceId: string | null,
): BridgethingSessionPeer | null {
  return useSession(s =>
    deviceId ? (s.peers.find(p => p.id === deviceId) ?? null) : null,
  );
}

function omit<T extends object>(obj: T, key: keyof T | string): T {
  const next = { ...obj } as Record<string, unknown>;
  delete next[key as string];
  return next as T;
}
