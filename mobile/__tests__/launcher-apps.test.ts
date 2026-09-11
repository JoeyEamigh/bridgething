import type { AppEntry, Catalog } from '@bridgething/catalog';

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
        config: [],
        permissions: [],
      },
      {
        id: FLOW_ID,
        name: 'FlowState',
        version,
        source: 'installed' as const,
        role: 'launcher' as const,
        provenance: OFFICIAL,
        config: [],
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

function app(version: string): AppEntry {
  return {
    id: FLOW_ID.toUpperCase(),
    name: 'FlowState',
    description: 'a launcher',
    author: 'someone',
    icon: null,
    homepage: null,
    source: null,
    versions: [
      {
        version,
        released_at: '2026-05-31T00:00:00Z',
        download: {
          url: `https://example.test/r/flowstate/${version}.zip`,
          size: 4096,
          sha256: 'a'.repeat(64),
        },
        permissions: [],
        min_libbridgething_version: '0.1.0',
        changelog: null,
      },
    ],
  };
}

function serve(version: string): void {
  const catalog: Catalog = {
    schema: 'catalog.v1',
    updated_at: '2026-05-31T00:00:00Z',
    repo: {
      name: 'official',
      description: 'official',
      homepage: null,
      icon: null,
    },
    apps: [app(version)],
    recommended_sources: [],
  };
  globalThis.fetch = jest.fn((url: string) => {
    if (url !== OFFICIAL)
      return Promise.resolve({ ok: false, status: 503, json: () => ({}) });
    return Promise.resolve({ ok: true, status: 200, json: () => catalog });
  }) as unknown as typeof fetch;
}

describe('a third-party launcher and updates', () => {
  test('a newer version installs itself like any other app', async () => {
    const r = rig();
    serve('2.0.0');
    const calls: unknown[][] = [];
    r.native.__returns.set('installWebappFromUrl', (...args: unknown[]) => {
      calls.push(args);
      return Promise.resolve(null);
    });
    r.catalog.startWebappAutoUpdate();
    connect(r);

    await r.catalog.refreshCatalog();
    await tick();

    expect(calls).toHaveLength(1);
  });
});
