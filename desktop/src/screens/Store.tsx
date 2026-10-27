import {
  browse,
  changelogText,
  failureLine,
  installsLabel,
  listingKindLabel,
  normalizeSourceUrl,
  OFFICIAL_CATALOG_URL,
  queryTitle,
  recommendedSources,
  search,
  SORT_LABELS,
  STORE_COPY,
  storeFilters,
  storefront,
  type AppExtension,
  type CatalogAppListing,
  type CatalogSnapshot,
  type Shelf,
  type StoreSort,
} from '@bridgething/catalog';
import {
  Button,
  describeError,
  Field,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionEmpty,
  SectionHeader,
  Segmented,
  Spinner,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';
import { useMemo, useState } from 'preact/hooks';

import { ConfirmInstall, InstallButton, useInstaller } from '../components/Install.tsx';
import { BackButton, Disclosure, ErrorNote, Hint, Screen, Section } from '../components/Screen.tsx';
import {
  AppColumns,
  AppList,
  AppTiles,
  CardGrid,
  Chips,
  Featured,
  RailSection,
  RefreshAction,
  ShelfCards,
} from '../components/Storefront.tsx';
import { useDesktop } from '../desktop.ts';
import { sideloadConsent } from '../lib/extension.ts';
import { since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { pickArtifact } from '../lib/picker.ts';
import { CatalogIcon } from '../lib/webapp-icon.tsx';
import { browseQuery, PATHS } from '../routes.ts';
import { sourceListings, storeListings, type StoreListings } from '../stores/catalog.ts';
import { catalogSources } from '../stores/session.ts';

type Meta = (listing: CatalogAppListing) => string | null;

const META: Record<StoreSort, Meta> = {
  popular: listing => installsLabel(listing.installs) ?? listingKindLabel(listing),
  trending: listing => installsLabel(listing.installs) ?? listingKindLabel(listing),
  new: listing => listingKindLabel(listing),
  updated: listing =>
    listing.newestCompatible
      ? `v${listing.newestCompatible.version} · ${since(listing.newestCompatible.released_at)}`
      : listingKindLabel(listing),
};

const SORT_OPTIONS = (['popular', 'new', 'updated'] as const).map(value => ({ value, label: SORT_LABELS[value] }));

export function StoreRoute(): VNode {
  const store = storeListings();
  const [query, setQuery] = useState('');
  const searching = query.trim().length > 0;

  return (
    <Screen>
      <ScreenHeader
        title="store"
        trailing={
          <Field
            value={query}
            onInput={setQuery}
            icon={<Icon name="search" />}
            placeholder="search"
            clearable
            class="w-60"
          />
        }
      />

      {searching ? <SearchResults listings={store.listings} query={query} /> : <StoreHome store={store} />}

      {store.snapshot.failures.length > 0 ? <Hint>{failureLine(store.snapshot.failures.length)}</Hint> : null}
      {store.error ? <ErrorNote>{store.error}</ErrorNote> : null}

      <div class="mt-12">
        <Disclosure title="sources">
          <MySources subscribed={store.subscribed} pending={catalogSources.pending.value} />
          <SuggestedSources snapshot={store.snapshot} subscribed={store.subscribed} />
          <BrowseByUrl />
        </Disclosure>

        <Disclosure title="install from a file">
          <SideloadBundle />
        </Disclosure>
      </div>
    </Screen>
  );
}

function StoreHome({ store }: { store: StoreListings }): VNode {
  const { listings, pin, pending, refresh } = store;
  const front = useMemo(() => storefront({ listings, pin, now: new Date() }), [listings, pin]);
  const updates = listings.filter(listing => listing.updateAvailable);

  return (
    <>
      <Chips
        active={null}
        total={front.all.length}
        filters={front.filters}
        action={<RefreshAction pending={pending} onRefresh={refresh} />}
      />

      {listings.length === 0 ? (
        <SectionEmpty>{pending ? <Spinner class="mx-auto" /> : STORE_COPY.empty}</SectionEmpty>
      ) : (
        <>
          {front.featured.length > 0 ? <Featured listings={front.featured} /> : null}

          {updates.length > 0 ? (
            <Section>
              <SectionHeader title="updates" />
              <ListGroup>
                {updates.map(listing => (
                  <UpdateRow key={listing.app.id} listing={listing} />
                ))}
              </ListGroup>
            </Section>
          ) : null}

          {front.shelves.map(shelf => (
            <ShelfSection key={shelf.id} shelf={shelf} />
          ))}

          <Section>
            <SectionHeader title="all apps" />
            <AppColumns listings={front.all} />
          </Section>
        </>
      )}
    </>
  );
}

function ShelfSection({ shelf }: { shelf: Shelf }): VNode {
  const { route } = useLocation();
  const seeAll = () => route(PATHS.storeBrowse(shelf.more));

  if (shelf.id === 'popular') {
    return (
      <Section>
        <SectionHeader title={shelf.title} action="see all" onAction={seeAll} />
        <AppColumns listings={shelf.listings} ranked />
      </Section>
    );
  }
  return (
    <RailSection title={shelf.title} onSeeAll={seeAll}>
      {shelf.id === 'essentials' ? (
        <AppTiles listings={shelf.listings} />
      ) : (
        <ShelfCards listings={shelf.listings} meta={META[shelf.more.sort]} />
      )}
    </RailSection>
  );
}

function SearchResults({ listings, query }: { listings: CatalogAppListing[]; query: string }): VNode {
  const results = search(listings, query);

  return (
    <Section>
      <SectionHeader title="results" />
      {results.length === 0 ? <SectionEmpty>{STORE_COPY.noMatches}</SectionEmpty> : <AppList listings={results} />}
    </Section>
  );
}

export function StoreBrowseRoute({ query }: { query: Record<string, string> }): VNode {
  const { route } = useLocation();
  const { listings, pending } = storeListings();
  const browsing = browseQuery(query);
  const { filter, sort } = browsing;
  const filters = useMemo(() => storeFilters(listings), [listings]);
  const shown = browse(listings, browsing);
  const narrowed = filter !== null && filter.kind !== 'author';

  return (
    <Screen>
      <BackButton>back</BackButton>
      <ScreenHeader
        title={queryTitle(browsing)}
        trailing={
          narrowed ? (
            <Segmented<StoreSort>
              label="sort"
              size="sm"
              options={SORT_OPTIONS}
              value={sort}
              onChange={next => route(PATHS.storeBrowse({ filter, sort: next }), true)}
            />
          ) : undefined
        }
      />

      {narrowed ? <Chips active={filter} total={listings.length} filters={filters} /> : null}

      {shown.length === 0 ? (
        <SectionEmpty>{pending ? <Spinner class="mx-auto" /> : STORE_COPY.empty}</SectionEmpty>
      ) : (
        <CardGrid listings={shown} meta={META[sort]} />
      )}
    </Screen>
  );
}

function UpdateRow({ listing }: { listing: CatalogAppListing }): VNode {
  const installer = useInstaller(listing);
  const { app, newestCompatible, installedVersion } = listing;

  return (
    <>
      <ListRow
        icon={<CatalogIcon url={app.icon} name={app.name} size="sm" />}
        title={app.name}
        subtitle={installer.failure ?? changelogText(newestCompatible?.changelog) ?? undefined}
        value={`${installedVersion} → ${newestCompatible?.version}`}
        trailing={<InstallButton listing={listing} installer={installer} size="sm" />}
      />
      {installer.dialog}
    </>
  );
}

function SuggestedSources({ snapshot, subscribed }: { snapshot: CatalogSnapshot; subscribed: string[] }): VNode {
  const { route } = useLocation();
  const suggested = recommendedSources({
    directory: snapshot.directory,
    orderedCatalogs: snapshot.catalogs,
    subscribed,
  });

  return (
    <Section>
      <SectionHeader title={STORE_COPY.suggestedTitle} />
      {suggested.length === 0 ? (
        <SectionEmpty>{STORE_COPY.suggestedEmpty}</SectionEmpty>
      ) : (
        <ListGroup>
          {suggested.map(source => (
            <ListRow
              key={source.url}
              icon={<Icon name="store" />}
              title={source.name}
              subtitle={source.description ?? source.url}
              trailing={source.attested ? <Pill tone="ok">attested</Pill> : undefined}
              chevron
              onClick={() => route(PATHS.storeSource(source.url))}
            />
          ))}
        </ListGroup>
      )}
    </Section>
  );
}

function MySources({ subscribed, pending }: { subscribed: string[]; pending: boolean }): VNode {
  const { route } = useLocation();

  return (
    <Section>
      <SectionHeader title="my sources" />
      <ListGroup>
        {subscribed.map(url => {
          const official = url === OFFICIAL_CATALOG_URL;
          return (
            <ListRow
              key={url}
              icon={<Icon name="store" />}
              iconTint={official ? 'accent' : 'default'}
              title={official ? 'bridgething' : url}
              subtitle={official ? url : undefined}
              chevron
              disabled={pending}
              onClick={() => route(PATHS.storeSource(url))}
            />
          );
        })}
      </ListGroup>
    </Section>
  );
}

export function SourceRoute({ source }: { source: string }): VNode {
  const url = decodeURIComponent(source);
  const { listings, catalog } = sourceListings(url);
  const held = catalog.data.value;
  const pending = catalog.pending.value;

  return (
    <Screen>
      <BackButton to={PATHS.store}>store</BackButton>
      <ScreenHeader
        title={held?.repo.name ?? url}
        subtitle={held?.repo.description ?? undefined}
        trailing={<SubscribeToggle url={url} />}
      />

      <Section>
        <SectionHeader title="apps" action="refresh" pending={pending} onAction={catalog.refresh} />
        {catalog.error.value ? (
          <ErrorNote>{catalog.error.value}</ErrorNote>
        ) : pending && listings.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : listings.length === 0 ? (
          <SectionEmpty>{STORE_COPY.empty}</SectionEmpty>
        ) : (
          <CardGrid
            listings={listings}
            meta={META.popular}
            href={listing => PATHS.storeSourceApp(url, listing.app.id)}
          />
        )}
      </Section>
    </Screen>
  );
}

function SubscribeToggle({ url }: { url: string }): VNode {
  const session = useDesktop();
  const [busy, setBusy] = useState(false);

  const held = catalogSources.data.value.includes(url);

  const toggle = async () => {
    setBusy(true);
    try {
      if (held) await session.removeCatalogSource(url);
      else await session.addCatalogSource(url);
      await catalogSources.refresh();
    } finally {
      setBusy(false);
    }
  };

  return (
    <Button
      size="sm"
      variant={held ? 'secondary' : 'primary'}
      icon={<Icon name={held ? 'trash' : 'plus'} size={14} />}
      loading={busy || catalogSources.pending.value}
      onClick={() => void toggle()}>
      {held ? 'remove source' : 'add source'}
    </Button>
  );
}

function BrowseByUrl(): VNode {
  const { route } = useLocation();
  const [url, setUrl] = useState('');
  const [failure, setFailure] = useState<string | null>(null);

  const open = (value: string) => {
    try {
      const normalized = normalizeSourceUrl(value);
      setFailure(null);
      route(PATHS.storeSource(normalized));
    } catch (reason) {
      setFailure(describeError(reason));
    }
  };

  return (
    <Section>
      <SectionHeader title="add by url" />
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          value={url}
          onInput={setUrl}
          onSubmit={open}
          icon={<Icon name="link" />}
          type="url"
          placeholder="https://example.com/catalog.json"
          clearable
        />
        <Button disabled={url.trim().length === 0} onClick={() => open(url)}>
          browse
        </Button>
      </div>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}

function SideloadBundle(): VNode {
  const session = useDesktop();
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<{ bundle: string; extension: AppExtension } | null>(null);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const install = async (bundle: string, confirmed?: string[]) => {
    setBusy(true);
    setConfirming(null);
    setOutcome(null);
    setFailure(null);
    try {
      const answer = await session.otaInstallWebapp(bundle, undefined, confirmed);
      if (answer.kind === 'installed') setOutcome('installed');
      else setFailure(answer.reason);
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(false);
    }
  };

  const start = async (bundle: string) => {
    setBusy(true);
    setOutcome(null);
    setFailure(null);
    let extension: AppExtension | null;
    try {
      extension = sideloadConsent(await session.webappBundleExtension(bundle));
    } catch (reason) {
      setFailure(describeError(reason));
      setBusy(false);
      return;
    }
    setBusy(false);
    if (extension) setConfirming({ bundle, extension });
    else await install(bundle);
  };

  const browse = async () => {
    const picked = await pickArtifact('webapp');
    if (!picked) return;
    setPath(picked);
    await start(picked);
  };

  return (
    <Section>
      <SectionHeader title="webapp zip" />
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          value={path}
          onInput={setPath}
          onSubmit={value => value.trim() && void start(value.trim())}
          icon={<Icon name="file" />}
          placeholder="/path/to/my-webapp.zip"
          clearable
        />
        <Button icon={<Icon name="upload" />} loading={busy} onClick={() => void browse()}>
          choose
        </Button>
      </div>
      {outcome ? <Hint>{outcome}</Hint> : null}
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      <ConfirmInstall
        open={confirming !== null}
        title="install this bundle?"
        extension={confirming?.extension ?? null}
        source={null}
        older={false}
        busy={busy}
        onCancel={() => setConfirming(null)}
        onConfirm={() => {
          if (confirming) void install(confirming.bundle, confirming.extension.permissions);
        }}
      />
    </Section>
  );
}
