import type { ComponentChildren, VNode } from 'preact';
import { useEffect, useRef } from 'preact/hooks';

import { cx } from '../cx.ts';

export function Dialog({
  open,
  onClose,
  title,
  subtitle,
  footer,
  class: className,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: ComponentChildren;
  subtitle?: ComponentChildren;
  footer?: ComponentChildren;
  class?: string;
  children?: ComponentChildren;
}): VNode {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    if (open && !element.open) element.showModal();
    else if (!open && element.open) element.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      class="m-auto max-h-[calc(100vh-4rem)] overflow-hidden border border-rule-strong bg-bg p-0 text-off-white"
      onClose={onClose}
      onClick={event => {
        if (event.target === ref.current) onClose();
      }}>
      <div class={cx('flex max-h-[inherit] w-[min(32rem,calc(100vw-2rem))] flex-col', className)}>
        <div class="flex items-start gap-3 border-b border-rule px-4 py-3">
          <div class="flex min-w-0 flex-1 flex-col gap-0.5">
            <h2 class="m-0 font-display text-title font-medium tracking-display">{title}</h2>
            {subtitle ? <span class="text-hint text-muted">{subtitle}</span> : null}
          </div>
          <button
            type="button"
            aria-label="close"
            class="shrink-0 px-1 font-mono text-title leading-none text-dim transition-colors hover:text-off-white focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
            onClick={onClose}>
            &times;
          </button>
        </div>
        {children ? <div class="min-h-0 flex-1 overflow-auto px-4 py-4">{children}</div> : null}
        {footer ? <div class="flex shrink-0 justify-end gap-2 border-t border-rule px-4 py-3">{footer}</div> : null}
      </div>
    </dialog>
  );
}
