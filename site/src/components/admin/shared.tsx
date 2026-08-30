import type { ComponentChildren } from 'preact';
import { DirectoryApiError } from '../../lib/directory-client';

export const INPUT =
  'min-w-0 flex-1 border border-white/25 bg-transparent px-3 py-2 font-mono text-sm text-white placeholder:text-white/30 focus:border-white/50 focus:outline-none';

export function reason(err: unknown): string {
  return err instanceof DirectoryApiError ? err.message : String(err);
}

export function Notice({ kind, children }: { kind: 'ok' | 'err'; children: ComponentChildren }) {
  return (
    <p
      role={kind === 'err' ? 'alert' : 'status'}
      class={`m-0 border px-3 py-2 font-mono text-sm ${
        kind === 'err' ? 'text-warn border-warn/40' : 'text-ok border-ok/40'
      }`}>
      {children}
    </p>
  );
}

export function Loading({ what }: { what: string }) {
  return <p class="m-0 font-mono text-sm text-white/40">loading {what}…</p>;
}

export function Empty({ what }: { what: string }) {
  return <p class="m-0 font-mono text-sm text-white/45">{what}</p>;
}

export function Group({ title, count, children }: { title: string; count: number; children: ComponentChildren }) {
  return (
    <section class="mb-10">
      <header class="mb-3 flex flex-wrap items-baseline justify-between gap-3 border-b border-white/20 pb-2">
        <h2 class="m-0 text-xl">{title}</h2>
        <p class="m-0 font-mono text-sm text-white/40">{count}</p>
      </header>
      {children}
    </section>
  );
}
