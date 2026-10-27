import type { LauncherGesture } from '@bridgething/client';
import { describe, expect, test } from 'bun:test';
import { GESTURES, practiceFor } from './gesture.ts';

describe('launcher gestures', () => {
  test('every gesture is offered once, hold first, then by press count', () => {
    expect(GESTURES.map(g => g.value)).toEqual(['longPress', 'doublePress', 'fivePress']);
  });

  test('each gesture practices itself', () => {
    const practiced: Record<LauncherGesture, ReturnType<typeof practiceFor>['kind']> = {
      longPress: 'hold',
      doublePress: 'presses',
      fivePress: 'presses',
    };
    for (const { value } of GESTURES) expect(practiceFor(value).kind).toBe(practiced[value]);
  });

  test('press practice asks for the same count the daemon fires on', () => {
    expect(practiceFor('doublePress')).toMatchObject({ count: 2, windowMs: 400 });
    expect(practiceFor('fivePress')).toMatchObject({ count: 5, windowMs: 1500 });
  });
});
