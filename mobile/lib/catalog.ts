import {
  blendStoreListings,
  compareVersions,
  fetchCatalog,
  fetchMergedApps,
  fetchSources,
  normalizeSourceUrl,
  OFFICIAL_CATALOG_URL,
  aggregate,
  recommendedSources as resolveRecommended,
  reportInstall,
  updates as resolveUpdates,
  type AppVersion,
  type Catalog,
  type CatalogAppListing,
  type CatalogAppUpdate,
  type InstallCount,
  type InstalledWebapp,
  type MergedCatalog,
  type RecommendedSource,
  type SourceCatalog,
  type SourceFailure,
  type StoreListings,
} from '@bridgething/catalog';
import type { BridgethingWebappInfo } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useMemo } from 'react';
import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';

import { getSession } from './bridge';
import { isRunning, useOtaStore } from './ota';
import { connectedPeers, useSessionStore, type SessionState } from './session';
import { DEFAULT_OTA_POLL_CONFIG, storage } from './storage';
import { formatBytes } from './utils';
import { useWebappsStore } from './webapps';

const SOURCES_KEY = 'catalog.sources';

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
  sources: loadSources(),
  catalogs: [],
  directory: null,
  merged: [],
  installs: [],
  failures: [],
  refreshing: false,
  previews: {},
}));

function loadSources(): string[] {
  const raw = storage.getString(SOURCES_KEY);
  if (!raw) return [OFFICIAL_CATALOG_URL];
  try {
    const parsed: unknown = JSON.parse(raw);
    const urls = Array.isArray(parsed)
      ? parsed.filter((u): u is string => typeof u === 'string')
      : [];
    return urls.length > 0 ? urls : [OFFICIAL_CATALOG_URL];
  } catch {
    return [OFFICIAL_CATALOG_URL];
  }
}

function saveSources(urls: string[]): void {
  storage.set(SOURCES_KEY, JSON.stringify(urls));
}

let refreshGeneration = 0;

export async function refreshCatalog(): Promise<void> {
  const generation = ++refreshGeneration;
  const {
    sources,
    merged: heldMerged,
    installs: heldInstalls,
  } = useCatalogStore.getState();
  useCatalogStore.setState({ refreshing: true });

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
  const { sources } = useCatalogStore.getState();
  if (sources.includes(normalized)) return normalized;
  const next = [...sources, normalized];
  saveSources(next);
  useCatalogStore.setState({ sources: next });
  await refreshCatalog();
  return normalized;
}

export function moveSource(url: string, delta: number): void {
  const { sources, catalogs } = useCatalogStore.getState();
  const from = sources.indexOf(url);
  const to = from + delta;
  if (from === -1 || to < 0 || to >= sources.length) return;
  const next = [...sources];
  next.splice(to, 0, ...next.splice(from, 1));
  saveSources(next);
  useCatalogStore.setState({
    sources: next,
    catalogs: [...catalogs].sort(
      (a, b) => next.indexOf(a.url) - next.indexOf(b.url),
    ),
  });
}

export async function removeSource(url: string): Promise<void> {
  const { sources, catalogs } = useCatalogStore.getState();
  if (!sources.includes(url)) return;
  const next = sources.filter(u => u !== url);
  saveSources(next);
  useCatalogStore.setState({
    sources: next,
    catalogs: catalogs.filter(e => e.url !== url),
  });
  await refreshCatalog();
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
  );
  reportInstall({
    appId: listing.app.id,
    sourceUrl: listing.sourceUrl,
    version: version.version,
  });
  if (!useCatalogStore.getState().sources.includes(listing.sourceUrl)) {
    await addSource(listing.sourceUrl);
  }
}

export function useCatalog<T>(selector: (state: CatalogState) => T): T {
  return useCatalogStore(useShallow(selector));
}

function pendingUpdates(deviceId: string): CatalogAppUpdate[] {
  const { catalogs } = useCatalogStore.getState();
  const installed = useWebappsStore.getState().byDevice[deviceId]?.list ?? [];
  return resolveUpdates({
    catalogs: new Map(catalogs.map(e => [e.url, e.catalog])),
    installed: toInstalled(installed),
    deviceLibVersion: deviceLibVersion(useSessionStore.getState(), deviceId),
    extensions: 'omitted',
  });
}

async function installUpdate(
  deviceId: string,
  update: CatalogAppUpdate,
): Promise<void> {
  const app = useCatalogStore
    .getState()
    .catalogs.find(c => c.url === update.sourceUrl)
    ?.catalog.apps.find(a => a.id.toLowerCase() === update.appId);
  await getSession().installWebappFromUrl(
    deviceId,
    update.target.download.url,
    update.target.download.sha256,
    update.target.download.size,
    update.sourceUrl,
    app?.id ?? update.appId,
    update.name,
  );
}

const attemptedAutoUpdates = new Set<string>();
let autoUpdateRunning = false;
let autoUpdateStarted = false;

function nextAutoUpdate(): {
  deviceId: string;
  update: CatalogAppUpdate;
  key: string;
} | null {
  const session = useSessionStore.getState();
  const autoPush =
    session.otaPollConfig?.autoPush ?? DEFAULT_OTA_POLL_CONFIG.autoPush;
  if (!autoPush) return null;

  for (const peer of connectedPeers(session.peers)) {
    if (isRunning(useOtaStore.getState().runs[peer.id])) continue;
    for (const update of pendingUpdates(peer.id)) {
      const key = `${peer.id}:${update.appId}@${update.target.version}`;
      if (attemptedAutoUpdates.has(key)) continue;
      return { deviceId: peer.id, update, key };
    }
  }
  return null;
}

async function sweepAutoUpdates(): Promise<void> {
  if (autoUpdateRunning) return;
  autoUpdateRunning = true;
  try {
    for (;;) {
      const target = nextAutoUpdate();
      if (!target) return;
      attemptedAutoUpdates.add(target.key);
      try {
        await installUpdate(target.deviceId, target.update);
      } catch {
        continue;
      }
    }
  } finally {
    autoUpdateRunning = false;
  }
}

export function startWebappAutoUpdate(): void {
  if (autoUpdateStarted) return;
  autoUpdateStarted = true;
  const check = () => void sweepAutoUpdates();
  useCatalogStore.subscribe(check);
  useWebappsStore.subscribe(check);
  useSessionStore.subscribe(check);
  useOtaStore.subscribe(check);
  check();
}
