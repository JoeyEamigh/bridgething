import {
  filterKey,
  installsLabel,
  listingKindLabel,
  webHref,
  type CatalogAppListing,
  type FilterChip,
  type StoreFilter,
} from '@bridgething/catalog';
import {
  Carousel,
  cx,
  DeviceFrame,
  RAIL_SCROLL,
  RailArrows,
  SectionHeader,
  SLIDE_ART,
  SLIDE_TEXT,
  Spinner,
  useRail,
} from '@bridgething/ui';
import frame from '@bridgething/ui/car-thing-frame.webp';
import type { ComponentChildren, VNode } from 'preact';

import { CatalogIcon } from '../lib/webapp-icon.tsx';
import { PATHS } from '../routes.ts';
import { InstallButton, useInstaller } from './Install.tsx';

export type ListingHref = (listing: CatalogAppListing) => string;

const toApp: ListingHref = listing => PATHS.storeApp(listing.app.id);

const LINK =
  'focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 transition-colors duration-150';

export function Screenshot({
  listing,
  src,
  compact = false,
  class: className,
}: {
  listing: CatalogAppListing;
  src?: string | null;
  compact?: boolean;
  class?: string;
}): VNode {
  const shot = src === undefined ? webHref(listing.app.screenshots?.[0]) : src;

  return (
    <DeviceFrame frame={frame} class={className}>
      {shot ? (
        <img src={shot} alt="" loading="lazy" class="size-full object-cover" />
      ) : (
        <span class="flex size-full items-center justify-center">
          <CatalogIcon url={listing.app.icon} name={listing.app.name} size={compact ? 'md' : 'lg'} />
        </span>
      )}
    </DeviceFrame>
  );
}

export function Chips({
  active,
  total,
  filters,
  action,
}: {
  active: StoreFilter | null;
  total: number;
  filters: FilterChip[];
  action?: ComponentChildren;
}): VNode {
  const current = active ? filterKey(active) : 'all';
  const chips: { key: string; label: string; href: string; count?: number }[] = [
    { key: 'all', label: 'all', count: total, href: PATHS.store },
    ...filters.map(entry => ({
      key: entry.key,
      label: entry.label,
      href: PATHS.storeBrowse({ filter: entry.filter, sort: 'popular' }),
    })),
  ];

  return (
    <div class="mb-8 flex items-start gap-4">
      <nav aria-label="categories" class="flex min-w-0 flex-1 flex-wrap gap-1.5">
        {chips.map(chip => (
          <a
            key={chip.key}
            href={chip.href}
            aria-current={chip.key === current ? 'page' : undefined}
            class={cx(
              'flex gap-1.5 border px-2.5 py-1 font-mono text-hint whitespace-nowrap',
              LINK,
              chip.key === current
                ? 'border-accent bg-accent-soft text-accent'
                : 'border-rule text-soft hover:border-edge hover:text-off-white',
            )}>
            {chip.label}
            {chip.count === undefined ? null : <span class="opacity-55 tabular-nums">{chip.count}</span>}
          </a>
        ))}
      </nav>
      {action}
    </div>
  );
}

export function RefreshAction({ pending, onRefresh }: { pending: boolean; onRefresh: () => void }): VNode {
  return (
    <button
      type="button"
      class="flex shrink-0 items-center gap-1.5 px-1 py-1 font-mono text-hint text-soft uppercase transition-colors hover:text-off-white focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 disabled:text-dim"
      disabled={pending}
      aria-busy={pending || undefined}
      onClick={onRefresh}>
      {pending ? <Spinner class="size-3" /> : null}
      refresh
    </button>
  );
}

function FeaturedSlide({ listing }: { listing: CatalogAppListing }): VNode {
  const installer = useInstaller(listing);
  const { app } = listing;
  const href = toApp(listing);
  const byline = [app.author, listingKindLabel(listing)].filter(Boolean).join(' · ');

  return (
    <div class="grid gap-x-7 gap-y-4 @2xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] @2xl:items-end">
      <a href={href} class={cx('block min-w-0 hover:opacity-90', LINK, SLIDE_ART)}>
        <Screenshot listing={listing} />
      </a>
      <div class={cx('flex min-w-0 flex-col gap-3', SLIDE_TEXT)}>
        <span class="font-mono text-eyebrow tracking-[0.18em] text-accent uppercase">featured</span>
        <a href={href} class={cx('flex min-w-0 flex-col gap-1 hover:text-accent', LINK)}>
          <span class="font-display text-title font-medium tracking-display wrap-break-word @2xl:text-[1.75rem]">
            {app.name}
          </span>
          <span class="truncate text-body text-muted">{byline}</span>
        </a>
        <p class="m-0 line-clamp-3 text-body leading-relaxed text-soft">{app.description}</p>
        <div class="flex items-center gap-3 pt-1">
          <InstallButton listing={listing} installer={installer} />
        </div>
        {installer.failure ? <p class="m-0 text-hint text-err">{installer.failure}</p> : null}
      </div>
      {installer.dialog}
    </div>
  );
}

export function Featured({ listings }: { listings: CatalogAppListing[] }): VNode {
  return (
    <div class="@container mb-10">
      <Carousel
        items={listings}
        label="featured apps"
        keyOf={listing => `${listing.sourceUrl}:${listing.app.id}`}
        render={listing => <FeaturedSlide listing={listing} />}
      />
    </div>
  );
}

