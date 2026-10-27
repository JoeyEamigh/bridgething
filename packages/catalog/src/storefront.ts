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

export type ShelfId = 'essentials' | 'popular' | 'trending' | 'new' | 'updated' | StoreKind | `category:${Category}`;

export type Shelf = {
  id: ShelfId;
  title: string;
  listings: CatalogAppListing[];
  more: BrowseQuery;
};

export type ChipFilter = Exclude<StoreFilter, { kind: 'author' }>;

export type FilterChip = { filter: ChipFilter; key: string; label: string };

export type Storefront = {
  featured: CatalogAppListing[];
  shelves: Shelf[];
  filters: FilterChip[];
  all: CatalogAppListing[];
};

export const SHELF_SIZE = 10;

export const FEATURED_COUNT = 3;

const MIN_SHELF = 4;

const AUTHOR_CAP = 2;

const DAY_MS = 24 * 60 * 60 * 1000;

export const SORT_LABELS: Record<StoreSort, string> = {
  popular: 'popular',
  trending: 'trending',
  new: 'new',
  updated: 'recently updated',
};

export function filterKey(filter: StoreFilter): string {
  switch (filter.kind) {
    case 'category':
      return `c:${filter.category}`;
    case 'author':
      return `by:${filter.author}`;
    default:
      return filter.kind;
  }
}

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

export function pinnedListing(listings: CatalogAppListing[], pin: StorePin | null): CatalogAppListing | null {
  if (!pin) return null;
  const id = pin.app_id.toLowerCase();
  return (
    listings.find(listing => listing.app.id.toLowerCase() === id && listing.sourceUrl === pin.source_url) ??
    listings.find(listing => listing.app.id.toLowerCase() === id) ??
    null
  );
}

function dailyRandom(now: Date): () => number {
  let state = utcDay(now) >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let mixed = Math.imul(state ^ (state >>> 15), state | 1);
    mixed ^= mixed + Math.imul(mixed ^ (mixed >>> 7), mixed | 61);
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

function featureWeight(listing: CatalogAppListing): number {
  return Math.sqrt(1 + rankedInstalls(listing) + 4 * rankedTrending(listing));
}

export function featuredListings(listings: CatalogAppListing[], pin: StorePin | null, now: Date): CatalogAppListing[] {
  const pinned = pinnedListing(listings, pin);
  const picked = pinned ? [pinned] : [];
  const pool = listings
    .filter(listing => hasScreenshot(listing) && !listing.official && listing !== pinned)
    .sort((a, b) => a.app.id.localeCompare(b.app.id) || a.sourceUrl.localeCompare(b.sourceUrl));
  const random = dailyRandom(now);

  while (picked.length < FEATURED_COUNT && pool.length > 0) {
    const weights = pool.map(featureWeight);
    let roll = random() * weights.reduce((sum, weight) => sum + weight, 0);
    const index = weights.findIndex(weight => (roll -= weight) < 0);
    picked.push(pool.splice(index === -1 ? pool.length - 1 : index, 1)[0]!);
  }
  return picked;
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

function varied(sorted: CatalogAppListing[], shown: Set<string>): CatalogAppListing[] {
  const perAuthor = new Map<string, number>();
  const picked: CatalogAppListing[] = [];
  for (const listing of sorted) {
    if (picked.length === SHELF_SIZE) break;
    if (shown.has(listing.app.id)) continue;
    const author = listing.app.author.trim().toLowerCase();
    const held = perAuthor.get(author) ?? 0;
    if (held === AUTHOR_CAP) continue;
    perAuthor.set(author, held + 1);
    picked.push(listing);
  }
  return picked;
}

export function storeFilters(listings: CatalogAppListing[]): FilterChip[] {
  const filters: ChipFilter[] = [
    { kind: 'launchers' },
    { kind: 'overlays' },
    ...CATEGORIES.map(category => ({ kind: 'category', category }) as const),
  ];
  return filters
    .filter(filter => listings.some(listing => matchesFilter(listing, filter)))
    .map(filter => ({ filter, key: filterKey(filter), label: filterLabel(filter) }));
}

export function storefront(args: { listings: CatalogAppListing[]; pin: StorePin | null; now: Date }): Storefront {
  const { listings, pin, now } = args;
  const featured = featuredListings(listings, pin, now);
  const shown = new Set(featured.map(listing => listing.app.id));

  const ranked = browse(listings, { filter: null, sort: 'popular' });

  const sorted = (id: 'trending' | 'new' | 'updated'): Shelf => {
    const more: BrowseQuery = { filter: null, sort: id };
    const picked = varied(browse(listings, more), shown);
    if (picked.length >= MIN_SHELF) for (const listing of picked) shown.add(listing.app.id);
    return { id, title: SORT_LABELS[id], listings: picked, more };
  };
  const filtered = (id: ShelfId, filter: ChipFilter): Shelf => {
    const more: BrowseQuery = { filter, sort: 'popular' };
    return { id, title: filterLabel(filter), listings: varied(browse(listings, more), new Set()), more };
  };

  const shelves: Shelf[] = [
    { id: 'essentials', title: 'essentials', listings: essentials(listings), more: { filter: null, sort: 'popular' } },
    {
      id: 'popular',
      title: 'popular',
      listings: ranked.filter(listing => rankedInstalls(listing) > 0).slice(0, SHELF_SIZE),
      more: { filter: null, sort: 'popular' },
    },
    sorted('trending'),
    sorted('new'),
    filtered('launchers', { kind: 'launchers' }),
    ...CATEGORIES.map(category => filtered(`category:${category}`, { kind: 'category', category })),
    filtered('overlays', { kind: 'overlays' }),
    sorted('updated'),
  ];

  return {
    featured,
    shelves: shelves.filter(shelf => shelf.listings.length >= (shelf.id === 'popular' ? 1 : MIN_SHELF)),
    filters: storeFilters(listings),
    all: [...listings].sort(byName),
  };
}

function words(text: string): string[] {
  return text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
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
