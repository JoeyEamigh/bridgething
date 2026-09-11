import { DEVICE, meta, peer, SERIAL, snapshot } from './fixtures';
import { rig, type Rig } from './harness';

function rendered(r: Rig, id = DEVICE) {
  const state = r.session.useSessionStore.getState();
  const live = state.peers.find(p => p.id === id);
  const known = r.session
    .knownDevices(state.ledger, state.peers)
    .find(d => d.id === id);
  return {
    peerName: live ? r.session.peerDisplayName(live, state.ledger) : null,
    knownName: known?.displayName ?? null,
    serial: known?.serialNumber ?? null,
    lastConnectedAt: known?.lastConnectedAt ?? null,
  };
}

function named(name = 'Garage'): Rig {
  const r = rig();
  r.emit('peerConnected', peer());
  r.emit('deviceMetaChanged', DEVICE, meta({ nickname: name }));
  return r;
}

describe('device identity', () => {
  test('the name the device reports is what every surface renders', () => {
    const r = named();
    expect(rendered(r)).toMatchObject({
      peerName: 'Garage',
      knownName: 'Garage',
      serial: SERIAL,
    });
  });

  test('disconnecting does not change the device identity', () => {
    const r = named();
    const before = rendered(r);

    r.emit('peerDisconnected', DEVICE);
    const after = rendered(r);

    expect(after.knownName).toBe(before.knownName);
    expect(after.serial).toBe(before.serial);
  });

  test('identity holds while a reconnect is still waiting on device meta', () => {
    const first = named();
    first.emit('peerDisconnected', DEVICE);

    const second = first.relaunch();
    second.emit('peerConnected', peer());

    expect(
      second.session.useSessionStore.getState().deviceMeta[DEVICE],
    ).toBeUndefined();
    expect(rendered(second)).toMatchObject({
      peerName: 'Garage',
      knownName: 'Garage',
    });
  });

  test('identity survives an app relaunch', () => {
    const r = named().relaunch();
    expect(rendered(r).knownName).toBe('Garage');
  });

  test('the device is authoritative: clearing the name upstream clears it here', () => {
    const r = named();
    r.emit('deviceMetaChanged', DEVICE, meta({ nickname: undefined }));
    expect(rendered(r).knownName).toBe('Car Thing');
  });

  test('reconciling a snapshot of the current world is a no-op', () => {
    jest.spyOn(Date, 'now').mockReturnValue(1_700_000_000_000);
    const r = named();
    const before = r.session.useSessionStore.getState().ledger;

    r.emit(
      'resumed',
      snapshot([peer()], { [DEVICE]: meta({ nickname: 'Garage' }) }),
    );
    const after = r.session.useSessionStore.getState().ledger;
    jest.restoreAllMocks();

    expect(after).toEqual(before);
  });

  test('a device that just disconnected reports a recent last-connected time', () => {
    const r = named();
    const connectedAt = Date.now();

    jest.spyOn(Date, 'now').mockReturnValue(connectedAt + 8 * 60 * 60 * 1000);
    r.emit('peerDisconnected', DEVICE);

    const since = Date.now() - (rendered(r).lastConnectedAt ?? 0);
    jest.restoreAllMocks();

    expect(since).toBeLessThan(60_000);
  });
});
