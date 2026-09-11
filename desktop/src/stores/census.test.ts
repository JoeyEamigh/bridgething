import type * as api from '@bridgething/companion-types';
import { afterEach, beforeEach, describe, expect, mock, test } from 'bun:test';

type Report = { deviceId: string | null; apps: { appId: string; sourceUrl: string; version?: string | null }[] };

const sent: Report[] = [];

mock.module('@bridgething/catalog', () => ({
  reportInstalled: (report: Report) => {
    sent.push(report);
  },
}));

const { selectedDevice, snapshot, webapps } = await import('./session.ts');
const { watchInstallCensus } = await import('./census.ts');

const DEVICE = 'aa:bb:cc:dd:ee:ff';
const SERIAL = '8558R481Q61R';
const APP = '019e6701-13f8-71b5-ba04-85d326630e98';
const SOURCE = 'https://listed.example/catalog.json';

function installed(): api.WebappInfo {
  return {
    id: APP,
    name: 'calendar',
    source: 'installed',
    role: 'standard',
    version: '1.0.0',
    provenance: SOURCE,
    description: null,
    iconHash: null,
    settingsHash: null,
    overlayHash: null,
    config: [],
    permissions: [],
    extension: null,
  };
}

function linked(serial: string | null): void {
  snapshot.data.value = {
    deviceMeta: serial === null ? [] : [{ deviceId: DEVICE, meta: { serialNumber: serial } as api.DeviceMeta }],
  } as api.SessionSnapshot;
  selectedDevice.data.value = DEVICE;
}

describe('reporting what the linked device holds', () => {
  let stop: (() => void) | null = null;

  const watching = () => {
    stop = watchInstallCensus();
  };

  afterEach(() => {
    stop?.();
    stop = null;
  });

  beforeEach(() => {
    sent.length = 0;
    webapps.data.value = [];
    webapps.settled.value = false;
    webapps.error.value = undefined;
    snapshot.data.value = null;
    selectedDevice.data.value = null;
  });

  test('a listing that has not arrived is never reported as an empty device', () => {
    watching();
    linked(SERIAL);

    expect(sent).toHaveLength(0);
  });

  test('a settled listing is reported with the serial of the device it came from', () => {
    watching();
    linked(SERIAL);

    webapps.data.value = [installed()];
    webapps.settled.value = true;

    expect(sent.at(-1)).toEqual({
      deviceId: SERIAL,
      apps: [{ appId: APP, sourceUrl: SOURCE, version: '1.0.0' }],
    });
  });

  test('a device with no serial is not reported', () => {
    watching();
    linked(null);

    webapps.data.value = [installed()];
    webapps.settled.value = true;

    expect(sent).toHaveLength(0);
  });

  test('a listing that failed to read is not reported as an empty device', () => {
    watching();
    linked(SERIAL);
    webapps.settled.value = true;
    sent.length = 0;

    webapps.error.value = 'the device went away';
    webapps.data.value = [];

    expect(sent).toHaveLength(0);
  });

  test('a builtin app is left out, since the store has nothing to count it against', () => {
    watching();
    linked(SERIAL);

    webapps.data.value = [installed(), { ...installed(), id: 'hub', source: 'builtin', provenance: null }];
    webapps.settled.value = true;

    expect(sent.at(-1)!.apps).toEqual([{ appId: APP, sourceUrl: SOURCE, version: '1.0.0' }]);
  });
});
