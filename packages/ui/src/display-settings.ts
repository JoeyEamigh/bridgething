import type { BrightnessMode } from '@bridgething/companion-types';

export const BRIGHTNESS_MODE_OPTIONS: ReadonlyArray<{ value: BrightnessMode; label: string }> = [
  { value: 'auto', label: 'auto' },
  { value: 'manual', label: 'manual' },
];

export type DimAfter = 'never' | '60' | '300' | '900' | '3600';

export const DIM_AFTER_OPTIONS: ReadonlyArray<{ value: DimAfter; label: string }> = [
  { value: 'never', label: 'never' },
  { value: '60', label: '1m' },
  { value: '300', label: '5m' },
  { value: '900', label: '15m' },
  { value: '3600', label: '1h' },
];

export const DIM_LEVEL_STEP_PCT = 5;

export function dimAfterSeconds(preset: DimAfter): number | null {
  return preset === 'never' ? null : Number(preset);
}

export function dimAfterPreset(seconds: number | null | undefined): DimAfter {
  if (seconds === null || seconds === undefined) return 'never';
  let best: DimAfter = '60';
  for (const { value } of DIM_AFTER_OPTIONS) {
    if (value === 'never') continue;
    if (Math.abs(Number(value) - seconds) < Math.abs(Number(best) - seconds)) best = value;
  }
  return best;
}

export function levelPct(level: number): number {
  return Math.round(level * 100);
}

export function pctLevel(pct: number): number {
  return Math.max(0, Math.min(100, pct)) / 100;
}

export function levelLabel(pct: number): string {
  return `${pct}%`;
}

export function dimLevelLabel(pct: number): string {
  return pct === 0 ? 'dark' : levelLabel(pct);
}
