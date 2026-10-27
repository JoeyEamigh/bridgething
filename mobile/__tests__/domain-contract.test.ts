import { DEVICE, meta, otaRun, peer, snapshot } from './fixtures';
import { rig } from './harness';

describe('disconnect contract', () => {
  test('an ota the device ended keeps its outcome across a foreground', () => {
    const r = rig();
    r.emit('peerConnected', peer());
    r.emit('otaRunChanged', otaRun());
    r.emit('peerDisconnected', DEVICE);
    r.emit(
      'otaRunChanged',
      otaRun({ phase: 'failed', outcome: 'failed', error: 'link died' }),
    );

    r.emit('resumed', {
      ...snapshot([]),
      otaRuns: [otaRun({ phase: 'failed', outcome: 'failed' })],
    });

    expect(r.ota.isRunning(r.ota.useOtaStore.getState().runs[DEVICE])).toBe(
      false,
    );
  });

  test('an install queued behind an update is still queued after a foreground', () => {
    const r = rig();
    const waiting = { webappId: 'app', webappName: 'App', version: '1.2.0' };
    r.emit('peerConnected', peer());
    r.emit('otaQueueChanged', { deviceId: DEVICE, installs: [waiting] });

    r.emit('resumed', {
      ...snapshot([peer()]),
      otaQueues: [{ deviceId: DEVICE, installs: [waiting] }],
    });

    expect(r.ota.useOtaStore.getState().queues[DEVICE]?.installs).toEqual([
      waiting,
    ]);

    r.emit('otaQueueChanged', { deviceId: DEVICE, installs: [] });

    expect(r.ota.useOtaStore.getState().queues[DEVICE]?.installs).toEqual([]);
  });

  test('webapps for a device are not rendered as installed once it is gone', () => {
    const r = rig();
    r.emit('peerConnected', peer());
    r.emit('webappsChanged', {
      deviceId: DEVICE,
      webapps: [
        {
          id: 'app',
          name: 'App',
          version: '1.0.0',
          source: 'catalog',
          role: 'primary',
        },
      ],
    });
    expect(r.webapps.installedWebapps(DEVICE)).toHaveLength(1);

    r.emit('peerDisconnected', DEVICE);

    expect(r.webapps.installedWebapps(DEVICE)).toHaveLength(0);
  });
});

describe('last-known device capability', () => {
  test('the resolved lib version is unchanged by a disconnect', () => {
    const r = rig();
    r.emit('peerConnected', peer());
    r.emit(
      'deviceMetaChanged',
      DEVICE,
      meta({ libbridgethingVersion: '0.4.0' }),
    );

    const connected = r.catalog.deviceLibVersion(
      r.session.useSessionStore.getState(),
      DEVICE,
    );
    r.emit('peerDisconnected', DEVICE);
    const disconnected = r.catalog.deviceLibVersion(
      r.session.useSessionStore.getState(),
      DEVICE,
    );

    expect(connected).toBe('0.4.0');
    expect(disconnected).toBe(connected);
  });

  test('the resolved lib version survives an app relaunch', () => {
    const first = rig();
    first.emit('peerConnected', peer());
    first.emit(
      'deviceMetaChanged',
      DEVICE,
      meta({ libbridgethingVersion: '0.4.0' }),
    );

    const second = first.relaunch();

    expect(
      second.catalog.deviceLibVersion(
        second.session.useSessionStore.getState(),
        DEVICE,
      ),
    ).toBe('0.4.0');
  });
});
