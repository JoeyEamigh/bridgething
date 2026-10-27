import { appCategories, CATEGORIES, CATEGORY_LABELS, type Category } from './categories.ts';
import type { CatalogAppListing } from './resolve.ts';
import type { AppEntry, StorePin } from './types.ts';
import { releasedAtInstant } from './versions.ts';

export type StoreKind = 'launchers' | 'overlays';

export type StoreFilter =
  | { kind: 'category'; category: Category }
  | { kind: StoreKind }
  | { kind: 'author'; author: string };

export type StoreSort = 'popular' | 'trending' | 'new' | 'updated';

export type BrowseQuery = { filter: StoreFilter | null; sort: StoreSort };

export type ShelfId = 'essentials' | 'popular' | 'launchers' | 'trending' | 'new' | 'overlays' | 'updated';

export type Shelf = {
  id: ShelfId;
  title: string;
  listings: CatalogAppListing[];
  more: BrowseQuery;
};

export type CategoryCount = { category: Category; label: string; count: number };

export type Storefront = {
  featured: CatalogAppListing | null;
  shelves: Shelf[];
  categories: CategoryCount[];
};

export const SHELF_SIZE = 10;

const FEATURE_POOL = 5;

const MIN_SHELF: Partial<Record<ShelfId, number>> = { essentials: 4, trending: 3 };

const DAY_MS = 24 * 60 * 60 * 1000;

export const SORT_LABELS: Record<StoreSort, string> = {
  popular: 'popular',
  trending: 'trending',
  new: 'new',
  updated: 'recently updated',
};

export function filterLabel(filter: StoreFilter): string {
  switch (filter.kind) {
    case 'category':
      return CATEGORY_LABELS[filter.category];
    case 'launchers':
      return 'home screens';
    case 'overlays':
      return 'overlays';
    case 'author':
      return filter.author;
  }
}

export function queryTitle(query: BrowseQuery): string {
  return query.filter ? filterLabel(query.filter) : SORT_LABELS[query.sort];
}

function instants(app: AppEntry): number[] {
  return app.versions.map(v => releasedAtInstant(v.released_at)).filter((at): at is number => at !== null);
}

export function firstReleasedAt(app: AppEntry): number | null {
  const all = instants(app);
  return all.length > 0 ? Math.min(...all) : null;
}

export function lastReleasedAt(app: AppEntry): number | null {
  const all = instants(app);
  return all.length > 0 ? Math.max(...all) : null;
}

function newerFirst(a: number | null, b: number | null): number {
  if (a === b) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  return b - a;
}

export function hasScreenshot(listing: CatalogAppListing): boolean {
  return (listing.app.screenshots?.length ?? 0) > 0;
}

export function isLauncher(listing: CatalogAppListing): boolean {
  return listing.newestCompatible?.role === 'launcher';
}

export function isOverlay(listing: CatalogAppListing): boolean {
  return listing.newestCompatible?.provides_overlay === true;
}

export function listingKindLabel(listing: CatalogAppListing): string | null {
  if (isLauncher(listing)) return 'home screen';
  if (isOverlay(listing)) return 'overlay';
  const first = appCategories(listing.app)[0];
  return first ? CATEGORY_LABELS[first] : null;
}

