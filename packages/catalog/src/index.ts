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
  StorePin,
  TrendingCount,
} from './types.ts';

export { CATEGORIES, CATEGORY_LABELS, MAX_CATEGORIES, appCategories, isCategory, type Category } from './categories.ts';

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

export { webHref } from './href.ts';

export { changelogText } from './changelog.ts';

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

export { reviewed, storeCatalog, type SourceTrust, type StoreCatalog, type StoreSource } from './store.ts';

export {
  FEATURED_COUNT,
  SHELF_SIZE,
  SORT_LABELS,
  browse,
  featuredListings,
  filterKey,
  filterLabel,
  firstReleasedAt,
  hasScreenshot,
  isLauncher,
  isOverlay,
  lastReleasedAt,
  listingKindLabel,
  matchesFilter,
  moreFromAuthor,
  pinnedListing,
  queryTitle,
  search,
  storeFilters,
  storefront,
  type BrowseQuery,
  type ChipFilter,
  type FilterChip,
  type Shelf,
  type ShelfId,
  type StoreFilter,
  type StoreKind,
  type StoreSort,
  type Storefront,
} from './storefront.ts';

export { STORE_COPY, alsoAvailableLabel, failureLine, installsLabel, permissionLabel, sourceLine } from './copy.ts';

export {
  CATALOG_FETCH_TIMEOUT_MS,
  DIRECTORY_ORIGIN,
  DirectoryApiError,
  OFFICIAL_CATALOG_URL,
  SOURCE_DIRECTORY_URL,
  SOURCE_STATUSES,
  SourceUrlError,
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  installedBody,
  normalizeSourceUrl,
  parseSourceUrl,
  reportInstalled,
  submitSource,
  unwrapDirectory,
  type CatalogSnapshot,
  type DirectoryEntry,
  type InstalledApp,
  type InstalledReport,
  type MergedApps,
  type MergedCatalog,
  type SourceFailure,
  type SourceStatus,
} from './sources.ts';
