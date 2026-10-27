import type { RefObject, VNode } from 'preact';
import { useCallback, useEffect, useRef, useState } from 'preact/hooks';

import { cx } from '../cx.ts';

export const RAIL_SCROLL = 'overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden';

export type Rail = {
  ref: RefObject<HTMLDivElement | null>;
  atStart: boolean;
  atEnd: boolean;
  page: (direction: 1 | -1) => void;
};

export function useRail(): Rail {
  const ref = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ atStart: true, atEnd: true });

  useEffect(() => {
    const rail = ref.current;
    if (!rail) return;
    const measure = () => {
      const atStart = rail.scrollLeft <= 1;
      const atEnd = rail.scrollLeft + rail.clientWidth >= rail.scrollWidth - 1;
      setEdges(held => (held.atStart === atStart && held.atEnd === atEnd ? held : { atStart, atEnd }));
    };
    measure();
    rail.addEventListener('scroll', measure, { passive: true });
    const observer = new ResizeObserver(measure);
    observer.observe(rail);
    return () => {
      rail.removeEventListener('scroll', measure);
      observer.disconnect();
    };
  }, []);

  const page = useCallback((direction: 1 | -1) => {
    const rail = ref.current;
    if (!rail) return;
    const style = getComputedStyle(rail);
    const visible = rail.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
    rail.scrollBy({ left: direction * visible, behavior: still ? 'auto' : 'smooth' });
  }, []);

  return { ref, ...edges, page };
}

const CHEVRON = (
  <svg viewBox="0 0 16 16" aria-hidden="true" class="size-3.5 fill-none stroke-current stroke-[1.6]">
    <path d="M6 3.5L10.5 8 6 12.5" />
  </svg>
);

function RailArrow({ direction, disabled, onClick }: { direction: 1 | -1; disabled: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={direction === 1 ? 'scroll forward' : 'scroll back'}
      disabled={disabled}
      onClick={onClick}
      class={cx(
        'grid size-7 place-items-center border border-rule text-soft transition-colors hover:border-edge hover:text-off-white focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:pointer-events-none disabled:opacity-35',
        direction === -1 && '[&>svg]:rotate-180',
      )}>
      {CHEVRON}
    </button>
  );
}

export function RailArrows({ rail, class: className }: { rail: Rail; class?: string }): VNode {
  const idle = rail.atStart && rail.atEnd;
  return (
    <span class={cx('flex shrink-0 gap-1', idle && 'invisible', className)}>
      <RailArrow direction={-1} disabled={rail.atStart} onClick={() => rail.page(-1)} />
      <RailArrow direction={1} disabled={rail.atEnd} onClick={() => rail.page(1)} />
    </span>
  );
}
