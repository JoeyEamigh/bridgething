import { useMemo } from 'preact/hooks';
import {
  installsLabel,
  listingKindLabel,
  storefront,
  type CatalogAppListing,
  type Shelf as ShelfModel,
  STORE_COPY,
} from '@bridgething/catalog';
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
  RankRow,
  Row,
  SearchBox,
  ShelfHeader,
  ShotRow,
} from './parts';

function Hero({ listing }: { listing: CatalogAppListing }) {
  const app = listing.app;
  return (
    <section class="mb-14 grid grid-cols-1 items-center gap-6 lg:grid-cols-[3fr_2fr] lg:gap-12">
      <a href={appDetailPath(app.id)} class="block border-b-0">
        <AppFrame app={app} shot={firstShot(app)} size="lg" eager />
      </a>
      <div class="flex flex-col gap-4">
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
    </section>
  );
}

function shotMeta(shelf: ShelfModel): (listing: CatalogAppListing) => string {
  if (shelf.id === 'launchers' || shelf.id === 'overlays') {
    return listing => installsLabel(listing.installs) ?? listing.app.author;
  }
  if (shelf.id === 'updated') return listing => listing.newestCompatible?.version ?? kindOrAuthor(listing);
  return kindOrAuthor;
}

function Shelf({ shelf }: { shelf: ShelfModel }) {
  const href = browsePath(shelf.more);
  if (shelf.id === 'essentials') {
    return (
      <section class="mb-14">
        <ShelfHeader title={shelf.title} href={href} />
        <Row>
          {shelf.listings.map(listing => (
            <IconTile key={listing.app.id} listing={listing} />
          ))}
        </Row>
      </section>
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
  return (
    <section class="mb-14">
      <ShelfHeader title={shelf.title} href={href} />
      <ShotRow listings={shelf.listings} meta={shotMeta(shelf)} />
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

  const chips = useMemo(() => (catalog && front ? chipsFor(catalog.listings, front.categories) : []), [catalog, front]);

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
      ) : front.shelves.length === 0 ? (
        <Notice>{STORE_COPY.empty}</Notice>
      ) : (
        <>
          {front.featured ? <Hero listing={front.featured} /> : null}
          {front.shelves.map(shelf => (
            <Shelf key={shelf.id} shelf={shelf} />
          ))}
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
