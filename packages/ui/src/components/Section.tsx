import type { ComponentChildren, VNode } from 'preact';

import { cx } from '../cx.ts';
import { Spinner } from './Spinner.tsx';

export function SectionHeader({
  title,
  hint,
  trailing,
  action,
  onAction,
  pending = false,
  class: className,
}: {
  title: ComponentChildren;
  hint?: ComponentChildren;
  trailing?: ComponentChildren;
  action?: ComponentChildren;
  onAction?: () => void;
  pending?: boolean;
  class?: string;
}): VNode {
  return (
    <div class={cx('mb-2 flex items-end justify-between gap-3', className)}>
      <div class="flex min-w-0 flex-1 flex-col gap-0.5">
        <span class="font-mono text-eyebrow tracking-[0.18em] text-muted uppercase">{title}</span>
        {hint ? <span class="text-hint text-muted">{hint}</span> : null}
      </div>
      {trailing}
      {action ? (
        <button
          type="button"
          class="flex shrink-0 items-center gap-1.5 px-1 font-mono text-hint text-soft uppercase transition-colors hover:text-off-white focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 disabled:text-dim"
          disabled={pending}
          aria-busy={pending || undefined}
          onClick={onAction}>
          {pending ? <Spinner class="size-3" /> : null}
          {action}
        </button>
      ) : null}
    </div>
  );
}

export function SectionEmpty({ class: className, children }: { class?: string; children: ComponentChildren }): VNode {
  return (
    <div
      class={cx('border border-rule bg-screen px-4 py-6 text-center text-body wrap-break-word text-muted', className)}>
      {children}
    </div>
  );
}
