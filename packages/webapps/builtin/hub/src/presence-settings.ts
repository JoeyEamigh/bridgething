import type { PresenceSettings } from '@bridgething/client';

/**
 * Idle seconds a timer slider can sit on, in order. The last stop is never. The shortest is 30 seconds because a
 * host reports its idle time every few seconds, and a shorter timer could fire between two reports.
 */
export const TIMER_STEPS: ReadonlyArray<number | null> = [30, 60, 120, 300, 600, 900, 1800, 3600, 7200, null];

export const DIM_LEVEL_MIN_PCT = 5;
export const DIM_LEVEL_MAX_PCT = 100;
export const DIM_LEVEL_STEP_PCT = 5;

export type Timer = 'dim' | 'sleep';

export function timerLabel(seconds: number | null): string {
  if (seconds === null) return 'never';
  if (seconds < 60) return `${seconds} sec`;
  if (seconds < 3600) return `${seconds / 60} min`;
  return `${seconds / 3600} hr`;
}

export function stepIndex(seconds: number | null | undefined): number {
  if (seconds === null || seconds === undefined) return TIMER_STEPS.length - 1;
  let best = 0;
  for (let i = 0; i < TIMER_STEPS.length - 1; i++) {
    const step = TIMER_STEPS[i]!;
    if (Math.abs(step - seconds) < Math.abs(TIMER_STEPS[best]! - seconds)) best = i;
  }
  return best;
}

function stepSeconds(index: number): number | null {
  return TIMER_STEPS[Math.max(0, Math.min(index, TIMER_STEPS.length - 1))] ?? null;
}

/** The settings after a timer slider moves to `index`. Dimming stays ahead of sleeping. */
export function moveTimer(settings: PresenceSettings, timer: Timer, index: number): PresenceSettings {
  const seconds = stepSeconds(index);
  const next: PresenceSettings = {
    ...settings,
    [timer === 'dim' ? 'dimAfterS' : 'sleepAfterS']: seconds,
  };
  if (next.dimAfterS == null || next.sleepAfterS == null) return next;
  const dim = stepIndex(next.dimAfterS);
  const sleep = stepIndex(next.sleepAfterS);
  if (dim < sleep) return next;
  if (timer === 'dim') return { ...next, sleepAfterS: stepSeconds(dim + 1) };
  return { ...next, dimAfterS: sleep === 0 ? null : stepSeconds(sleep - 1) };
}

/** The timers stay as they are: while following they take over whenever no computer is connected. */
export function setFollowHost(settings: PresenceSettings, on: boolean): PresenceSettings {
  return { ...settings, followHost: on };
}

export function setDimLevel(settings: PresenceSettings, pct: number): PresenceSettings {
  const clamped = Math.max(DIM_LEVEL_MIN_PCT, Math.min(DIM_LEVEL_MAX_PCT, pct));
  return { ...settings, dimLevel: clamped / 100 };
}

export function dimLevelPct(settings: PresenceSettings): number {
  return Math.round(settings.dimLevel * 100);
}
