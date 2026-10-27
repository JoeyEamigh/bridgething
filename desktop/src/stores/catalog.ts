import {
  blendStoreListings,
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  type Catalog,
  type CatalogAppListing,
  type CatalogSnapshot,
  type InstalledWebapp,
  type MergedApps,
} from '@bridgething/catalog';

import { toInstalled } from '../lib/catalog.ts';
import { keyed, type Store } from './resource.ts';
import { catalogSources, selectedMeta, webapps } from './session.ts';

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

export function deviceContext(): { installed: InstalledWebapp[]; libVersion: string | null } {
  return { installed: toInstalled(webapps.data.value), libVersion: selectedMeta.value?.libbridgethingVersion ?? null };
}

export type StoreListings = {
  vouched: CatalogAppListing[];
  community: CatalogAppListing[];
  subscribed: string[];
  snapshot: CatalogSnapshot;
  pending: boolean;
  error: string | undefined;
  refresh: () => void;
};

export function storeListings(): StoreListings {
  const { installed, libVersion } = deviceContext();
  const subscribed = catalogSources.data.value;
  const catalogs = catalogsFor(subscribed);
  const directory = mergedApps();
  const snapshot = catalogs.data.value;
  const merged = directory.data.value;

  const { vouched, community } = blendStoreListings({
    catalogs: snapshot.catalogs,
    merged: merged.catalogs,
    installed,
    deviceLibVersion: libVersion,
    installs: merged.installs,
    subscribed,
    extensions: 'listed',
  });

  return {
    vouched,
    community,
    subscribed,
    snapshot,
    pending: catalogs.pending.value || directory.pending.value,
    error: catalogs.error.value,
    refresh: () => {
      void catalogs.refresh();
      void directory.refresh();
    },
  };
}
