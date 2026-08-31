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
  isListedWebapp,
  listedWebapps,
  newestCompatible,
  offersApp,
  pinsFrom,
  recommendedSources,
  satisfies,
  settingsOrigin,
  settingsOriginFor,
  updates,
  versionCompatible,
  type CatalogAppListing,
  type CatalogAppUpdate,
  type ExtensionOffering,
  type InstalledWebapp,
} from './resolve.ts';

export { blendStoreListings, type StoreListings } from './blend.ts';

export {
  CATALOG_FETCH_TIMEOUT_MS,
  DIRECTORY_ORIGIN,
  OFFICIAL_CATALOG_URL,
  SOURCE_DIRECTORY_URL,
  SourceUrlError,
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  normalizeSourceUrl,
  parseSourceUrl,
  reportInstall,
  type CatalogSnapshot,
  type InstallReport,
  type MergedApps,
  type MergedCatalog,
  type SourceFailure,
} from './sources.ts';
