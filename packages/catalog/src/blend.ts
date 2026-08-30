import { aggregate, type CatalogAppListing, type ExtensionOffering, type InstalledWebapp } from './resolve.ts';
import type { MergedCatalog } from './sources.ts';
import type { InstallCount, SourceCatalog } from './types.ts';

export type StoreListings = {
  vouched: CatalogAppListing[];
  community: CatalogAppListing[];
  sourceNames: Record<string, string>;
};

export function blendStoreListings(args: {
  catalogs: SourceCatalog[];
  merged: MergedCatalog[];
  installed: InstalledWebapp[];
  deviceLibVersion: string | null;
  installs: InstallCount[];
  subscribed: string[];
  extensions: ExtensionOffering;
}): StoreListings {
  const subscribed = new Set(args.subscribed);
  const extras = args.merged.filter(entry => !subscribed.has(entry.url));
  const communityUrls = new Set(extras.filter(entry => !entry.official && !entry.attested).map(entry => entry.url));
  const asSource = (entry: MergedCatalog): SourceCatalog => ({ url: entry.url, catalog: entry.catalog });

  const orderedCatalogs: SourceCatalog[] = [
    ...args.catalogs,
    ...extras.filter(entry => !communityUrls.has(entry.url)).map(asSource),
    ...extras.filter(entry => communityUrls.has(entry.url)).map(asSource),
  ];

  const listings = aggregate({
    orderedCatalogs,
    installed: args.installed,
    deviceLibVersion: args.deviceLibVersion,
    installs: args.installs,
    extensions: args.extensions,
  });

  const sourceNames: Record<string, string> = {};
  for (const { url, catalog } of orderedCatalogs) sourceNames[url] = catalog.repo.name;

  return {
    vouched: listings.filter(listing => !communityUrls.has(listing.sourceUrl)),
    community: listings.filter(listing => communityUrls.has(listing.sourceUrl)),
    sourceNames,
  };
}
