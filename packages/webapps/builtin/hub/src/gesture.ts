import type { LauncherGesture } from '@bridgething/client';

export const GESTURES: ReadonlyArray<{ value: LauncherGesture; label: string; hint: string }> = [
  { value: 'longPress', label: 'hold m', hint: 'press and hold the m button for a moment' },
  { value: 'doublePress', label: 'press m twice', hint: 'tap the m button twice, quickly' },
  { value: 'fivePress', label: 'press m 5 times', hint: 'tap the m button five times, quickly' },
];

export type Practice =
  | { kind: 'hold'; holdMs: number; prompt: string }
  | { kind: 'presses'; count: number; windowMs: number; prompt: string };

export function practiceFor(gesture: LauncherGesture): Practice {
  switch (gesture) {
    case 'longPress':
      return { kind: 'hold', holdMs: 800, prompt: 'hold the m button down until the bar fills. give it a go.' };
    case 'doublePress':
      return {
        kind: 'presses',
        count: 2,
        windowMs: 400,
        prompt: 'push m twice, quickly, to return here from any app. give it a go.',
      };
    case 'fivePress':
      return {
        kind: 'presses',
        count: 5,
        windowMs: 1500,
        prompt: 'push m five times, quickly, to return here from any app. give it a go.',
      };
  }
}
