export type {
  AppEntry,
  AppExtension,
  AppVersion,
  Catalog,
  Download,
  InstallCount,
  RecommendedSource,
  Repo,
  SourceCatalog,
} from './types.ts';

export { CatalogValidationError, validate, validateInvariants, validateSchema } from './validate.ts';

export {
  EXTENSION_PERMISSION_PATTERN,
  EXTENSION_SOURCE_PATTERN,
  declaresExtension,
  describeExtensionPermission,
  describeExtensionPermissions,
  extensionOf,
  extensionRepoLabel,
  isExtensionPermission,
} from './extension.ts';

export { releasedAtInstant, sortNewestFirst } from './versions.ts';

export {
  SETTINGS_PAGE_MIME,
  aggregate,
  compareVersions,
  fillsSlot,
  isListedWebapp,
  listedWebapps,
  newestCompatible,
  offersApp,
  pinsFrom,
  recommendedSources,
  satisfies,
  settingsOrigin,
  settingsOriginFor,
  slotCandidates,
  updates,
  versionCompatible,
  type CatalogAppListing,
  type CatalogAppUpdate,
  type ExtensionOffering,
  type InstalledWebapp,
  type WebappSlotName,
} from './resolve.ts';

export { blendStoreListings, type StoreListings } from './blend.ts';

export { STORE_COPY, alsoAvailableLabel, countLine, failureLine, installsLabel, listingTraits } from './copy.ts';

export {
  CATALOG_FETCH_TIMEOUT_MS,
  DIRECTORY_ORIGIN,
  OFFICIAL_CATALOG_URL,
  SOURCE_DIRECTORY_URL,
  SourceUrlError,
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  installedBody,
  normalizeSourceUrl,
  parseSourceUrl,
  reportInstalled,
  type CatalogSnapshot,
  type InstalledApp,
  type InstalledReport,
  type MergedApps,
  type MergedCatalog,
  type SourceFailure,
} from './sources.ts';
