import type {
  BridgethingWebappInfo,
  SessionEvent,
} from '@bridgething/session-react-native';

import { DEVICE, peer, snapshot } from './fixtures';
import { rig, type Rig } from './harness';

const ROOT = 'https://ota.example';

function manifest(channels: Array<{ slug: string; latest: string }>) {
  return {
    updatedAt: '2026-07-01T00:00:00Z',
    channels: channels.map(c => ({
      slug: c.slug,
      name: c.slug,
      stability: 'stable',
      isDefault: c.slug === 'stable',
      latest: c.latest,
      releases: [],
    })),
  };
}

function applied(r: Rig): unknown[][] {
  const calls: unknown[][] = [];
  r.native.__returns.set('applyOtaUpdate', (...args: unknown[]) => {
    calls.push(args);
    return Promise.resolve();
  });
  return calls;
}

describe('installing the latest update', () => {
  test('pushes the channel latest to the device', async () => {
    const r = rig();
    r.native.__returns.set(
      'fetchOtaManifest',
      manifest([{ slug: 'stable', latest: '0.7.0+image.2026.7.1' }]),
    );
    const calls = applied(r);

    await r.ota.installLatestOta(DEVICE, 'stable', ROOT);

    expect(calls).toEqual([[DEVICE, 'stable', '0.7.0+image.2026.7.1', ROOT]]);
  });

  test('says so when the channel is not in the manifest', async () => {
    const r = rig();
    r.native.__returns.set(
      'fetchOtaManifest',
      manifest([{ slug: 'stable', latest: '0.7.0+image.2026.7.1' }]),
    );
    const calls = applied(r);

    await expect(r.ota.installLatestOta(DEVICE, 'dev', ROOT)).rejects.toThrow(
      /dev/,
    );
    expect(calls).toEqual([]);
  });

  test('says so when the channel has no release yet', async () => {
    const r = rig();
    r.native.__returns.set(
      'fetchOtaManifest',
      manifest([{ slug: 'dev', latest: '' }]),
    );
    const calls = applied(r);

    await expect(r.ota.installLatestOta(DEVICE, 'dev', ROOT)).rejects.toThrow();
    expect(calls).toEqual([]);
  });
});

describe('event fan-out', () => {
  test('a domain registered twice under one name only applies once', () => {
    const r = rig();
    const seen: SessionEvent[] = [];
    const domain = {
      name: 'counter',
      apply: (e: SessionEvent) => seen.push(e),
      reconcile: () => {},
    };
    r.bridge.registerDomain(domain);
    r.bridge.registerDomain({ ...domain });

    r.emit('peerConnected', peer());

    expect(seen).toHaveLength(1);
  });

  test('starting the bridge twice does not double every event', () => {
    const r = rig();
    const seen: SessionEvent[] = [];
    r.bridge.registerDomain({
      name: 'counter',
      apply: (e: SessionEvent) => seen.push(e),
      reconcile: () => {},
    });
    r.bridge.startBridge();

    r.emit('peerConnected', peer());

    expect(seen).toHaveLength(1);
    expect(r.session.useSessionStore.getState().peers).toHaveLength(1);
  });

  test('the resync a peer connect triggers keeps that peer', async () => {
    const r = rig();
    r.emit('peerConnected', peer());
    await new Promise<void>(resolve => setTimeout(() => resolve(), 0));

    expect(r.native.__calls).toContain('snapshot');
    expect(r.session.useSessionStore.getState().peers).toEqual([peer()]);
  });

  test('a resume is reconciled, not replayed as an event', () => {
    const r = rig();
    const events: SessionEvent[] = [];
    const snapshots: unknown[] = [];
    r.bridge.registerDomain({
      name: 'counter',
      apply: (e: SessionEvent) => events.push(e),
      reconcile: s => snapshots.push(s),
    });

    r.emit('resumed', snapshot([]));

    expect(events).toHaveLength(0);
    expect(snapshots).toHaveLength(1);
  });

  test('a domain that throws does not stop the others from updating', () => {
    const r = rig();
    r.bridge.registerDomain({
      name: 'exploder',
      apply: () => {
        throw new Error('cannot parse this frame');
      },
      reconcile: () => {
        throw new Error('cannot parse this snapshot');
      },
    });
    const reached: string[] = [];
    r.bridge.registerDomain({
      name: 'downstream',
      apply: e => reached.push(e.type),
      reconcile: () => reached.push('reconcile'),
    });

    r.emit('peerConnected', peer());
    r.emit('resumed', snapshot([peer()]));

    expect(reached).toEqual(['peerConnected', 'reconcile']);
    expect(r.session.useSessionStore.getState().ledger[DEVICE]).toBeDefined();
  });
});

function webapp(
  over: Partial<BridgethingWebappInfo> = {},
): BridgethingWebappInfo {
  return {
    id: 'com.example.app',
    name: 'example',
    source: 'installed',
    role: 'standard',
    version: '1.0.0',
    config: [],
    permissions: [],
    ...over,
  };
}

describe('the apps grid', () => {
  test('keeps the order the device reported', () => {
    const tiles = rig().webapps.appTiles(
      [webapp({ id: 'b', name: 'second' }), webapp({ id: 'a', name: 'first' })],
      null,
      [],
      null,
    );

    expect(tiles.map(t => t.id)).toEqual(['b', 'a']);
  });

  test('marks the app the car thing is showing', () => {
    const [tile] = rig().webapps.appTiles(
      [webapp()],
      'com.example.app',
      [],
      null,
    );

    expect(tile?.state).toEqual({ label: 'active', tone: 'ok' });
  });

  test('an update outranks being active, because it is the actionable one', () => {
    const [tile] = rig().webapps.appTiles(
      [webapp()],
      'com.example.app',
      ['com.example.app'],
      null,
    );

    expect(tile?.state).toEqual({ label: 'update', tone: 'accent' });
  });

  test('ids that disagree on case still match the same app', () => {
    const [tile] = rig().webapps.appTiles(
      [webapp({ id: 'Com.Example.App' })],
      'com.example.app',
      [],
      null,
    );

    expect(tile?.state).toEqual({ label: 'active', tone: 'ok' });
  });

  test('a built-in app says so once nothing more urgent applies', () => {
    const [tile] = rig().webapps.appTiles(
      [webapp({ source: 'builtin' })],
      null,
      [],
      null,
    );

    expect(tile).toMatchObject({
      builtin: true,
      state: { label: 'built-in', tone: 'neutral' },
    });
  });

  test('the built-in home screen is not listed as an app of its own', () => {
    const tiles = rig().webapps.appTiles(
      [
        webapp({ id: 'hub', source: 'builtin', role: 'launcher' }),
        webapp({ id: 'browser', source: 'builtin' }),
      ],
      null,
      [],
      null,
    );

    expect(tiles.map(t => t.id)).toEqual(['browser']);
  });

  test('a home screen the user installed themselves stays listed', () => {
    const tiles = rig().webapps.appTiles(
      [webapp({ id: 'custom', role: 'launcher' })],
      null,
      [],
      null,
    );

    expect(tiles.map(t => t.id)).toEqual(['custom']);
  });

  test('an idle installed app carries no state to explain', () => {
    const [tile] = rig().webapps.appTiles([webapp()], 'other', [], null);

    expect(tile?.state).toBeNull();
  });
});
