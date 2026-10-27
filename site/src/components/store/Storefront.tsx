import { useMemo } from 'preact/hooks';
import {
  installsLabel,
  listingKindLabel,
  storefront,
  type CatalogAppListing,
  type Shelf as ShelfModel,
  STORE_COPY,
} from '@bridgething/catalog';
import { Carousel, cx, SLIDE_ART, SLIDE_TEXT } from '@bridgething/ui';
import { appDetailPath, browsePath, SOURCES_PATH } from '../../lib/app-routes';
import type { StoreData } from '../../lib/directory-client';
import { useStore } from '../../lib/use-store';
import {
  AppIcon,
  Chips,
  chipsFor,
  AppFrame,
  firstShot,
  GetButton,
  IconTile,
  kindOrAuthor,
  Notice,
  RailShelf,
  RankRow,
  SearchBox,
  ShelfHeader,
  ShotTile,
} from './parts';

function Slide({ listing, first }: { listing: CatalogAppListing; first: boolean }) {
  const app = listing.app;
  return (
    <div class="grid grid-cols-1 items-center gap-6 lg:grid-cols-[3fr_2fr] lg:gap-12">
      <a href={appDetailPath(app.id)} class={cx('block border-b-0', SLIDE_ART)}>
        <AppFrame app={app} shot={firstShot(app)} size="lg" eager={first} />
      </a>
      <div class={cx('flex flex-col gap-4', SLIDE_TEXT)}>
        <p class="text-accent m-0 font-mono text-sm">featured</p>
        <div class="flex items-center gap-4">
          <AppIcon app={app} size={56} />
          <div class="min-w-0">
            <h2 class="m-0 text-3xl">
              <a href={appDetailPath(app.id)} class="border-b-0 hover:text-white">
                {app.name}
              </a>
            </h2>
            <p class="m-0 mt-1 truncate font-mono text-sm text-white/45">
              {[app.author, listingKindLabel(listing)].filter(Boolean).join(' · ')}
            </p>
          </div>
        </div>
        <p class="m-0 line-clamp-3 max-w-[52ch] text-white/65">{app.description}</p>
        <div class="flex flex-wrap items-center gap-4">
          <GetButton listing={listing} label="install" primary />
          {installsLabel(listing.installs) ? (
            <span class="font-mono text-sm text-white/40">{installsLabel(listing.installs)}</span>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function Hero({ listings }: { listings: CatalogAppListing[] }) {
  const first = listings[0];
  return (
    <Carousel
      items={listings}
      label="featured apps"
      keyOf={listing => `${listing.sourceUrl} ${listing.app.id}`}
      render={listing => <Slide listing={listing} first={listing === first} />}
      class="mb-14"
    />
  );
}

function shotMeta(shelf: ShelfModel): (listing: CatalogAppListing) => string {
  if (shelf.id === 'updated') return listing => listing.newestCompatible?.version ?? kindOrAuthor(listing);
  if (shelf.more.filter !== null) return listing => installsLabel(listing.installs) ?? listing.app.author;
  return kindOrAuthor;
}

function Shelf({ shelf }: { shelf: ShelfModel }) {
  const href = browsePath(shelf.more);
  if (shelf.id === 'essentials') {
    return (
      <RailShelf title={shelf.title} href={href}>
        {shelf.listings.map(listing => (
          <IconTile key={listing.app.id} listing={listing} />
        ))}
      </RailShelf>
    );
  }
  if (shelf.id === 'popular') {
    return (
      <section class="mb-14">
        <ShelfHeader title={shelf.title} href={href} />
        <div class="bg-screen grid grid-cols-1 border border-white/15 md:grid-cols-2">
          {[shelf.listings.slice(0, 5), shelf.listings.slice(5)].map((column, half) =>
            column.length > 0 ? (
              <ol
                key={half}
                start={half * 5 + 1}
                class="m-0 list-none p-0 not-first:hidden md:not-first:block md:not-first:border-l md:not-first:border-white/10 [&>li+li]:border-t [&>li+li]:border-white/10">
                {column.map((listing, index) => (
                  <RankRow key={listing.app.id} listing={listing} rank={half * 5 + index + 1} />
                ))}
              </ol>
            ) : null,
          )}
        </div>
      </section>
    );
  }
  const meta = shotMeta(shelf);
  return (
    <RailShelf title={shelf.title} href={href}>
      {shelf.listings.map(listing => (
        <ShotTile key={listing.app.id} listing={listing} meta={meta(listing)} />
      ))}
    </RailShelf>
  );
}

function AllApps({ listings }: { listings: CatalogAppListing[] }) {
  return (
    <section class="mb-14">
      <ShelfHeader title="all apps" />
      <ul class="bg-screen m-0 grid list-none grid-cols-1 border-t border-l border-white/10 p-0 md:grid-cols-2 lg:grid-cols-3">
        {listings.map(listing => (
          <RankRow key={listing.app.id} listing={listing} class="border-r border-b border-white/10" />
        ))}
      </ul>
    </section>
  );
}

export function Storefront({ data }: { data?: StoreData | null }) {
  const { data: loaded, catalog, failed } = useStore(data);

  const front = useMemo(
    () =>
      catalog && loaded
        ? storefront({ listings: catalog.listings, pin: loaded.apps.featured, now: new Date(loaded.apps.updated_at) })
        : null,
    [catalog, loaded],
  );

  const chips = useMemo(() => (front ? chipsFor(front.all.length, front.filters) : []), [front]);

  return (
    <>
      <header class="mb-10 flex flex-col gap-5">
        <div class="flex flex-wrap items-end justify-between gap-x-8 gap-y-4">
          <h1 class="m-0">apps</h1>
          <div class="w-full sm:w-80">
            <SearchBox value="" />
          </div>
        </div>
        {chips.length > 1 ? <Chips chips={chips} active="all" /> : null}
      </header>

      {front === null ? (
        <Notice>{failed ? 'could not reach the catalog. try reloading the page.' : 'loading…'}</Notice>
      ) : front.all.length === 0 ? (
        <Notice>{STORE_COPY.empty}</Notice>
      ) : (
        <>
          {front.featured.length > 0 ? <Hero listings={front.featured} /> : null}
          {front.shelves.map(shelf => (
            <Shelf key={shelf.id} shelf={shelf} />
          ))}
          <AllApps listings={front.all} />
        </>
      )}

      <footer class="flex flex-wrap items-center justify-between gap-4 border-t border-white/15 pt-4 font-mono text-sm text-white/45">
        <span>apps come from community sources</span>
        <span class="flex gap-5">
          <a href={SOURCES_PATH}>sources</a>
          <a href="/docs/publishing-apps">publish an app</a>
        </span>
      </footer>
    </>
  );
}
