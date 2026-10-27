import { appCategories } from './categories.ts';
import { aggregate, type CatalogAppListing, type ExtensionOffering, type InstalledWebapp } from './resolve.ts';
import type { MergedCatalog } from './sources.ts';
import type { InstallCount, SourceCatalog, TrendingCount } from './types.ts';

export type SourceTrust = 'official' | 'attested' | 'listed' | 'unlisted';

export type StoreSource = {
  url: string;
  name: string;
  icon: string | null;
  trust: SourceTrust;
};

export type StoreCatalog = {
  listings: CatalogAppListing[];
  sources: Record<string, StoreSource>;
};

const TRUST_ORDER: Record<SourceTrust, number> = { official: 0, attested: 1, listed: 2, unlisted: 3 };

function trustOf(entry: MergedCatalog): SourceTrust {
  if (entry.official) return 'official';
  return entry.attested ? 'attested' : 'listed';
}

export function reviewed(source: StoreSource | null | undefined): boolean {
  return source?.trust === 'official' || source?.trust === 'attested';
}

export function storeCatalog(args: {
  subscribed: SourceCatalog[];
  merged: MergedCatalog[];
  installed: InstalledWebapp[];
  deviceLibVersion: string | null;
  installs: InstallCount[];
  trending: TrendingCount[];
  categoryBackfill: Record<string, string[]>;
  extensions: ExtensionOffering;
}): StoreCatalog {
  const sources: Record<string, StoreSource> = {};
  for (const entry of args.merged) {
    const { name, icon } = entry.catalog.repo;
    sources[entry.url] = { url: entry.url, name, icon, trust: trustOf(entry) };
  }
  for (const { url, catalog } of args.subscribed) {
    sources[url] ??= { url, name: catalog.repo.name, icon: catalog.repo.icon, trust: 'unlisted' };
  }

  const subscribedUrls = new Set(args.subscribed.map(entry => entry.url));
  const extras = args.merged
    .filter(entry => !subscribedUrls.has(entry.url))
    .sort((a, b) => TRUST_ORDER[trustOf(a)] - TRUST_ORDER[trustOf(b)]);

  const listings = aggregate({
    orderedCatalogs: [...args.subscribed, ...extras.map(entry => ({ url: entry.url, catalog: entry.catalog }))],
    installed: args.installed,
    deviceLibVersion: args.deviceLibVersion,
    installs: args.installs,
    trending: args.trending,
    extensions: args.extensions,
  });

  return { listings: listings.map(listing => backfilled(listing, args.categoryBackfill)), sources };
}

function backfilled(listing: CatalogAppListing, backfill: Record<string, string[]>): CatalogAppListing {
  const fallback = backfill[listing.app.id.toLowerCase()];
  if (!fallback || appCategories(listing.app).length > 0) return listing;
  return { ...listing, app: { ...listing.app, categories: fallback } };
}
