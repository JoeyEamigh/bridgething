import type { PresenceSettings } from '@bridgething/client';
import { describe, expect, test } from 'bun:test';
import {
  TIMER_STEPS,
  dimLevelPct,
  moveTimer,
  setDimLevel,
  setFollowHost,
  stepIndex,
  timerLabel,
} from './presence-settings.ts';

const fresh: PresenceSettings = { followHost: false, dimAfterS: null, sleepAfterS: null, dimLevel: 0.2 };
const NEVER = TIMER_STEPS.length - 1;

describe('timer steps', () => {
  test('the slider ends at never, and every other stop is an increasing number of seconds the daemon accepts', () => {
    expect(TIMER_STEPS[NEVER]).toBeNull();
    const seconds = TIMER_STEPS.slice(0, NEVER) as number[];
    expect(seconds).toEqual([...seconds].sort((a, b) => a - b));
    for (const s of seconds) {
      expect(s).toBeGreaterThanOrEqual(5);
      expect(s).toBeLessThanOrEqual(86_400);
    }
  });

  test('stops read as seconds, minutes, hours and never', () => {
    expect(timerLabel(null)).toBe('never');
    expect(timerLabel(30)).toBe('30 sec');
    expect(timerLabel(60)).toBe('1 min');
    expect(timerLabel(900)).toBe('15 min');
    expect(timerLabel(3600)).toBe('1 hr');
    expect(timerLabel(7200)).toBe('2 hr');
  });

  test('a stored value lands on its stop, an unset timer on never, and an odd value on the nearest stop', () => {
    expect(stepIndex(null)).toBe(NEVER);
    expect(stepIndex(undefined)).toBe(NEVER);
    expect(stepIndex(120)).toBe(TIMER_STEPS.indexOf(120));
    expect(stepIndex(100)).toBe(TIMER_STEPS.indexOf(120));
    expect(stepIndex(1)).toBe(0);
    expect(stepIndex(86_400)).toBe(NEVER - 1);
  });
});

describe('moving a timer', () => {
  test('a fresh device has both timers on never, nothing followed, and the dim level at twenty percent', () => {
    expect(fresh.followHost).toBe(false);
    expect(stepIndex(fresh.dimAfterS)).toBe(NEVER);
    expect(stepIndex(fresh.sleepAfterS)).toBe(NEVER);
    expect(dimLevelPct(fresh)).toBe(20);
  });

  test('moving a timer sets just that timer', () => {
    const dim = moveTimer(fresh, 'dim', TIMER_STEPS.indexOf(60));
    expect(dim).toMatchObject({ dimAfterS: 60, sleepAfterS: null, followHost: false });
    const sleep = moveTimer(dim, 'sleep', TIMER_STEPS.indexOf(900));
    expect(sleep).toMatchObject({ dimAfterS: 60, sleepAfterS: 900 });
  });

  test('moving a timer leaves following the host alone', () => {
    const following = setFollowHost(fresh, true);
    expect(moveTimer(following, 'dim', 3).followHost).toBe(true);
    expect(moveTimer(following, 'sleep', NEVER).followHost).toBe(true);
  });

  test('dimming cannot catch up with sleeping: the dim slider pushes sleep later', () => {
    const base = moveTimer(moveTimer(fresh, 'sleep', TIMER_STEPS.indexOf(300)), 'dim', TIMER_STEPS.indexOf(60));
    const moved = moveTimer(base, 'dim', TIMER_STEPS.indexOf(300));
    expect(moved.dimAfterS).toBe(300);
    expect(moved.sleepAfterS).toBe(600);
  });

  test('sleeping cannot come before dimming: the sleep slider pulls dim earlier', () => {
    const base = moveTimer(moveTimer(fresh, 'dim', TIMER_STEPS.indexOf(300)), 'sleep', TIMER_STEPS.indexOf(900));
    const moved = moveTimer(base, 'sleep', TIMER_STEPS.indexOf(120));
    expect(moved.sleepAfterS).toBe(120);
    expect(moved.dimAfterS).toBe(60);
  });

  test('sleeping on the first stop leaves no room to dim first, so dimming goes to never', () => {
    const base = moveTimer(fresh, 'dim', 0);
    const moved = moveTimer(base, 'sleep', 0);
    expect(moved.sleepAfterS).toBe(30);
    expect(moved.dimAfterS).toBeNull();
  });

  test('dimming on the last stop before never pushes sleep to never rather than past the ladder', () => {
    const base = moveTimer(fresh, 'sleep', NEVER - 1);
    const moved = moveTimer(base, 'dim', NEVER - 1);
    expect(moved.dimAfterS).toBe(7200);
    expect(moved.sleepAfterS).toBeNull();
  });

  test('every combination the sliders can reach is one the daemon accepts', () => {
    for (let d = 0; d <= NEVER; d++) {
      for (let s = 0; s <= NEVER; s++) {
        const settings = moveTimer(moveTimer(fresh, 'dim', d), 'sleep', s);
        if (settings.dimAfterS != null && settings.sleepAfterS != null) {
          expect(settings.dimAfterS).toBeLessThan(settings.sleepAfterS);
        }
      }
    }
  });
});

describe('following the host', () => {
  test('turning it on keeps the timers, which take over when no computer is connected', () => {
    const timed = moveTimer(moveTimer(fresh, 'dim', 3), 'sleep', 6);
    expect(setFollowHost(timed, true)).toMatchObject({ followHost: true, dimAfterS: 300, sleepAfterS: 1800 });
  });

  test('turning it off keeps them too', () => {
    const timed = moveTimer(moveTimer(fresh, 'dim', 3), 'sleep', 6);
    expect(setFollowHost(setFollowHost(timed, true), false)).toEqual(timed);
  });

  test('on a fresh device it turns on and off without touching anything else', () => {
    expect(setFollowHost(setFollowHost(fresh, true), false)).toEqual(fresh);
  });

  test('the dim level survives either change', () => {
    const custom = setDimLevel(fresh, 55);
    expect(setFollowHost(custom, true).dimLevel).toBeCloseTo(0.55);
    expect(moveTimer(custom, 'dim', 2).dimLevel).toBeCloseTo(0.55);
  });
});

describe('dim level', () => {
  test('is a percentage the daemon accepts', () => {
    expect(setDimLevel(fresh, 40).dimLevel).toBeCloseTo(0.4);
    expect(setDimLevel(fresh, 0).dimLevel).toBeCloseTo(0.05);
    expect(setDimLevel(fresh, 250).dimLevel).toBe(1);
    expect(dimLevelPct(setDimLevel(fresh, 35))).toBe(35);
  });
});
