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
  aggregate,
  compareVersions,
  isListedWebapp,
  listedWebapps,
  newestCompatible,
  offersApp,
  pinsFrom,
  recommendedSources,
  satisfies,
  updates,
  versionCompatible,
  type CatalogAppListing,
  type CatalogAppUpdate,
  type ExtensionOffering,
  type InstalledWebapp,
} from './resolve.ts';

export { blendStoreListings, type StoreListings } from './blend.ts';

export {
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
