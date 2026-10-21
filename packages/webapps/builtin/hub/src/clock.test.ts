import { describe, expect, test } from 'bun:test';
import { clockText } from './clock.ts';

const TIME = { wallClockUnixS: 1_792_605_999, tzIana: 'America/New_York', locale: 'en-US' };

describe('clockText', () => {
  test('without a wall clock from the phone there is no time to show', () => {
    expect(clockText(null, 0, 0)).toBe('n/a');
    expect(clockText({ ...TIME, wallClockUnixS: null }, 0, 0)).toBe('n/a');
  });

  test('reads the phone wall clock in its timezone and locale', () => {
    const text = clockText(TIME, 1_000, 1_000);
    expect(text).toContain('Oct 21');
    expect(text).toContain('02:06 PM');
  });

  test('advances by the time since the phone reported it', () => {
    expect(clockText(TIME, 1_000, 1_000 + 3 * 60_000)).toContain('02:09 PM');
  });
});
