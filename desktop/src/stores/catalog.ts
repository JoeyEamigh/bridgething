import {
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  type Catalog,
  type CatalogSnapshot,
  type MergedApps,
} from '@bridgething/catalog';

import { keyed, type Store } from './resource.ts';

const EMPTY: CatalogSnapshot = { catalogs: [], directory: null, failures: [] };
const NO_DIRECTORY: MergedApps = { updated_at: '', catalogs: [], failures: [], installs: [] };

const snapshots = keyed<CatalogSnapshot>(EMPTY);
const single = keyed<Catalog | null>(null);
const directory = keyed<MergedApps>(NO_DIRECTORY);

export function catalogsFor(subscribed: string[]): Store<CatalogSnapshot> {
  return snapshots.at(subscribed.join('\n'), () => fetchSources(subscribed));
}

export function catalogFor(url: string): Store<Catalog | null> {
  return single.at(url, () => fetchCatalog(url));
}

export function mergedApps(): Store<MergedApps> {
  return directory.at('apps', () => fetchMergedApps());
}
