import {
  blendStoreListings,
  compareVersions,
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  normalizeSourceUrl,
  aggregate,
  recommendedSources as resolveRecommended,
  reportInstalled,
  SETTINGS_PAGE_MIME,
  settingsOriginFor,
  updates as resolveUpdates,
  type AppVersion,
  type Catalog,
  type CatalogAppListing,
  type CatalogAppUpdate,
  type InstallCount,
  type InstalledReport,
  type InstalledWebapp,
  type MergedCatalog,
  type RecommendedSource,
  type SourceCatalog,
  type SourceFailure,
  type StoreListings,
} from '@bridgething/catalog';
import type {
  BridgethingResourceOrigin,
  BridgethingWebappInfo,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useMemo } from 'react';
import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';

import { getSession } from './bridge';
import { storage } from './storage';
import { connectedPeers, useSessionStore, type SessionState } from './session';
import { formatBytes } from './utils';
import { useWebappsStore } from './webapps';

type CatalogState = {
  sources: string[];
  catalogs: SourceCatalog[];
  directory: Catalog | null;
  merged: MergedCatalog[];
  installs: InstallCount[];
  failures: SourceFailure[];
  refreshing: boolean;
  previews: Record<string, { catalog: Catalog; fetchedAt: number }>;
};

const PREVIEW_TTL_MS = 5 * 60 * 1000;

export const useCatalogStore = create<CatalogState>(() => ({
  sources: [],
  catalogs: [],
  directory: null,
  merged: [],
  installs: [],
  failures: [],
  refreshing: false,
  previews: {},
}));

const INHERITED_SOURCES_KEY = 'catalog.sources';

async function inheritStoredSources(): Promise<void> {
  const raw = storage.getString(INHERITED_SOURCES_KEY);
  if (raw === undefined) return;
  let urls: unknown;
  try {
    urls = JSON.parse(raw);
  } catch {
    urls = [];
  }
  if (Array.isArray(urls) && urls.length > 0) {
    const held = await getSession().catalogSources();
    const extra = urls.filter(
      (url): url is string => typeof url === 'string' && !held.includes(url),
    );
    if (extra.length > 0)
      await getSession().setCatalogSources([...held, ...extra]);
  }
  storage.remove(INHERITED_SOURCES_KEY);
}

let inheriting: Promise<void> | null = null;

async function heldSources(): Promise<string[]> {
  try {
    inheriting ??= inheritStoredSources().catch(() => {
      inheriting = null;
    });
    await inheriting;
    const sources = await getSession().catalogSources();
    useCatalogStore.setState({ sources });
    return sources;
  } catch {
    return useCatalogStore.getState().sources;
  }
}

let sourceWrites: Promise<unknown> = Promise.resolve();

function rewriteSources(
  change: (held: string[]) => string[] | null,
): Promise<boolean> {
  const write = sourceWrites.then(async () => {
    const next = change(await heldSources());
    if (!next) return false;
    const sources = await getSession().setCatalogSources(next);
    useCatalogStore.setState(s => ({
      sources,
      catalogs: s.catalogs
        .filter(e => sources.includes(e.url))
        .sort((a, b) => sources.indexOf(a.url) - sources.indexOf(b.url)),
    }));
    return true;
  });
  sourceWrites = write.catch(() => undefined);
  return write;
}

let refreshGeneration = 0;

export async function refreshCatalog(): Promise<void> {
  const generation = ++refreshGeneration;
  useCatalogStore.setState({ refreshing: true });
  const sources = await heldSources();
  const { merged: heldMerged, installs: heldInstalls } =
    useCatalogStore.getState();

  const [snapshot, merged] = await Promise.all([
    fetchSources(sources).catch(err => ({
      catalogs: [],
      directory: null,
      failures: sources.map(url => ({ url, reason: describeError(err) })),
    })),
    fetchMergedApps().catch(() => ({
      catalogs: heldMerged,
      installs: heldInstalls,
    })),
  ]);

  if (generation !== refreshGeneration) return;
  useCatalogStore.setState({
    ...snapshot,
    merged: merged.catalogs,
    installs: merged.installs,
    refreshing: false,
  });
}