export function RailSection({
  title,
  onSeeAll,
  children,
}: {
  title: string;
  onSeeAll: () => void;
  children: ComponentChildren;
}): VNode {
  const rail = useRail();
  return (
    <section class="mb-9">
      <SectionHeader title={title} trailing={<RailArrows rail={rail} />} action="see all" onAction={onSeeAll} />
      <div ref={rail.ref} class={cx('flex snap-x gap-4 pb-3', RAIL_SCROLL)}>
        {children}
      </div>
    </section>
  );
}

export function AppTiles({ listings, href = toApp }: { listings: CatalogAppListing[]; href?: ListingHref }): VNode[] {
  return listings.map(listing => (
    <a
      key={`${listing.sourceUrl}:${listing.app.id}`}
      href={href(listing)}
      class={cx('group flex w-20 shrink-0 snap-start flex-col gap-2', LINK)}>
      <CatalogIcon
        url={listing.app.icon}
        name={listing.app.name}
        size="xl"
        class="border border-rule transition-colors group-hover:border-edge"
      />
      <span class="flex min-w-0 flex-col">
        <span class="truncate text-body text-off-white">{listing.app.name}</span>
        <span class="truncate font-mono text-eyebrow text-dim">{listingKindLabel(listing) ?? ''}</span>
      </span>
    </a>
  ));
}

export function ShelfCards({
  listings,
  meta,
  href = toApp,
}: {
  listings: CatalogAppListing[];
  meta: (listing: CatalogAppListing) => string | null;
  href?: ListingHref;
}): VNode[] {
  return listings.map(listing => (
    <AppCard
      key={`${listing.sourceUrl}:${listing.app.id}`}
      listing={listing}
      meta={meta(listing)}
      href={href(listing)}
      class="w-64 shrink-0 snap-start"
    />
  ));
}

export function CardGrid({
  listings,
  meta,
  href = toApp,
}: {
  listings: CatalogAppListing[];
  meta: (listing: CatalogAppListing) => string | null;
  href?: ListingHref;
}): VNode {
  return (
    <div class="grid grid-cols-[repeat(auto-fill,minmax(15rem,1fr))] gap-x-4 gap-y-6">
      {listings.map(listing => (
        <AppCard
          key={`${listing.sourceUrl}:${listing.app.id}`}
          listing={listing}
          meta={meta(listing)}
          href={href(listing)}
        />
      ))}
    </div>
  );
}

function AppCard({
  listing,
  meta,
  href,
  class: className,
}: {
  listing: CatalogAppListing;
  meta: string | null;
  href: string;
  class?: string;
}): VNode {
  const { app } = listing;

  return (
    <a href={href} class={cx('group flex min-w-0 flex-col gap-2.5', LINK, className)}>
      <Screenshot listing={listing} compact class="transition-opacity group-hover:opacity-85" />
      <span class="flex min-w-0 items-center gap-2.5">
        <CatalogIcon url={app.icon} name={app.name} size="sm" class="border border-rule" />
        <span class="flex min-w-0 flex-col">
          <span class="truncate text-body text-off-white">{app.name}</span>
          {meta ? <span class="truncate font-mono text-eyebrow text-dim">{meta}</span> : null}
        </span>
      </span>
    </a>
  );
}

export function AppColumns({
  listings,
  ranked = false,
  href = toApp,
}: {
  listings: CatalogAppListing[];
  ranked?: boolean;
  href?: ListingHref;
}): VNode {
  const half = Math.ceil(listings.length / 2);
  const columns = [listings.slice(0, half), listings.slice(half)].filter(column => column.length > 0);

  return (
    <div class="@container">
      <div class="grid gap-3 @2xl:grid-cols-2">
        {columns.map((column, index) => (
          <div key={index} class="flex flex-col border border-rule bg-screen">
            {column.map((listing, offset) => (
              <AppRow
                key={`${listing.sourceUrl}:${listing.app.id}`}
                listing={listing}
                rank={ranked ? index * half + offset + 1 : undefined}
                href={href(listing)}
                detail={[listing.app.author, installsLabel(listing.installs)].filter(Boolean).join(' · ')}
              />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

export function AppList({ listings, href = toApp }: { listings: CatalogAppListing[]; href?: ListingHref }): VNode {
  return (
    <div class="flex flex-col border border-rule bg-screen">
      {listings.map(listing => (
        <AppRow
          key={`${listing.sourceUrl}:${listing.app.id}`}
          listing={listing}
          href={href(listing)}
          detail={[listing.app.author, listingKindLabel(listing), installsLabel(listing.installs)]
            .filter(Boolean)
            .join(' · ')}
        />
      ))}
    </div>
  );
}

function AppRow({
  listing,
  rank,
  href,
  detail,
}: {
  listing: CatalogAppListing;
  rank?: number;
  href: string;
  detail: string;
}): VNode {
  const installer = useInstaller(listing);
  const { app } = listing;

  return (
    <div class="border-t border-rule first:border-t-0">
      <div class="flex min-w-0 items-center gap-3 pr-4 transition-colors hover:bg-neutral-soft">
        <a
          href={href}
          class="flex min-w-0 flex-1 items-center gap-3 py-2.5 pl-4 focus-visible:outline-2 focus-visible:outline-accent focus-visible:-outline-offset-2">
          {rank === undefined ? null : <span class="w-5 shrink-0 text-right font-mono text-body text-dim">{rank}</span>}
          <CatalogIcon url={app.icon} name={app.name} size="md" class="border border-rule" />
          <span class="flex min-w-0 flex-1 flex-col gap-0.5">
            <span class="truncate text-row text-off-white">{app.name}</span>
            <span class="truncate text-hint text-muted">{installer.failure ?? detail}</span>
          </span>
        </a>
        <InstallButton listing={listing} installer={installer} size="sm" quiet />
      </div>
      {installer.dialog}
    </div>
  );
}
