import type { DimSettings } from '@bridgething/client';
import { describe, expect, test } from 'bun:test';

import {
  BRIGHTNESS_MODE_OPTIONS,
  DIM_AFTER_OPTIONS,
  DIM_LEVEL_STEP_PCT,
  dimAfterPreset,
  dimAfterSeconds,
  dimLevelLabel,
  levelLabel,
  levelPct,
  pctLevel,
} from './display-settings.ts';

const fresh: DimSettings = { afterS: null, level: 0.2 };

function acceptedByDaemon(settings: DimSettings): boolean {
  const timer = settings.afterS == null || (settings.afterS >= 15 && settings.afterS <= 86_400);
  return timer && settings.level >= 0 && settings.level <= 1;
}

describe('brightness mode', () => {
  test('offers auto then manual', () => {
    expect(BRIGHTNESS_MODE_OPTIONS.map(option => option.value)).toEqual(['auto', 'manual']);
  });
});

describe('dim after', () => {
  test('offers never, then one, five and fifteen minutes and an hour', () => {
    expect(DIM_AFTER_OPTIONS.map(option => option.label)).toEqual(['never', '1m', '5m', '15m', '1h']);
    expect(DIM_AFTER_OPTIONS.map(option => dimAfterSeconds(option.value))).toEqual([null, 60, 300, 900, 3600]);
  });

  test('every preset reads back as itself', () => {
    for (const { value } of DIM_AFTER_OPTIONS) expect(dimAfterPreset(dimAfterSeconds(value))).toBe(value);
  });

  test('an unset timer is never, and a timer set elsewhere shows the nearest preset', () => {
    expect(dimAfterPreset(null)).toBe('never');
    expect(dimAfterPreset(undefined)).toBe('never');
    expect(dimAfterPreset(15)).toBe('60');
    expect(dimAfterPreset(120)).toBe('60');
    expect(dimAfterPreset(600)).toBe('300');
    expect(dimAfterPreset(1800)).toBe('900');
    expect(dimAfterPreset(86_400)).toBe('3600');
  });
});

describe('levels', () => {
  test('run from dark to full and read as a percentage', () => {
    expect(pctLevel(0)).toBe(0);
    expect(pctLevel(250)).toBe(1);
    expect(pctLevel(-5)).toBe(0);
    expect(levelPct(pctLevel(35))).toBe(35);
    expect(dimLevelLabel(0)).toBe('dark');
    expect(dimLevelLabel(20)).toBe('20%');
    expect(levelLabel(0)).toBe('0%');
  });

  test('a fresh device dims to twenty percent and never times out', () => {
    expect(levelPct(fresh.level)).toBe(20);
    expect(dimAfterPreset(fresh.afterS)).toBe('never');
  });
});

test('every combination the controls can reach is one the daemon accepts', () => {
  for (const { value } of DIM_AFTER_OPTIONS) {
    for (let pct = 0; pct <= 100; pct += DIM_LEVEL_STEP_PCT) {
      expect(acceptedByDaemon({ afterS: dimAfterSeconds(value), level: pctLevel(pct) })).toBe(true);
    }
  }
});
