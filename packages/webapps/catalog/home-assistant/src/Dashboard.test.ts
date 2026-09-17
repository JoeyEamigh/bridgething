import { describe, expect, test } from 'bun:test';
import type { Tile } from './App';
import { sameTile } from './Dashboard';
import { applyEntitiesEvent, type HaEntities } from './ha';

function tile(id: string, entities: HaEntities, pendingTemp: number | null = null): Tile {
  return { entityId: id, state: entities[id] ?? null, pendingTemp, pendingBrightness: null };
}

function seed(): HaEntities {
  return applyEntitiesEvent(
    {},
    {
      a: {
        'light.kitchen': { s: 'off', a: { friendly_name: 'Kitchen' } },
        'sensor.temp': { s: '20', a: { unit_of_measurement: 'C' } },
      },
    },
  )!;
}

describe('sameTile', () => {
  test('a sensor update leaves every untouched tile comparing equal', () => {
    const before = seed();
    const after = applyEntitiesEvent(before, { c: { 'sensor.temp': { '+': { s: '21' } } } })!;

    expect(sameTile(tile('light.kitchen', before), tile('light.kitchen', after))).toBe(true);
    expect(sameTile(tile('sensor.temp', before), tile('sensor.temp', after))).toBe(false);
  });

  test('an attribute-only change is not equal', () => {
    const before = seed();
    const after = applyEntitiesEvent(before, { c: { 'light.kitchen': { '+': { a: { brightness: 40 } } } } })!;

    expect(sameTile(tile('light.kitchen', before), tile('light.kitchen', after))).toBe(false);
  });

  test('an optimistic overlay on top of the same live entity is not equal', () => {
    const entities = seed();
    const live = tile('light.kitchen', entities);
    const optimistic: Tile = { ...live, state: { ...entities['light.kitchen'], state: 'on' } };

    expect(sameTile(live, optimistic)).toBe(false);
  });

  test('the same overlay re-derived from an unchanged entity compares equal', () => {
    const entities = seed();
    const overlay = (): Tile => ({
      entityId: 'light.kitchen',
      state: { ...entities['light.kitchen'], state: 'on' },
      pendingTemp: null,
      pendingBrightness: null,
    });

    expect(sameTile(overlay(), overlay())).toBe(true);
  });

  test('a pending climate target change is not equal', () => {
    const entities = seed();
    expect(sameTile(tile('sensor.temp', entities, 20), tile('sensor.temp', entities, 21))).toBe(false);
    expect(sameTile(tile('sensor.temp', entities, 20), tile('sensor.temp', entities, 20))).toBe(true);
  });

  test('an entity that has not arrived yet compares equal to itself and unequal once it lands', () => {
    const missing = tile('light.hall', {});
    expect(sameTile(missing, tile('light.hall', {}))).toBe(true);
    expect(sameTile(missing, tile('light.kitchen', seed()))).toBe(false);
  });

  test('a pending brightness change is not equal', () => {
    const entities = seed();
    const a: Tile = { ...tile('light.kitchen', entities), pendingBrightness: 70 };
    const b: Tile = { ...tile('light.kitchen', entities), pendingBrightness: 80 };
    expect(sameTile(a, b)).toBe(false);
    expect(sameTile(a, { ...a })).toBe(true);
  });
});
