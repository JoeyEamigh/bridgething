import type * as api from '@bridgething/companion-types';
import { beforeEach, describe, expect, test } from 'bun:test';

import { selectedDevice, selectedMeta, snapshot } from './session.ts';

function meta(libbridgethingVersion: string): api.DeviceMeta {
  return {
    daemonVersion: '0.1.0',
    libbridgethingVersion,
    imageVersion: '2026.1',
    appName: 'bridgething',
    osName: 'superbird',
    osVersion: '1.0',
    channel: 'stable',
    modelName: 'Car Thing',
    serialNumber: 'sn',
    nickname: null,
    launcherGesture: 'fivePress',
  };
}

function linked(entries: api.DeviceMetaEntry[]): void {
  snapshot.data.value = { deviceMeta: entries } as api.SessionSnapshot;
}

describe('selectedMeta', () => {
  beforeEach(() => {
    snapshot.data.value = null;
    selectedDevice.data.value = null;
  });

  test('reads the selected device, never the first one linked', () => {
    linked([
      { deviceId: 'older', meta: meta('0.4.0') },
      { deviceId: 'newer', meta: meta('0.9.0') },
    ]);
    selectedDevice.data.value = 'newer';

    expect(selectedMeta.value?.libbridgethingVersion).toBe('0.9.0');
  });

  test('holds nothing while no device is selected', () => {
    linked([{ deviceId: 'older', meta: meta('0.4.0') }]);

    expect(selectedMeta.value).toBe(null);
  });

  test('holds nothing for a selection that never reported meta', () => {
    linked([{ deviceId: 'older', meta: meta('0.4.0') }]);
    selectedDevice.data.value = 'newer';

    expect(selectedMeta.value).toBe(null);
  });
});
