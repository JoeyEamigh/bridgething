import type { ComponentChildren } from 'preact';
import {
  filterKey,
  installsLabel,
  listingKindLabel,
  STORE_COPY,
  type AppEntry,
  type CatalogAppListing,
  type FilterChip,
  type StoreFilter,
  webHref,
} from '@bridgething/catalog';
import { cx, DeviceFrame, RAIL_SCROLL, RailArrows, useRail, type Rail as RailState } from '@bridgething/ui';
import { appDetailPath, filterPath, SEARCH_PATH, STORE_PATH } from '../../lib/app-routes';
import { hideOnError } from '../../lib/img';
import { installListing, isPlaceholderDownload } from '../../lib/pending-install';

export type FrameSize = 'sm' | 'md' | 'lg';

const FRAME_ICON: Record<FrameSize, number> = { sm: 44, md: 56, lg: 88 };

const FRAME_SRC = '/car-thing-frame.webp';

export function firstShot(app: AppEntry): string | null {
  for (const shot of app.screenshots ?? []) {
    const href = webHref(shot);
    if (href) return href;
  }
  return null;
}

export function screenshots(app: AppEntry): string[] {
  return (app.screenshots ?? []).map(webHref).filter((href): href is string => href !== null);
}

export function AppIcon({ app, size, class: extra = '' }: { app: AppEntry; size: number; class?: string }) {
  const icon = webHref(app.icon);
  return (
    <span
      aria-hidden="true"
      style={{ width: `${size}px`, height: `${size}px` }}
      class={`bg-neutral-soft relative grid shrink-0 place-items-center overflow-hidden border border-white/10 ${extra}`}>
      <span class="font-mono text-white/35 uppercase" style={{ fontSize: `${Math.round(size * 0.4)}px` }}>
        {app.name.trim().charAt(0)}
      </span>
      {icon ? (
        <img
          src={icon}
          alt=""
          width={size}
          height={size}
          loading="lazy"
          class="absolute inset-0 size-full object-cover"
          onError={hideOnError}
        />
      ) : null}
    </span>
  );
}

export function AppFrame({
  app,
  shot,
  size,
  eager = false,
}: {
  app: AppEntry;
  shot: string | null;
  size: FrameSize;
  eager?: boolean;
}) {
  return (
    <DeviceFrame frame={FRAME_SRC}>
      {shot ? (
        <img
          src={shot}
          alt={`${app.name} on a car thing`}
          width="800"
          height="480"
          loading={eager ? 'eager' : 'lazy'}
          class="absolute inset-0 size-full object-cover"
          onError={hideOnError}
        />
      ) : (
        <span class="absolute inset-0 grid place-items-center">
          <AppIcon app={app} size={FRAME_ICON[size]} />
        </span>
      )}
    </DeviceFrame>
  );
}

export function installable(listing: CatalogAppListing): boolean {
  const newest = listing.newestCompatible;
  return newest !== null && !isPlaceholderDownload(newest.download);
}

export function GetButton({
  listing,
  label,
  primary = false,
}: {
  listing: CatalogAppListing;
  label: string;
  primary?: boolean;
}) {
  if (!installable(listing)) {
    return <span class="shrink-0 font-mono text-xs text-white/35">{STORE_COPY.unpublished}</span>;
  }
  return (
    <button
      type="button"
      class={`btn btn-sm shrink-0 ${primary ? 'btn-primary' : ''}`}
      onClick={() => installListing(listing)}>
      {label}
    </button>
  );
}

export function kindOrAuthor(listing: CatalogAppListing): string {
  return listingKindLabel(listing) ?? listing.app.author;
}

export function ShelfHeader({ title, href, trailing }: { title: string; href?: string; trailing?: ComponentChildren }) {
  return (
    <header class="mb-4 flex items-center justify-between gap-3 border-b border-white/20 pb-2">
      <h2 class="m-0 text-xl">{title}</h2>
      <span class="flex items-center gap-4">
        {href ? (
          <a href={href} class="border-b-0 font-mono text-sm text-white/50 hover:text-white">
            see all
          </a>
        ) : null}
        {trailing}
      </span>
    </header>
  );
}

export function Rail({ rail, children }: { rail: RailState; children: ComponentChildren }) {
  return (
    <div
      ref={rail.ref}
      class={cx('-mx-6 flex snap-x snap-mandatory scroll-px-6 gap-4 px-6 pb-2 sm:gap-6', RAIL_SCROLL)}>
      {children}
    </div>
  );
}

export function RailShelf({
  title,
  href,
  class: extra = 'mb-14',
  children,
}: {
  title: string;
  href?: string;
  class?: string;
  children: ComponentChildren;
}) {
  const rail = useRail();
  return (
    <section class={extra}>
      <ShelfHeader title={title} href={href} trailing={<RailArrows rail={rail} />} />
      <Rail rail={rail}>{children}</Rail>
    </section>
  );
}

