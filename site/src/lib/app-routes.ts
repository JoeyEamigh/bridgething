import { isCategory, type BrowseQuery, type StoreFilter, type StoreKind, type StoreSort } from '@bridgething/catalog';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export const APP_DETAIL_SHELL_ID = 'lookup';
export const APP_DETAIL_SHELL = `/apps/${APP_DETAIL_SHELL_ID}/`;
export const AUTHOR_SHELL = '/apps/by/';

export const STORE_PATH = '/apps';
export const SEARCH_PATH = '/apps/search';
export const SOURCES_PATH = '/apps/sources';

export const SORTS: StoreSort[] = ['popular', 'trending', 'new', 'updated'];

const KIND_SLUGS: Record<StoreKind, string> = { launchers: 'home-screens', overlays: 'overlays' };

export type BrowseView =
  | { kind: 'query'; query: BrowseQuery }
  | { kind: 'search'; text: string; sort: StoreSort | null };

export type StoreRoute =
  | { page: 'front' }
  | { page: 'browse'; view: BrowseView }
  | { page: 'app'; id: string }
  | { page: 'sources' };

export function appDetailPath(id: string): string {
  return `/apps/${id}`;
}

function sortSuffix(sort: StoreSort, implied: StoreSort): string {
  return sort === implied ? '' : `?sort=${sort}`;
}

export function filterPath(filter: StoreFilter): string {
  switch (filter.kind) {
    case 'category':
      return `/apps/c/${filter.category}`;
    case 'launchers':
    case 'overlays':
      return `/apps/${KIND_SLUGS[filter.kind]}`;
    case 'author':
      return `${AUTHOR_SHELL}${encodeURIComponent(filter.author)}`;
  }
}

export function browsePath(query: BrowseQuery): string {
  if (query.filter === null) return `/apps/${query.sort}`;
  return `${filterPath(query.filter)}${sortSuffix(query.sort, 'popular')}`;
}

export function searchPath(text: string, sort: StoreSort | null = null): string {
  const params = new URLSearchParams();
  if (text) params.set('q', text);
  if (sort) params.set('sort', sort);
  const search = params.toString();
  return search ? `${SEARCH_PATH}?${search}` : SEARCH_PATH;
}

export function viewPath(view: BrowseView): string {
  return view.kind === 'search' ? searchPath(view.text, view.sort) : browsePath(view.query);
}

function sortOf(raw: string | null): StoreSort | null {
  return raw !== null && (SORTS as string[]).includes(raw) ? (raw as StoreSort) : null;
}

function filtered(filter: StoreFilter, params: URLSearchParams): StoreRoute {
  return { page: 'browse', view: { kind: 'query', query: { filter, sort: sortOf(params.get('sort')) ?? 'popular' } } };
}

function single(segment: string, params: URLSearchParams): StoreRoute | null {
  if (UUID.test(segment)) return { page: 'app', id: segment.toLowerCase() };
  if (segment === 'sources') return { page: 'sources' };
  if (segment === 'search') {
    return {
      page: 'browse',
      view: { kind: 'search', text: params.get('q')?.trim() ?? '', sort: sortOf(params.get('sort')) },
    };
  }
  const sort = sortOf(segment);
  if (sort) return { page: 'browse', view: { kind: 'query', query: { filter: null, sort } } };
  for (const [kind, slug] of Object.entries(KIND_SLUGS) as [StoreKind, string][]) {
    if (segment === slug) return filtered({ kind }, params);
  }
  return null;
}

function decoded(segment: string): string | null {
  try {
    return decodeURIComponent(segment).trim() || null;
  } catch {
    return null;
  }
}

export function storeRoute(pathname: string, search = ''): StoreRoute | null {
  const parts = pathname.split('/').filter(Boolean);
  if (parts[0] !== 'apps') return null;
  const params = new URLSearchParams(search);

  if (parts.length === 1) return { page: 'front' };
  if (parts.length === 2) return single(parts[1]!, params);
  if (parts.length !== 3) return null;

  const [, prefix, value] = parts as [string, string, string];
  if (prefix === 'c' && isCategory(value)) return filtered({ kind: 'category', category: value }, params);
  if (prefix === 'by') {
    const author = decoded(value);
    return author ? filtered({ kind: 'author', author }, params) : null;
  }
  return null;
}

export function appIdFromPath(pathname: string): string | null {
  const route = storeRoute(pathname);
  return route?.page === 'app' ? route.id : null;
}

export function shellFor(pathname: string): string | null {
  const route = storeRoute(pathname);
  if (route?.page === 'app') return APP_DETAIL_SHELL;
  if (route?.page === 'browse' && route.view.kind === 'query' && route.view.query.filter?.kind === 'author') {
    return AUTHOR_SHELL;
  }
  return null;
}
