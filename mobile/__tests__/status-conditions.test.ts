import type {
  BridgethingOtaRun,
  BridgethingProviderInfo,
  BridgethingSessionPeer,
  BridgethingVoiceModelState,
} from '@bridgething/session-react-native';

import {
  conditions,
  type ConditionId,
  type ConditionInput,
} from '../lib/status';

const OFFICIAL = 'https://apps.bridgething.com/catalog.json';
const THIRD_PARTY = 'https://apps.example.com/catalog.json';

const READY: BridgethingVoiceModelState = {
  status: 'ready',
  receivedBytes: 1,
  totalBytes: 1,
  version: '1',
};

function provider(over: Partial<BridgethingProviderInfo> = {}) {
  return {
    id: 'spotify',
    displayName: 'Spotify',
    available: true,
    connected: true,
    authState: { kind: 'authenticated' },
    serviceHealth: { kind: 'ok' },
    ...over,
  } as BridgethingProviderInfo;
}

function peer(over: Partial<BridgethingSessionPeer> = {}) {
  return {
    id: 'AA:BB:CC',
    name: 'Car Thing',
    status: 'connected',
    ...over,
  } as BridgethingSessionPeer;
}

function run(over: Partial<BridgethingOtaRun> = {}) {
  return {
    runId: 'r1',
    deviceId: 'AA:BB:CC',
    otaKind: 'image',
    lane: 'firmware',
    phase: 'failed',
    steps: [],
    stepId: 0,
    startedAt: 0,
    phaseStartedAt: 0,
    resumable: false,
    ...over,
  } as BridgethingOtaRun;
}

function input(over: Partial<ConditionInput> = {}): ConditionInput {
  const peers = over.peers ?? [peer()];
  return {
    reachable: true,
    providers: [provider()],
    peers,
    knownDeviceCount: peers.length,
    voiceModel: READY,
    otaRuns: [],
    catalogSources: [OFFICIAL],
    catalogFailures: [],
    ...over,
  };
}

function ids(over: Partial<ConditionInput> = {}): ConditionId[] {
  return conditions(input(over)).map(c => c.id);
}

describe('conditions', () => {
  test('a fresh install reports both unmet preconditions in priority order', () => {
    expect(ids({ providers: [], peers: [] })).toEqual([
      'notSignedIn',
      'noDevice',
    ]);
  });

  test('a healthy session reports nothing', () => {
    expect(ids()).toEqual([]);
  });

  test('a paired device that is out of range is not a missing device', () => {
    expect(ids({ peers: [], knownDeviceCount: 1 })).toEqual([]);
  });

  test('offline is exactly one condition on an otherwise healthy session', () => {
    const list = conditions(input({ reachable: false }));
    expect(list).toHaveLength(1);
    expect(list[0].id).toBe('offline');
    expect(list[0].action).toBeUndefined();
  });

  test('offline absorbs an unreachable provider so one cause reads as one condition', () => {
    const unreachable = provider({ serviceHealth: { kind: 'unreachable' } });
    expect(ids({ reachable: false, providers: [unreachable] })).toEqual([
      'offline',
    ]);
    expect(ids({ providers: [unreachable] })).toEqual(['serviceDegraded']);
  });

  test('a signed-out provider counts as not signed in', () => {
    expect(ids({ providers: [provider({ connected: false })] })).toEqual([
      'notSignedIn',
    ]);
  });

  test('an attached peer whose link failed is a link failure, not a missing device', () => {
    const list = conditions(
      input({
        peers: [peer({ status: 'linkFailed', linkError: 'rfcomm refused' })],
      }),
    );
    expect(list.map(c => c.id)).toEqual(['linkFailed']);
    expect(list[0].tone).toBe('err');
    expect(list[0].detail).toContain('rfcomm refused');
  });

  test('a failed update and a failed voice model both surface', () => {
    expect(
      ids({
        otaRuns: [run({ outcome: 'failed', error: 'flash write failed' })],
        voiceModel: {
          status: 'failed',
          receivedBytes: 0,
          totalBytes: 0,
          error: 'wifi dropped',
        },
      }),
    ).toEqual(['updateFailed', 'voiceModelFailed']);
  });

  test('a failed store install is not a failed update', () => {
    expect(
      ids({
        otaRuns: [
          run({
            otaKind: 'installedWebapp',
            lane: 'webapp',
            outcome: 'failed',
            error: 'no room',
          }),
        ],
      }),
    ).toEqual([]);
  });

  test('a succeeded update is not a condition', () => {
    expect(
      ids({ otaRuns: [run({ phase: 'completed', outcome: 'succeeded' })] }),
    ).toEqual([]);
  });

  test('a store the phone cannot read is a condition rather than a silent boot warning', () => {
    const list = conditions(
      input({
        catalogFailures: [
          { url: OFFICIAL, reason: 'catalog.json returned 500' },
        ],
      }),
    );
    expect(list.map(c => c.id)).toEqual(['storeUnavailable']);
    expect(list[0].label).toBe('store unavailable');
    expect(list[0].detail).toContain('catalog.json returned 500');
  });

  test('offline absorbs a store that could not be read so one cause reads as one condition', () => {
    expect(
      ids({
        reachable: false,
        catalogFailures: [{ url: OFFICIAL, reason: 'network request failed' }],
      }),
    ).toEqual(['offline']);
  });

  test('a failure from a source this phone no longer subscribes to is not a condition', () => {
    expect(
      ids({ catalogFailures: [{ url: THIRD_PARTY, reason: 'gone' }] }),
    ).toEqual([]);
  });

  test('one source of several failing does not claim the whole store is down', () => {
    const list = conditions(
      input({
        catalogSources: [OFFICIAL, THIRD_PARTY],
        catalogFailures: [{ url: THIRD_PARTY, reason: 'timed out' }],
      }),
    );
    expect(list.map(c => c.id)).toEqual(['storeUnavailable']);
    expect(list[0].label).toBe('a source is unavailable');
    expect(list[0].detail).toContain('1 of 2');
  });

  test('every condition carries a label and a detail', () => {
    const list = conditions(
      input({
        reachable: false,
        providers: [],
        peers: [peer({ status: 'linkFailed' })],
        voiceModel: { status: 'failed', receivedBytes: 0, totalBytes: 0 },
        otaRuns: [run({ outcome: 'failed' })],
      }),
    );
    expect(list.length).toBeGreaterThan(3);
    for (const condition of list) {
      expect(condition.label.trim()).toBeTruthy();
      expect(condition.detail.trim()).toBeTruthy();
      expect(condition.label).toBe(condition.label.toLowerCase());
    }
  });

  test('every condition id is unique in one run', () => {
    const list = conditions(
      input({ reachable: false, providers: [], peers: [] }),
    );
    expect(new Set(list.map(c => c.id)).size).toBe(list.length);
  });
});