export async function previewSource(url: string): Promise<Catalog> {
  const cached = useCatalogStore.getState().previews[url];
  if (cached && Date.now() - cached.fetchedAt < PREVIEW_TTL_MS) {
    return cached.catalog;
  }
  const catalog = await fetchCatalog(url);
  useCatalogStore.setState(s => ({
    previews: { ...s.previews, [url]: { catalog, fetchedAt: Date.now() } },
  }));
  return catalog;
}

export function usePreview(url: string | null): Catalog | null {
  return useCatalogStore(s =>
    url ? (s.previews[url]?.catalog ?? null) : null,
  );
}

export function useIsSubscribed(url: string): boolean {
  return useCatalogStore(s => s.sources.includes(url));
}

export type SourceInput =
  | { ok: true; url: string }
  | { ok: false; error: string };

export function parseSourceInput(raw: string): SourceInput {
  try {
    return { ok: true, url: normalizeSourceUrl(raw) };
  } catch (err) {
    return { ok: false, error: describeError(err) };
  }
}

export async function addSource(url: string): Promise<string> {
  const normalized = normalizeSourceUrl(url);
  const added = await rewriteSources(held =>
    held.includes(normalized) ? null : [...held, normalized],
  );
  if (added) await refreshCatalog();
  return normalized;
}

export async function moveSource(url: string, delta: number): Promise<void> {
  await rewriteSources(held => {
    const from = held.indexOf(url);
    const to = from + delta;
    if (from === -1 || to < 0 || to >= held.length) return null;
    const next = [...held];
    next.splice(to, 0, ...next.splice(from, 1));
    return next;
  });
}

export async function removeSource(url: string): Promise<void> {
  const removed = await rewriteSources(held =>
    held.includes(url) ? held.filter(u => u !== url) : null,
  );
  if (removed) await refreshCatalog();
}

function toInstalled(list: BridgethingWebappInfo[]): InstalledWebapp[] {
  return list.map(info => ({
    id: info.id,
    version: info.version,
    source: info.source,
    role: info.role,
    provenance: info.provenance ?? null,
  }));
}

export function deviceLibVersion(
  state: SessionState,
  deviceId: string | null,
): string | null {
  if (!deviceId) return null;
  return state.ledger[deviceId]?.libVersion ?? null;
}

export function deviceSerial(
  state: SessionState,
  deviceId: string | null,
): string | null {
  if (!deviceId) return null;
  return state.ledger[deviceId]?.serialNumber ?? null;
}

function useDerivedInputs(deviceId: string | null) {
  const catalogs = useCatalogStore(s => s.catalogs);
  const installed = useWebappsStore(
    useShallow(s => (deviceId ? (s.byDevice[deviceId]?.list ?? []) : [])),
  );
  const libVersion = useSessionStore(s => deviceLibVersion(s, deviceId));
  return { catalogs, installed, deviceLibVersion: libVersion };
}

export function useStoreListings(deviceId: string | null): StoreListings {
  const { catalogs, installed, deviceLibVersion } = useDerivedInputs(deviceId);
  const merged = useCatalogStore(s => s.merged);
  const installs = useCatalogStore(s => s.installs);
  const sources = useCatalogStore(s => s.sources);

  return useMemo(
    () =>
      blendStoreListings({
        catalogs,
        merged,
        installed: toInstalled(installed),
        deviceLibVersion,
        installs,
        subscribed: sources,
        extensions: 'omitted',
      }),
    [catalogs, merged, installs, sources, installed, deviceLibVersion],
  );
}

export function useSourceListings(
  url: string | null,
  deviceId: string | null,
): CatalogAppListing[] {
  const preview = usePreview(url);
  const merged = useCatalogStore(s => s.merged);
  const installs = useCatalogStore(s => s.installs);
  const { catalogs, installed, deviceLibVersion } = useDerivedInputs(deviceId);
  return useMemo(() => {
    if (!url) return [];
    const catalog =
      preview ??
      catalogs.find(c => c.url === url)?.catalog ??
      merged.find(c => c.url === url)?.catalog;
    if (!catalog) return [];
    return aggregate({
      orderedCatalogs: [{ url, catalog }],
      installed: toInstalled(installed),
      deviceLibVersion,
      installs,
      extensions: 'omitted',
    });
  }, [url, preview, catalogs, merged, installs, installed, deviceLibVersion]);
}

