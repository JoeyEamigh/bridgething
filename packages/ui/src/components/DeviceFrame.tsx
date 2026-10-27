import type { ComponentChildren, VNode } from 'preact';

import { cx } from '../cx.ts';
import { DEVICE_FRAME } from '../tokens.ts';

const { screen } = DEVICE_FRAME;

const percent = (fraction: number) => `${(fraction * 100).toFixed(3)}%`;

export function DeviceFrame({
  frame,
  class: className,
  children,
}: {
  frame: string;
  class?: string;
  children: ComponentChildren;
}): VNode {
  return (
    <div
      class={cx('relative min-w-0', className)}
      style={{ aspectRatio: `${DEVICE_FRAME.width} / ${DEVICE_FRAME.height}` }}>
      <div
        class="absolute overflow-hidden bg-black"
        style={{
          left: percent(screen.left),
          top: percent(screen.top),
          width: percent(screen.width),
          height: percent(screen.height),
        }}>
        {children}
      </div>
      <img src={frame} alt="" aria-hidden="true" draggable={false} class="pointer-events-none absolute inset-0 size-full" />
    </div>
  );
}
