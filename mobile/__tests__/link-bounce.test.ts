import type { BridgethingWebappInfo } from '@bridgething/session-react-native';

import { DEVICE, OTHER, peer } from './fixtures';
import { connectedPeers, knownDevices } from '../lib/session';
import { appTiles } from '../lib/webapps';

const LEDGER = {
  [DEVICE]: {
    id: DEVICE,
    lastName: 'car thing',
    nickname: null,
    lastConnectedAt: 2_000,
    serialNumber: 'SN1',
    libVersion: 'v0.10.0',
  },
  [OTHER]: {
    id: OTHER,
    lastName: 'spare',
    nickname: null,
    lastConnectedAt: 1_000,
    serialNumber: 'SN2',
    libVersion: 'v0.10.0',
  },
};

function webapp(over: Partial<BridgethingWebappInfo>): BridgethingWebappInfo {
  return {
    id: 'ha',
    name: 'Home Assistant',
    version: '1.0.0',
    source: 'catalog',
    ...over,
  } as BridgethingWebappInfo;
}

describe('a link that bounces on connect', () => {
  test('the screen keeps pointing at the same device across the drop', () => {
    const up = knownDevices(LEDGER, [peer(DEVICE)]);
    const dropped = knownDevices(LEDGER, []);
    const back = knownDevices(LEDGER, [peer(DEVICE)]);

    expect(up[0].id).toBe(DEVICE);
    expect(dropped[0].id).toBe(DEVICE);
    expect(back[0].id).toBe(DEVICE);
  });

  test('the old connected-peer read is what went empty mid-bounce', () => {
    expect(connectedPeers([peer(DEVICE)])[0]?.id ?? null).toBe(DEVICE);
    expect(connectedPeers([])[0]?.id ?? null).toBeNull();
    expect(knownDevices(LEDGER, [])[0]?.id ?? null).toBe(DEVICE);
  });

  test('a failed link still holds the head rather than blanking the screen', () => {
    const failed = knownDevices(LEDGER, [
      peer(DEVICE, { status: 'linkFailed' }),
    ]);

    expect(failed[0].id).toBe(DEVICE);
  });

  test('a peer that is actually connected outranks a more recent ledger entry', () => {
    const other = knownDevices(LEDGER, [peer(OTHER)]);

    expect(other[0].id).toBe(OTHER);
  });
});

describe('a webapp list that arrives half filled', () => {
  test('an entry the device has not filled in is dropped, not dereferenced', () => {
    const list = [
      webapp({}),
      webapp({ id: undefined as unknown as string, name: 'half' }),
    ];

    expect(() => appTiles(list, null, [])).not.toThrow();
    expect(appTiles(list, null, []).map(t => t.id)).toEqual(['ha']);
  });
});