export function IconTile({ listing }: { listing: CatalogAppListing }) {
  return (
    <a href={appDetailPath(listing.app.id)} class="group w-20 shrink-0 snap-start border-b-0">
      <AppIcon app={listing.app} size={80} class="group-hover:border-white/30" />
      <p class="m-0 mt-2 truncate text-sm group-hover:text-white">{listing.app.name}</p>
      <p class="m-0 truncate font-mono text-xs text-white/40">{kindOrAuthor(listing)}</p>
    </a>
  );
}

export function ShotCard({ listing, meta }: { listing: CatalogAppListing; meta: string }) {
  return (
    <a href={appDetailPath(listing.app.id)} class="group block min-w-0 border-b-0">
      <AppFrame app={listing.app} shot={firstShot(listing.app)} size="sm" />
      <div class="mt-3 flex items-center gap-2.5">
        <AppIcon app={listing.app} size={32} />
        <div class="min-w-0">
          <p class="m-0 truncate text-sm group-hover:text-white">{listing.app.name}</p>
          <p class="m-0 truncate font-mono text-xs text-white/40">{meta}</p>
        </div>
      </div>
    </a>
  );
}

export function ShotTile({ listing, meta }: { listing: CatalogAppListing; meta: string }) {
  return (
    <div class="w-64 shrink-0 snap-start sm:w-[calc((100%-3rem)/2.25)] lg:w-[calc((100%-6rem)/4.25)]">
      <ShotCard listing={listing} meta={meta} />
    </div>
  );
}

export function RankRow({
  listing,
  rank,
  class: extra,
}: {
  listing: CatalogAppListing;
  rank?: number;
  class?: string;
}) {
  const sub = [listing.app.author, installsLabel(listing.installs)].filter(Boolean).join(' · ');
  return (
    <li class={cx('flex min-w-0 items-center gap-3 px-3 py-2.5 sm:px-4', extra)}>
      {rank === undefined ? null : (
        <span class="w-5 shrink-0 font-mono text-sm text-white/35 tabular-nums">{rank}</span>
      )}
      <a href={appDetailPath(listing.app.id)} class="group flex min-w-0 flex-1 items-center gap-3 border-b-0">
        <AppIcon app={listing.app} size={44} />
        <span class="min-w-0">
          <span class="block truncate group-hover:text-white">{listing.app.name}</span>
          <span class="block truncate text-sm text-white/45">{sub}</span>
        </span>
      </a>
      <GetButton listing={listing} label="get" />
    </li>
  );
}

const SEARCH_ICON = (
  <svg viewBox="0 0 24 24" aria-hidden="true" class="size-4 shrink-0 fill-none stroke-current stroke-[1.6]">
    <circle cx="11" cy="11" r="6" />
    <path d="M16 16l4 4" />
  </svg>
);

export function SearchBox({ value, onInput }: { value: string; onInput?: (text: string) => void }) {
  return (
    <form
      action={SEARCH_PATH}
      method="get"
      role="search"
      class="flex items-center gap-2 border border-white/25 px-3 text-white/40 focus-within:border-white/50">
      {SEARCH_ICON}
      <input
        type="search"
        name="q"
        value={value}
        aria-label="search apps"
        placeholder="search"
        autocomplete="off"
        spellcheck={false}
        onInput={onInput ? event => onInput((event.target as HTMLInputElement).value) : undefined}
        class="min-w-0 flex-1 bg-transparent py-2.5 font-mono text-sm text-white placeholder:text-white/35 focus:outline-none"
      />
    </form>
  );
}

export type Chip = { key: string; label: string; href: string; count?: number };

export function activeChip(filter: StoreFilter | null): string {
  return filter ? filterKey(filter) : 'all';
}

export function chipsFor(total: number, filters: FilterChip[]): Chip[] {
  return [
    { key: 'all', label: 'all', href: STORE_PATH, count: total },
    ...filters.map(entry => ({ key: entry.key, label: entry.label, href: filterPath(entry.filter) })),
  ];
}

export function Chips({ chips, active }: { chips: Chip[]; active: string }) {
  return (
    <nav
      aria-label="categories"
      class={cx('-mx-6 flex gap-2 px-6 pb-1 sm:mx-0 sm:flex-wrap sm:overflow-visible sm:px-0', RAIL_SCROLL)}>
      {chips.map(chip => (
        <a
          key={chip.key}
          href={chip.href}
          aria-current={chip.key === active ? 'page' : undefined}
          class={`flex shrink-0 gap-1.5 border px-3 py-1.5 font-mono text-xs whitespace-nowrap ${
            chip.key === active
              ? 'border-accent text-accent bg-accent-soft'
              : 'border-white/15 text-white/60 hover:border-white/40 hover:text-white'
          }`}>
          {chip.label}
          {chip.count === undefined ? null : <span class="tabular-nums opacity-55">{chip.count}</span>}
        </a>
      ))}
    </nav>
  );
}

export function Notice({ children }: { children: ComponentChildren }) {
  return (
    <div class="border border-dashed border-white/25 p-16 text-center">
      <p class="m-0 text-white/60">{children}</p>
    </div>
  );
}
