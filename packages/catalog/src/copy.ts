import type { CatalogAppListing } from './resolve.ts';

export const STORE_COPY = {
  appsTitle: 'apps',
  appsHint: 'from your sources and the bridgething directory',
  appsEmpty: 'the catalog exists but has no apps in it.',
  communityTitle: 'community apps',
  communityNote: 'these are unreviewed',
  communityHint: 'from directory sources you have not added',
  communityEmpty: 'nothing here',
  suggestedTitle: 'suggested sources',
  suggestedHint: 'listed in the bridgething directory',
  suggestedEmpty: 'nothing the directory lists is missing from your sources.',
  needsFirmware: 'needs newer firmware',
  unpublished: 'not published yet',
} as const;

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? '' : 's'}`;
}

export function countLine(listings: number, sources: number): string {
  return `${listings} across ${plural(sources, 'source')}`;
}

export function failureLine(failures: number): string {
  return `${plural(failures, 'source')} could not be read.`;
}

export function installsLabel(installs: number): string | null {
  return installs > 0 ? plural(installs, 'install') : null;
}

export function alsoAvailableLabel(alsoAvailableFrom: readonly string[]): string | null {
  return alsoAvailableFrom.length > 0 ? `also offered by ${plural(alsoAvailableFrom.length, 'other source')}` : null;
}

export function listingTraits(listing: CatalogAppListing): string[] {
  const newest = listing.newestCompatible;
  return [
    newest ? `lib ${newest.min_libbridgething_version}` : null,
    newest?.role === 'launcher' ? 'launcher' : null,
    newest?.provides_overlay ? 'overlay' : null,
    installsLabel(listing.installs),
  ].filter((trait): trait is string => trait !== null);
}
