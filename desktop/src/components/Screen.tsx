import { Button, cx } from '@bridgething/ui';
import type { ComponentChildren, VNode } from 'preact';
import { useLocation } from 'preact-iso';

import { Icon } from '../lib/icons.tsx';

export function Screen({ class: className, children }: { class?: string; children: ComponentChildren }): VNode {
  return (
    <main class="min-h-0 min-w-0 flex-1 overflow-y-auto">
      <div class={cx('mx-auto w-full max-w-4xl px-8 py-7', className)}>{children}</div>
    </main>
  );
}

export function Section({ class: className, children }: { class?: string; children: ComponentChildren }): VNode {
  return <section class={cx('mb-9', className)}>{children}</section>;
}

export function ErrorNote({ children }: { children: ComponentChildren }): VNode {
  return <p class="mt-2 border border-err/30 bg-err-soft px-3 py-2 text-hint wrap-break-word text-err">{children}</p>;
}

export function Hint({ children }: { children: ComponentChildren }): VNode {
  return <p class="mt-2 text-hint leading-relaxed wrap-break-word text-muted">{children}</p>;
}

export function BackButton({ to, children }: { to?: string; children: ComponentChildren }): VNode {
  const { back, route } = useLocation();

  return (
    <Button
      variant="ghost"
      size="sm"
      class="mb-5"
      icon={<Icon name="back" size={14} />}
      onClick={to === undefined ? back : () => route(to)}>
      {children}
    </Button>
  );
}
