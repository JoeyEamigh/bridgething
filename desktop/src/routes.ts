import { isCategory, type BrowseQuery, type StoreFilter, type StoreSort } from '@bridgething/catalog';

import type { IconName } from './lib/icons.tsx';

export const PATHS = {
  devices: '/devices',
  apps: '/apps',
  app: (webappId: string) => `/apps/${encodeURIComponent(webappId)}`,
  appSettings: (webappId: string) => `/apps/${encodeURIComponent(webappId)}/settings`,
  store: '/store',
  storeBrowse: (query: BrowseQuery) => `/store/browse?${browseParams(query)}`,
  storeApp: (appId: string) => `/store/app/${encodeURIComponent(appId)}`,
  storeSource: (url: string) => `/store/source/${encodeURIComponent(url)}`,
  storeSourceApp: (url: string, appId: string) =>
    `/store/source/${encodeURIComponent(url)}/app/${encodeURIComponent(appId)}`,
  updates: '/updates',
  logs: '/logs',
  settings: '/settings',
} as const;

const SORTS: readonly StoreSort[] = ['popular', 'trending', 'new', 'updated'];

function browseParams({ filter, sort }: BrowseQuery): string {
  const params = new URLSearchParams({ sort });
  if (filter?.kind === 'category') params.set('category', filter.category);
  else if (filter?.kind === 'author') params.set('author', filter.author);
  else if (filter) params.set('kind', filter.kind);
  return params.toString();
}

export function browseQuery(params: Record<string, string | undefined>): BrowseQuery {
  const sort = SORTS.find(candidate => candidate === params.sort) ?? 'popular';
  return { filter: browseFilter(params), sort };
}

function browseFilter({ category, author, kind }: Record<string, string | undefined>): StoreFilter | null {
  if (category && isCategory(category)) return { kind: 'category', category };
  if (author) return { kind: 'author', author };
  if (kind === 'launchers' || kind === 'overlays') return { kind };
  return null;
}

export const SECTIONS: { path: string; label: string; icon: IconName }[] = [
  { path: PATHS.devices, label: 'devices', icon: 'device' },
  { path: PATHS.apps, label: 'apps', icon: 'grid' },
  { path: PATHS.store, label: 'store', icon: 'store' },
  { path: PATHS.updates, label: 'updates', icon: 'download' },
  { path: PATHS.logs, label: 'logs', icon: 'terminal' },
  { path: PATHS.settings, label: 'settings', icon: 'gear' },
];

export function sectionFor(path: string): string | null {
  return SECTIONS.find(section => path === section.path || path.startsWith(`${section.path}/`))?.path ?? null;
}