export function useUpdates(deviceId: string | null): CatalogAppUpdate[] {
  const { catalogs, installed, deviceLibVersion } = useDerivedInputs(deviceId);
  return useMemo(
    () =>
      resolveUpdates({
        catalogs: new Map(catalogs.map(e => [e.url, e.catalog])),
        installed: toInstalled(installed),
        deviceLibVersion,
        extensions: 'omitted',
      }),
    [catalogs, installed, deviceLibVersion],
  );
}

export function useQuickAddSources(): RecommendedSource[] {
  const catalogs = useCatalogStore(s => s.catalogs);
  const directory = useCatalogStore(s => s.directory);
  const sources = useCatalogStore(s => s.sources);

  return useMemo(
    () =>
      resolveRecommended({
        directory,
        orderedCatalogs: catalogs,
        subscribed: sources,
      }),
    [catalogs, directory, sources],
  );
}

export type VersionInstallCopy = {
  title: string;
  body: string;
  warning: string | null;
  detail: string;
};

export function describeVersionInstall(args: {
  version: AppVersion;
  newest: AppVersion | null;
  installedVersion: string | null;
}): VersionInstallCopy {
  const { version, newest, installedVersion } = args;
  const back =
    installedVersion !== null &&
    compareVersions(version.version, installedVersion) < 0;

  return {
    title: `install v${version.version}?`,
    body: back
      ? `this puts v${version.version} back on your car thing in place of v${installedVersion}.`
      : 'this replaces what is on your car thing now.',
    warning:
      newest && newest.version !== version.version
        ? `v${newest.version} is the newest build your car thing can run, and the next update offer moves you back to it.`
        : null,
    detail: `detail: needs firmware ${version.min_libbridgething_version} · ${formatBytes(version.download.size)}`,
  };
}

export async function installApp(
  deviceId: string,
  listing: CatalogAppListing,
  version: AppVersion | null = listing.newestCompatible,
): Promise<void> {
  if (!version) throw new Error('no compatible version to install');
  await getSession().installWebappFromUrl(
    deviceId,
    version.download.url,
    version.download.sha256,
    version.download.size,
    listing.sourceUrl,
    listing.app.id,
    listing.app.name,
    version.version,
  );
  if (!useCatalogStore.getState().sources.includes(listing.sourceUrl)) {
    await addSource(listing.sourceUrl);
  }
}

export function useCatalog<T>(selector: (state: CatalogState) => T): T {
  return useCatalogStore(useShallow(selector));
}

function inventoryOf(deviceId: string): InstalledReport | null {
  const held = useWebappsStore.getState().byDevice[deviceId];
  const serial = deviceSerial(useSessionStore.getState(), deviceId);
  if (!held?.listed || !serial) return null;

  return {
    deviceId: serial,
    apps: held.list
      .filter(info => info.source === 'installed' && info.provenance)
      .map(info => ({
        appId: info.id,
        sourceUrl: info.provenance!,
        version: info.version,
      })),
  };
}

let censusStarted = false;

export function startInstallCensus(): void {
  if (censusStarted) return;
  censusStarted = true;

  const sweep = () => {
    for (const peer of connectedPeers(useSessionStore.getState().peers)) {
      const inventory = inventoryOf(peer.id);
      if (inventory) reportInstalled(inventory);
    }
  };

  useWebappsStore.subscribe(sweep);
  useSessionStore.subscribe(sweep);
  sweep();
}

export function settingsOrigin(
  webapp: BridgethingWebappInfo,
): BridgethingResourceOrigin | undefined {
  if (!webapp.provenance || !webapp.settingsHash) return undefined;
  const { catalogs, merged } = useCatalogStore.getState();
  const hosted = settingsOriginFor(
    [...catalogs, ...merged],
    webapp.provenance,
    webapp.id,
    webapp.settingsHash,
  );
  if (!hosted) return undefined;
  return {
    url: hosted.url,
    sha256: hosted.sha256,
    size: hosted.size,
    mime: SETTINGS_PAGE_MIME,
  };
}
