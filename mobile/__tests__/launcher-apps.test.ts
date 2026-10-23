import { DEVICE, peer } from './fixtures';
import { rig, type Rig } from './harness';

const OFFICIAL = 'https://apps.bridgething.com/catalog.json';
const FLOW_ID = '01890000-0000-7000-8000-0000000000ab';
const HUB_ID = '01890000-0000-7000-8000-0000000000cd';

function entry(version: string) {
  return {
    deviceId: DEVICE,
    webapps: [
      {
        id: HUB_ID,
        name: 'hub',
        version: '1.0.0',
        source: 'builtin' as const,
        role: 'launcher' as const,
        configJson: '[]',
        permissions: [],
      },
      {
        id: FLOW_ID,
        name: 'FlowState',
        version,
        source: 'installed' as const,
        role: 'launcher' as const,
        provenance: OFFICIAL,
        configJson: '[]',
        permissions: [],
      },
    ],
    active: undefined,
    listed: true,
  };
}

function connect(r: Rig, version = '1.0.0'): void {
  r.native.__world.webapps = [entry(version)];
  r.emit('peerConnected', peer());
  r.emit('webappsChanged', entry(version));
}

const tick = () => new Promise<void>(resolve => setTimeout(resolve, 0));

describe('a third-party launcher on the phone', () => {
  test('is listed among the apps on the device, while the built-in hub is not', () => {
    const r = rig();
    connect(r);

    const tiles = r.webapps.appTiles(
      r.webapps.installedWebapps(DEVICE),
      null,
      [],
      null,
    );

    expect(tiles.map(t => t.id)).toEqual([FLOW_ID]);
  });

  test('wears the home screen mark once it holds the slot', () => {
    const r = rig();
    connect(r);

    const tiles = r.webapps.appTiles(
      r.webapps.installedWebapps(DEVICE),
      null,
      [],
      FLOW_ID,
    );

    expect(tiles[0]!.state).toEqual({ label: 'home screen', tone: 'neutral' });
  });

  test('becomes the home screen from its own card, with no picker in the way', async () => {
    const r = rig();
    const asked: unknown[][] = [];
    r.native.__returns.set('setWebappSlot', (...args: unknown[]) => {
      asked.push(args);
      return Promise.resolve({ launcher: FLOW_ID, overlay: undefined });
    });
    connect(r);

    await r.webapps.assignSlot(DEVICE, 'launcher', FLOW_ID);

    expect(asked).toEqual([[DEVICE, 'launcher', FLOW_ID]]);
    expect(
      r.webapps.useWebappsStore.getState().slotsByDevice[DEVICE]?.slots,
    ).toEqual({ launcher: FLOW_ID, overlay: undefined });
  });

  test('the held slot is read back when the device says its apps changed', async () => {
    const r = rig();
    r.native.__returns.set('getWebappSlots', () =>
      Promise.resolve({ launcher: FLOW_ID, overlay: undefined }),
    );
    connect(r);

    await tick();

    expect(
      r.webapps.useWebappsStore.getState().slotsByDevice[DEVICE]?.slots
        ?.launcher,
    ).toBe(FLOW_ID);
  });

  test('is dropped along with the device it was read from', () => {
    const r = rig();
    connect(r);
    r.emit('peerDisconnected', DEVICE);

    expect(
      r.webapps.useWebappsStore.getState().slotsByDevice[DEVICE],
    ).toBeUndefined();
  });
});
