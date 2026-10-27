export type Tone = 'neutral' | 'accent' | 'ok' | 'err' | 'warn' | 'experimental';

export const TONE_FILL: Record<Tone, string> = {
  neutral: 'bg-neutral-soft text-soft',
  accent: 'bg-accent-soft text-accent',
  ok: 'bg-ok-soft text-ok',
  err: 'bg-err-soft text-err',
  warn: 'bg-warn-soft text-warn',
  experimental: 'bg-experimental-soft text-experimental',
};

export const TONE_DOT: Record<Tone, string> = {
  neutral: 'bg-soft',
  accent: 'bg-accent',
  ok: 'bg-ok',
  err: 'bg-err',
  warn: 'bg-warn',
  experimental: 'bg-experimental',
};

export const TONE_EDGE: Record<Tone, string> = {
  neutral: 'border-rule',
  accent: 'border-accent/30',
  ok: 'border-ok/30',
  err: 'border-err/30',
  warn: 'border-warn/30',
  experimental: 'border-experimental/30',
};

export type BoxSize = 'sm' | 'md' | 'lg' | 'xl';

export const BOX: Record<BoxSize, string> = {
  sm: 'size-8',
  md: 'size-11',
  lg: 'size-14',
  xl: 'size-20',
};

export const BOX_TEXT: Record<BoxSize, string> = {
  sm: 'text-body',
  md: 'text-row-lg',
  lg: 'text-title',
  xl: 'text-screen-title',
};

export const DEVICE_FRAME = {
  width: 1394,
  height: 706,
  screen: { left: 96 / 1394, top: 69 / 706, width: 980 / 1394, height: 590 / 706 },
} as const;
