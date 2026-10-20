import type { ComponentChildren, VNode } from 'preact';
import { useId } from 'preact/hooks';

import { cx } from '../cx.ts';

const INPUT_TYPES = {
  text: { type: 'text' },
  password: { type: 'password' },
  url: { type: 'url' },
  search: { type: 'search' },
  email: { type: 'email' },
} as const;

export function Field({
  label,
  hint,
  icon,
  value,
  onInput,
  onCommit,
  placeholder,
  type = 'text',
  disabled = false,
  clearable = false,
  class: className,
}: {
  label?: ComponentChildren;
  hint?: ComponentChildren;
  icon?: ComponentChildren;
  value: string;
  onInput: (next: string) => void;
  onCommit?: (value: string) => void;
  placeholder?: string;
  type?: keyof typeof INPUT_TYPES;
  disabled?: boolean;
  clearable?: boolean;
  class?: string;
}): VNode {
  const id = useId();

  return (
    <div class={cx('flex flex-col', className)}>
      {label ? (
        <label for={id} class="mb-1.5 font-mono text-eyebrow tracking-[0.18em] text-muted uppercase">
          {label}
        </label>
      ) : null}
      <div
        class={cx(
          'flex items-center gap-2 border border-rule-strong bg-screen px-3 transition-colors focus-within:border-accent',
          disabled && 'opacity-40',
        )}>
        {icon ? <span class="flex shrink-0 items-center text-dim">{icon}</span> : null}
        <input
          id={id}
          {...INPUT_TYPES[type]}
          value={value}
          placeholder={placeholder}
          disabled={disabled}
          class="min-w-0 flex-1 border-0 bg-transparent py-2.5 text-row text-off-white outline-none placeholder:text-dim"
          onInput={event => onInput(event.currentTarget.value)}
          onBlur={event => onCommit?.(event.currentTarget.value)}
          onKeyDown={event => {
            if (event.key === 'Enter') onCommit?.(event.currentTarget.value);
          }}
        />
        {clearable && value.length > 0 ? (
          <button
            type="button"
            aria-label="clear"
            disabled={disabled}
            class="shrink-0 px-1 font-mono text-body text-dim transition-colors hover:text-off-white focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            onClick={() => onInput('')}>
            &times;
          </button>
        ) : null}
      </div>
      {hint ? <span class="mt-1.5 text-hint text-muted">{hint}</span> : null}
    </div>
  );
}
