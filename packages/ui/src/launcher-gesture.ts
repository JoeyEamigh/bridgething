import type { LauncherGesture } from '@bridgething/companion-types';

export const LAUNCHER_GESTURE_OPTIONS: ReadonlyArray<{ value: LauncherGesture; label: string }> = [
  { value: 'longPress', label: 'hold' },
  { value: 'doublePress', label: '2x' },
  { value: 'fivePress', label: '5x' },
];
