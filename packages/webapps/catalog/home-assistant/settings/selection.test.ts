import { describe, expect, test } from 'bun:test';
import { edited, fromDevice, NO_SELECTION, splitIds, stored, toggled } from './selection';

function picked(held: { picked: ReadonlySet<string> }): string[] {
  return [...held.picked];
}

describe('splitIds', () => {
  test('reads the comma list the device holds', () => {
    expect(splitIds('light.kitchen, switch.fan ,,')).toEqual(['light.kitchen', 'switch.fan']);
  });

  test('a missing doc is an empty selection', () => {
    expect(splitIds(null)).toEqual([]);
  });
});

describe('a selection the user has not touched', () => {
  test('takes whatever the device holds', () => {
    const loaded = fromDevice(NO_SELECTION, 'light.kitchen,switch.fan');

    expect(picked(loaded)).toEqual(['light.kitchen', 'switch.fan']);
    expect(edited(loaded)).toBe(false);
  });

  test('follows the device when it changes again', () => {
    const loaded = fromDevice(NO_SELECTION, 'light.kitchen');
    const reread = fromDevice(loaded, 'switch.fan');

    expect(picked(reread)).toEqual(['switch.fan']);
    expect(edited(reread)).toBe(false);
  });

  test('follows the device when the doc is deleted', () => {
    const reread = fromDevice(fromDevice(NO_SELECTION, 'light.kitchen'), null);

    expect(picked(reread)).toEqual([]);
    expect(edited(reread)).toBe(false);
  });
});

describe('a selection with unsaved ticks', () => {
  const loaded = fromDevice(NO_SELECTION, 'light.kitchen');
  const ticked = toggled(loaded, 'switch.fan');

  test('is edited until it matches the device again', () => {
    expect(picked(ticked)).toEqual(['light.kitchen', 'switch.fan']);
    expect(edited(ticked)).toBe(true);
    expect(edited(toggled(ticked, 'switch.fan'))).toBe(false);
  });

  test('keeps the ticks when the device is reread', () => {
    const reread = fromDevice(ticked, 'light.kitchen,sensor.porch');

    expect(picked(reread)).toEqual(['light.kitchen', 'switch.fan']);
    expect(edited(reread)).toBe(true);
  });

  test('stops being edited once the device holds the same entities, in any order', () => {
    const reread = fromDevice(ticked, 'switch.fan,light.kitchen');

    expect(picked(reread)).toEqual(['light.kitchen', 'switch.fan']);
    expect(edited(reread)).toBe(false);
  });

  test('is clean after its own write lands, and keeps ticks made while that write was in flight', () => {
    const written = picked(ticked);
    const later = toggled(ticked, 'sensor.porch');
    const settled = stored(later, written);

    expect(picked(settled)).toEqual(['light.kitchen', 'switch.fan', 'sensor.porch']);
    expect(edited(settled)).toBe(true);
    expect(edited(stored(ticked, written))).toBe(false);
  });
});
