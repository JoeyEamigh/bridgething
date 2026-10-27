import { useMemo, useState } from 'preact/hooks';
import {
  browse,
  installsLabel,
  listingKindLabel,
  queryTitle,
  search,
  SORT_LABELS,
  STORE_COPY,
  storeFilters,
  type CatalogAppListing,
  type StoreSort,
} from '@bridgething/catalog';
import { SORTS, STORE_PATH, storeRoute, viewPath, type BrowseView } from '../../lib/app-routes';
import type { StoreData } from '../../lib/directory-client';
import { currentHref, useStore } from '../../lib/use-store';
import { activeChip, Chips, chipsFor, kindOrAuthor, Notice, SearchBox, ShotCard } from './parts';

function viewOf(href: string | null): BrowseView | null {
  if (href === null) return null;
  const url = new URL(href, 'https://bridgething.com');
  const route = storeRoute(url.pathname, url.search);
  return route?.page === 'browse' ? route.view : null;
}

function results(listings: CatalogAppListing[], view: BrowseView): CatalogAppListing[] {
  if (view.kind === 'query') return browse(listings, view.query);
  const found = search(listings, view.text);
  return view.sort ? browse(found, { filter: null, sort: view.sort }) : found;
}

function titleOf(view: BrowseView, shown: CatalogAppListing[]): string {
  if (view.kind === 'search') return view.text ? `"${view.text}"` : 'search';
  const filter = view.query.filter;
  if (filter?.kind === 'author') return shown[0]?.app.author ?? filter.author;
  return queryTitle(view.query);
}

type SortTab = { label: string; sort: StoreSort | null };

function sortTabs(view: BrowseView): SortTab[] {
  const tabs: SortTab[] = SORTS.map(sort => ({ label: SORT_LABELS[sort], sort }));
  return view.kind === 'search' ? [{ label: 'best match', sort: null }, ...tabs] : tabs;
}

function withSort(view: BrowseView, sort: StoreSort | null): BrowseView {
  if (view.kind === 'search') return { ...view, sort };
  return { kind: 'query', query: { ...view.query, sort: sort ?? 'popular' } };
}

function currentSort(view: BrowseView): StoreSort | null {
  return view.kind === 'search' ? view.sort : view.query.sort;
}

function Sorts({ view }: { view: BrowseView }) {
  const active = currentSort(view);
  return (
    <nav aria-label="sort" class="-mx-6 flex [scrollbar-width:none] overflow-x-auto px-6 font-mono text-xs">
      {sortTabs(view).map(tab => (
        <a
          key={tab.label}
          href={viewPath(withSort(view, tab.sort))}
          aria-current={tab.sort === active ? 'page' : undefined}
          class={`-ml-px shrink-0 border px-3 py-1.5 whitespace-nowrap first:ml-0 ${
            tab.sort === active
              ? 'relative border-white/40 bg-white/10 text-white'
              : 'border-white/15 text-white/55 hover:text-white'
          }`}>
          {tab.label}
        </a>
      ))}
    </nav>
  );
}

function metaFor(view: BrowseView): (listing: CatalogAppListing) => string {
  if (view.kind === 'query' && view.query.filter?.kind === 'author') {
    return listing => listingKindLabel(listing) ?? installsLabel(listing.installs) ?? listing.app.author;
  }
  return listing => {
    const kind = listingKindLabel(listing);
    return kind ? `${listing.app.author} · ${kind}` : kindOrAuthor(listing);
  };
}

function plural(count: number): string {
  return `${count} app${count === 1 ? '' : 's'}`;
}

export function StoreBrowse({ data, href }: { data?: StoreData | null; href?: string }) {
  const { data: loaded, catalog, failed } = useStore(data);
  const [initial] = useState(() => viewOf(href ?? currentHref()));
  const [text, setText] = useState(initial?.kind === 'search' ? initial.text : '');

  const view = useMemo<BrowseView | null>(
    () => (initial?.kind === 'search' ? { ...initial, text: text.trim() } : initial),
    [initial, text],
  );

  const chips = useMemo(
    () => (catalog ? chipsFor(catalog.listings.length, storeFilters(catalog.listings)) : []),
    [catalog],
  );

  const shown = useMemo(() => (catalog && view ? results(catalog.listings, view) : []), [catalog, view]);

  if (initial === null || view === null) {
    return (
      <Notice>
        {href === undefined && typeof window === 'undefined' ? 'loading…' : 'nothing lives at this address.'}
      </Notice>
    );
  }

  function onSearch(next: string) {
    setText(next);
    if (view?.kind === 'search') history.replaceState(null, '', viewPath({ ...view, text: next.trim() }));
  }

  const active = view.kind === 'search' ? '' : activeChip(view.query.filter);
  const meta = metaFor(view);

  return (
    <>
      <p class="m-0 mb-6 font-mono text-sm text-white/40">
        <a href={STORE_PATH}>← apps</a>
      </p>

      <header class="mb-8 flex flex-col gap-5">
        <div class="flex flex-wrap items-end justify-between gap-x-8 gap-y-4">
          <div class="min-w-0">
            <h1 class="m-0 break-words">{titleOf(view, shown)}</h1>
            {catalog ? <p class="m-0 mt-2 font-mono text-sm text-white/40">{plural(shown.length)}</p> : null}
          </div>
          <div class="w-full sm:w-80">
            <SearchBox
              value={view.kind === 'search' ? text : ''}
              onInput={view.kind === 'search' ? onSearch : undefined}
            />
          </div>
        </div>
        {chips.length > 1 ? <Chips chips={chips} active={active} /> : null}
        <Sorts view={view} />
      </header>

      {catalog === null ? (
        <Notice>{failed ? 'could not reach the catalog. try reloading the page.' : 'loading…'}</Notice>
      ) : shown.length === 0 ? (
        <Notice>
          {view.kind === 'search' && !view.text
            ? 'search by name, author, or kind'
            : view.kind === 'search'
              ? STORE_COPY.noMatches
              : STORE_COPY.empty}
        </Notice>
      ) : (
        <ul class="m-0 grid list-none grid-cols-1 gap-x-6 gap-y-10 p-0 sm:grid-cols-2 lg:grid-cols-3">
          {shown.map(listing => (
            <li key={listing.app.id} class="min-w-0">
              <ShotCard listing={listing} meta={meta(listing)} />
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
