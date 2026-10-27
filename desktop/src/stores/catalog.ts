import {
  aggregate,
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  storeCatalog,
  type Catalog,
  type CatalogAppListing,
  type CatalogSnapshot,
  type InstalledWebapp,
  type MergedApps,
  type StoreCatalog,
  type StorePin,
  type StoreSource,
} from '@bridgething/catalog';
import type { WebappInfo } from '@bridgething/companion-types';

import { toInstalled } from '../lib/catalog.ts';
import { keyed, type Store } from './resource.ts';
import { catalogSources, selectedMeta, webapps } from './session.ts';

const EMPTY: CatalogSnapshot = { catalogs: [], directory: null, failures: [] };
const NO_DIRECTORY: MergedApps = {
  updated_at: '',
  catalogs: [],
  failures: [],
  installs: [],
  trending: [],
  featured: null,
  category_backfill: {},
};

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
  listings: CatalogAppListing[];
  sources: Record<string, StoreSource>;
  pin: StorePin | null;
  subscribed: string[];
  snapshot: CatalogSnapshot;
  pending: boolean;
  error: string | undefined;
  refresh: () => void;
};

type Inputs = { snapshot: CatalogSnapshot; merged: MergedApps; installed: WebappInfo[]; libVersion: string | null };

let held: { inputs: Inputs; catalog: StoreCatalog } | null = null;

function blended(inputs: Inputs): StoreCatalog {
  const last = held?.inputs;
  if (
    held &&
    last?.snapshot === inputs.snapshot &&
    last.merged === inputs.merged &&
    last.installed === inputs.installed &&
    last.libVersion === inputs.libVersion
  ) {
    return held.catalog;
  }

  const catalog = storeCatalog({
    subscribed: inputs.snapshot.catalogs,
    merged: inputs.merged.catalogs,
    installed: toInstalled(inputs.installed),
    deviceLibVersion: inputs.libVersion,
    installs: inputs.merged.installs,
    trending: inputs.merged.trending,
    categoryBackfill: inputs.merged.category_backfill,
    extensions: 'listed',
  });
  held = { inputs, catalog };
  return catalog;
}

export function storeListings(): StoreListings {
  const subscribed = catalogSources.data.value;
  const catalogs = catalogsFor(subscribed);
  const directory = mergedApps();
  const snapshot = catalogs.data.value;
  const merged = directory.data.value;

  const { listings, sources } = blended({
    snapshot,
    merged,
    installed: webapps.data.value,
    libVersion: selectedMeta.value?.libbridgethingVersion ?? null,
  });

  return {
    listings,
    sources,
    pin: merged.featured,
    subscribed,
    snapshot,
    pending: catalogs.pending.value || directory.pending.value,
    error: catalogs.error.value ?? directory.error.value,
    refresh: () => {
      void catalogs.refresh();
      void directory.refresh();
    },
  };
}

export function sourceListings(url: string): { listings: CatalogAppListing[]; catalog: Store<Catalog | null> } {
  const catalog = catalogFor(url);
  const held = catalog.data.value;
  const merged = mergedApps().data.value;
  const { installed, libVersion } = deviceContext();
  const listings = held
    ? aggregate({
        orderedCatalogs: [{ url, catalog: held }],
        installed,
        deviceLibVersion: libVersion,
        installs: merged.installs,
        trending: merged.trending,
        extensions: 'listed',
      })
    : [];
  return { listings, catalog };
}
