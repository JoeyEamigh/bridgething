import { describe, expect, test } from 'bun:test';
import { deviceName } from '../src/device-name.js';

describe('deviceName', () => {
  test('a nickname wins over the bluetooth name', () => {
    expect(deviceName({ nickname: 'kitchen', bluetoothName: 'Car Thing (SN: Q61R)' })).toBe('kitchen');
  });

  test('without a nickname the device goes by the name phones see when pairing', () => {
    expect(deviceName({ nickname: null, bluetoothName: 'Car Thing (SN: Q61R)' })).toBe('Car Thing (SN: Q61R)');
  });

  test('a daemon with no radio and no nickname is just a Car Thing', () => {
    expect(deviceName({ nickname: '', bluetoothName: null })).toBe('Car Thing');
  });
});
