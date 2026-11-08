import type { VNode } from 'preact';

import { cx } from '../cx.ts';

export function Slider({
  value,
  min,
  max,
  step = 1,
  onInput,
  onChange,
  label,
  disabled = false,
  class: className,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onInput?: (next: number) => void;
  onChange: (next: number) => void;
  label: string;
  disabled?: boolean;
  class?: string;
}): VNode {
  const fill = max > min ? ((value - min) / (max - min)) * 100 : 0;

  return (
    <input
      type="range"
      min={min}
      max={max}
      step={step}
      value={value}
      disabled={disabled}
      aria-label={label}
      style={{ '--fill': `${fill}%` }}
      onInput={event => onInput?.(Number(event.currentTarget.value))}
      onChange={event => onChange(Number(event.currentTarget.value))}
      class={cx(
        'h-5 cursor-pointer appearance-none bg-transparent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
        '[&::-webkit-slider-runnable-track]:h-px [&::-webkit-slider-runnable-track]:bg-[linear-gradient(to_right,var(--color-accent)_var(--fill),var(--color-rule)_var(--fill))]',
        '[&::-webkit-slider-thumb]:-mt-[5.5px] [&::-webkit-slider-thumb]:size-3 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-none [&::-webkit-slider-thumb]:border-0 [&::-webkit-slider-thumb]:bg-accent [&::-webkit-slider-thumb]:shadow-none',
        '[&::-moz-range-track]:h-px [&::-moz-range-track]:bg-rule [&::-moz-range-progress]:h-px [&::-moz-range-progress]:bg-accent',
        '[&::-moz-range-thumb]:size-3 [&::-moz-range-thumb]:rounded-none [&::-moz-range-thumb]:border-0 [&::-moz-range-thumb]:bg-accent [&::-moz-range-thumb]:shadow-none',
        disabled && 'cursor-default',
        className,
      )}
    />
  );
}