function sameAuthor(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

export function matchesFilter(listing: CatalogAppListing, filter: StoreFilter | null): boolean {
  if (filter === null) return true;
  switch (filter.kind) {
    case 'category':
      return appCategories(listing.app).includes(filter.category);
    case 'launchers':
      return isLauncher(listing);
    case 'overlays':
      return isOverlay(listing);
    case 'author':
      return sameAuthor(listing.app.author, filter.author);
  }
}

function rankedInstalls(listing: CatalogAppListing): number {
  return listing.official ? 0 : listing.installs;
}

function rankedTrending(listing: CatalogAppListing): number {
  return listing.official ? 0 : listing.trending;
}

function byName(a: CatalogAppListing, b: CatalogAppListing): number {
  return a.app.name.localeCompare(b.app.name) || a.app.id.localeCompare(b.app.id);
}

const COMPARE: Record<StoreSort, (a: CatalogAppListing, b: CatalogAppListing) => number> = {
  popular: (a, b) => rankedInstalls(b) - rankedInstalls(a) || rankedTrending(b) - rankedTrending(a) || byName(a, b),
  trending: (a, b) => rankedTrending(b) - rankedTrending(a) || rankedInstalls(b) - rankedInstalls(a) || byName(a, b),
  new: (a, b) => newerFirst(firstReleasedAt(a.app), firstReleasedAt(b.app)) || byName(a, b),
  updated: (a, b) => newerFirst(lastReleasedAt(a.app), lastReleasedAt(b.app)) || byName(a, b),
};

export function browse(listings: CatalogAppListing[], query: BrowseQuery): CatalogAppListing[] {
  const matched = listings.filter(listing => matchesFilter(listing, query.filter));
  const sorted = matched.sort(COMPARE[query.sort]);
  if (query.sort === 'trending') return sorted.filter(listing => rankedTrending(listing) > 0);
  if (query.sort === 'updated') return sorted.filter(listing => listing.app.versions.length > 1);
  return sorted;
}

export function moreFromAuthor(
  listings: CatalogAppListing[],
  listing: CatalogAppListing,
  limit = SHELF_SIZE,
): CatalogAppListing[] {
  return browse(listings, { filter: { kind: 'author', author: listing.app.author }, sort: 'popular' })
    .filter(other => other.app.id !== listing.app.id)
    .slice(0, limit);
}

function utcDay(now: Date): number {
  return Math.floor(now.getTime() / DAY_MS);
}

export function featuredListing(
  listings: CatalogAppListing[],
  pin: StorePin | null,
  now: Date,
): CatalogAppListing | null {
  if (pin) {
    const id = pin.app_id.toLowerCase();
    const pinned =
      listings.find(listing => listing.app.id.toLowerCase() === id && listing.sourceUrl === pin.source_url) ??
      listings.find(listing => listing.app.id.toLowerCase() === id);
    if (pinned) return pinned;
  }

  const pool = listings
    .filter(listing => hasScreenshot(listing) && !listing.official)
    .sort(COMPARE.trending)
    .slice(0, FEATURE_POOL);
  if (pool.length === 0) return null;
  return pool[utcDay(now) % pool.length]!;
}

function essentials(listings: CatalogAppListing[]): CatalogAppListing[] {
  const groups: StoreFilter[] = [
    { kind: 'launchers' },
    { kind: 'overlays' },
    ...CATEGORIES.map(category => ({ kind: 'category', category }) as const),
  ];
  const picked = new Map<string, CatalogAppListing>();
  for (const filter of groups) {
    const top = browse(listings, { filter, sort: 'popular' }).find(
      listing => rankedInstalls(listing) > 0 && !picked.has(listing.app.id),
    );
    if (top) picked.set(top.app.id, top);
  }
  return [...picked.values()].sort(COMPARE.popular);
}

export function storefront(args: { listings: CatalogAppListing[]; pin: StorePin | null; now: Date }): Storefront {
  const { listings, pin, now } = args;
  const top = (query: BrowseQuery) => browse(listings, query).slice(0, SHELF_SIZE);

  const candidates: Shelf[] = [
    { id: 'essentials', title: 'essentials', listings: essentials(listings), more: { filter: null, sort: 'popular' } },
    {
      id: 'popular',
      title: 'popular',
      listings: top({ filter: null, sort: 'popular' }).filter(listing => rankedInstalls(listing) > 0),
      more: { filter: null, sort: 'popular' },
    },
    {
      id: 'launchers',
      title: 'home screens',
      listings: top({ filter: { kind: 'launchers' }, sort: 'popular' }),
      more: { filter: { kind: 'launchers' }, sort: 'popular' },
    },
    { id: 'trending', title: 'trending', listings: top({ filter: null, sort: 'trending' }), more: { filter: null, sort: 'trending' } },
    { id: 'new', title: 'new', listings: top({ filter: null, sort: 'new' }), more: { filter: null, sort: 'new' } },
    {
      id: 'overlays',
      title: 'overlays',
      listings: top({ filter: { kind: 'overlays' }, sort: 'popular' }),
      more: { filter: { kind: 'overlays' }, sort: 'popular' },
    },
    {
      id: 'updated',
      title: 'recently updated',
      listings: top({ filter: null, sort: 'updated' }),
      more: { filter: null, sort: 'updated' },
    },
  ];

  const shelves = candidates.filter(shelf => shelf.listings.length >= (MIN_SHELF[shelf.id] ?? 1));

  const categories = CATEGORIES.map(category => ({
    category,
    label: CATEGORY_LABELS[category],
    count: listings.filter(listing => appCategories(listing.app).includes(category)).length,
  })).filter(entry => entry.count > 0);

  return { featured: featuredListing(listings, pin, now), shelves, categories };
}

function words(text: string): string[] {
  return text.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
}

function relevance(listing: CatalogAppListing, query: string, tokens: string[]): number {
  const { name, author, description } = listing.app;
  const lowerName = name.toLowerCase();
  const nameWords = words(name);
  const labels = appCategories(listing.app).map(category => CATEGORY_LABELS[category]);
  const kind = listingKindLabel(listing);
  if (kind) labels.push(kind);
  const tags = labels.join(' ').toLowerCase();
  const lowerAuthor = author.toLowerCase();
  const lowerDescription = description.toLowerCase();

  let score = lowerName === query ? 1000 : lowerName.startsWith(query) ? 500 : 0;
  for (const token of tokens) {
    let best = 0;
    if (nameWords.some(word => word.startsWith(token))) best = Math.max(best, 60);
    else if (lowerName.includes(token)) best = Math.max(best, 30);
    if (lowerAuthor.includes(token)) best = Math.max(best, 20);
    if (tags.includes(token)) best = Math.max(best, 15);
    if (lowerDescription.includes(token)) best = Math.max(best, 5);
    if (best === 0) return 0;
    score += best;
  }
  return score;
}

export function search(listings: CatalogAppListing[], raw: string): CatalogAppListing[] {
  const query = raw.trim().toLowerCase();
  if (query.length === 0) return [];
  const tokens = query.split(/\s+/);
  return listings
    .map(listing => ({ listing, score: relevance(listing, query, tokens) }))
    .filter(entry => entry.score > 0)
    .sort((a, b) => b.score - a.score || COMPARE.popular(a.listing, b.listing))
    .map(entry => entry.listing);
}
