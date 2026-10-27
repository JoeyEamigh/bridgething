import type { ComponentChildren, VNode } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { cx } from '../cx.ts';

export const SLIDE_ART =
  'opacity-0 transition-opacity duration-700 group-data-[on]/slide:opacity-100 motion-reduce:transition-none';

export const SLIDE_TEXT =
  'translate-y-1 opacity-0 transition-[opacity,translate] duration-200 group-data-[on]/slide:translate-y-0 group-data-[on]/slide:opacity-100 group-data-[on]/slide:delay-200 motion-reduce:transition-none';

function useReducedMotion(): boolean {
  const [still, setStill] = useState(false);
  useEffect(() => {
    const query = matchMedia('(prefers-reduced-motion: reduce)');
    const sync = () => setStill(query.matches);
    sync();
    query.addEventListener('change', sync);
    return () => query.removeEventListener('change', sync);
  }, []);
  return still;
}

export function Carousel<T>({
  items,
  label,
  keyOf,
  render,
  class: className,
}: {
  items: T[];
  label: string;
  keyOf: (item: T) => string;
  render: (item: T) => ComponentChildren;
  class?: string;
}): VNode {
  const [chosen, setChosen] = useState(0);
  const [held, setHeld] = useState(false);
  const still = useReducedMotion();
  const shown = chosen < items.length ? chosen : 0;
  const rotating = items.length > 1 && !still;

  return (
    <section
      aria-roledescription="carousel"
      aria-label={label}
      class={className}
      onMouseEnter={() => setHeld(true)}
      onMouseLeave={() => setHeld(false)}
      onFocusIn={() => setHeld(true)}
      onFocusOut={() => setHeld(false)}>
      <div class="grid">
        {items.map((item, index) => (
          <div
            key={keyOf(item)}
            class={cx('group/slide col-start-1 row-start-1 min-w-0', index !== shown && 'pointer-events-none')}
            data-on={index === shown || undefined}
            aria-hidden={index !== shown || undefined}
            inert={index !== shown}>
            {render(item)}
          </div>
        ))}
      </div>
      {items.length > 1 ? (
        <div class="mt-3 flex gap-1">
          {items.map((item, index) => (
            <button
              key={keyOf(item)}
              type="button"
              aria-label={`show ${index + 1} of ${items.length}`}
              aria-current={index === shown || undefined}
              onClick={() => setChosen(index)}
              class="group/dot relative h-5 w-10 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent">
              <span class="absolute inset-x-0 top-1/2 h-0.5 -translate-y-1/2 overflow-hidden bg-rule-strong transition-colors group-hover/dot:bg-edge">
                {index === shown ? (
                  <span
                    key={`${shown}:${rotating}`}
                    class={cx(
                      'absolute inset-0 origin-left bg-accent',
                      rotating && 'animate-carousel',
                      rotating && held && '[animation-play-state:paused]',
                    )}
                    onAnimationEnd={() => setChosen((shown + 1) % items.length)}
                  />
                ) : null}
              </span>
            </button>
          ))}
        </div>
      ) : null}
    </section>
  );
}
