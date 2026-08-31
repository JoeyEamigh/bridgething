import { declaresExtension } from './extension.ts';
import type {
  AppEntry,
  AppVersion,
  Catalog,
  Download,
  InstallCount,
  RecommendedSource,
  SourceCatalog,
} from './types.ts';
import { sortNewestFirst } from './versions.ts';

export type ExtensionOffering = 'listed' | 'omitted';

export function offersApp(app: AppEntry, extensions: ExtensionOffering): boolean {
  return extensions === 'listed' || !declaresExtension(app);
}

export type InstalledWebapp = {
  id: string;
  version: string;
  source: 'builtin' | 'installed';
  role: 'standard' | 'launcher';
  provenance?: string | null;
};

export type CatalogAppListing = {
  app: AppEntry;
  sourceUrl: string;
  newestCompatible: AppVersion | null;
  installedVersion: string | null;
  updateAvailable: boolean;
  alsoAvailableFrom: string[];
  installs: number;
};

export type CatalogAppUpdate = {
  appId: string;
  name: string;
  installedVersion: string;
  target: AppVersion;
  sourceUrl: string;
};

export function satisfies(deviceVersion: string, minimum: string): boolean {
  return compareVersions(deviceVersion, minimum) >= 0;
}

export function isUpgrade(candidate: string, installed: string): boolean {
  return compareVersions(candidate, installed) > 0;
}

export function compareVersions(a: string, b: string): number {
  const pa = versionComponents(a);
  const pb = versionComponents(b);
  for (let i = 0; i < Math.max(pa.length, pb.length); i += 1) {
    const x = pa[i] ?? 0;
    const y = pb[i] ?? 0;
    if (x !== y) return x < y ? -1 : 1;
  }
  return 0;
}

function versionComponents(raw: string): number[] {
  let v = raw.trim();
  if (v.startsWith('v') || v.startsWith('V')) v = v.slice(1);
  const cut = v.search(/[-+]/);
  if (cut !== -1) v = v.slice(0, cut);
  return v.split('.').map(part => {
    const n = Number.parseInt(part, 10);
    return Number.isNaN(n) ? 0 : n;
  });
}

export function pinsFrom(installed: InstalledWebapp[]): Map<string, string> {
  const out = new Map<string, string>();
  for (const info of installed) {
    if (info.provenance) out.set(info.id.toLowerCase(), info.provenance);
  }
  return out;
}

export function versionCompatible(version: AppVersion, deviceLibVersion: string | null): boolean {
  return deviceLibVersion === null || satisfies(deviceLibVersion, version.min_libbridgething_version);
}

export function newestCompatible(app: AppEntry, deviceLibVersion: string | null): AppVersion | null {
  return sortNewestFirst(app.versions).find(v => versionCompatible(v, deviceLibVersion)) ?? null;
}

export function isListedWebapp(webapp: { role: string; source: string }): boolean {
  return webapp.role !== 'launcher' || webapp.source === 'installed';
}

export function listedWebapps<T extends { role: string; source: string }>(list: T[]): T[] {
  return list.filter(isListedWebapp);
}

function installsById(counts: InstallCount[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const entry of counts) {
    const tally = Number(entry.count);
    if (!Number.isFinite(tally) || tally <= 0) continue;
    const id = entry.app_id.toLowerCase();
    out.set(id, (out.get(id) ?? 0) + tally);
  }
  return out;
}

export function aggregate(args: {
  orderedCatalogs: SourceCatalog[];
  installed: InstalledWebapp[];
  deviceLibVersion: string | null;
  installs?: InstallCount[];
  extensions: ExtensionOffering;
}): CatalogAppListing[] {
  const { orderedCatalogs, installed, deviceLibVersion, extensions } = args;
  const installedById = new Map(installed.map(i => [i.id.toLowerCase(), i]));
  const tallies = installsById(args.installs ?? []);
  const pins = pinsFrom(installed);

  const offerings = new Map<string, { url: string; app: AppEntry }[]>();
  for (const { url, catalog } of orderedCatalogs) {
    for (const app of catalog.apps) {
      if (!offersApp(app, extensions)) continue;
      const list = offerings.get(app.id.toLowerCase());
      if (list) list.push({ url, app });
      else offerings.set(app.id.toLowerCase(), [{ url, app }]);
    }
  }

  const listings: CatalogAppListing[] = [];
  for (const [id, offers] of offerings) {
    if (offers.length === 0) continue;
    const pinned = pins.get(id);
    const primary = offers.find(o => o.url === pinned) ?? offers[0]!;
    const alsoAvailableFrom = offers.filter(o => o.url !== primary.url).map(o => o.url);

    const newest = newestCompatible(primary.app, deviceLibVersion);
    const installedVersion = installedById.get(id)?.version ?? null;

    listings.push({
      app: primary.app,
      sourceUrl: primary.url,
      newestCompatible: newest,
      installedVersion,
      updateAvailable: installedVersion !== null && newest !== null && isUpgrade(newest.version, installedVersion),
      alsoAvailableFrom,
      installs: tallies.get(id) ?? 0,
    });
  }

  return listings.sort(
    (a, b) => b.installs - a.installs || a.app.name.localeCompare(b.app.name) || a.app.id.localeCompare(b.app.id),
  );
}

export function updates(args: {
  catalogs: Map<string, Catalog>;
  installed: InstalledWebapp[];
  deviceLibVersion: string | null;
  extensions: ExtensionOffering;
}): CatalogAppUpdate[] {
  const { catalogs, installed, deviceLibVersion, extensions } = args;
  const pins = pinsFrom(installed);
  const out: CatalogAppUpdate[] = [];

  for (const info of installed) {
    if (info.source !== 'installed' || info.role !== 'standard') continue;
    const id = info.id.toLowerCase();
    const sourceUrl = pins.get(id);
    if (!sourceUrl) continue;
    const app = catalogs.get(sourceUrl)?.apps.find(a => a.id.toLowerCase() === id);
    if (!app || !offersApp(app, extensions)) continue;
    const newest = newestCompatible(app, deviceLibVersion);
    if (!newest || !isUpgrade(newest.version, info.version)) continue;
    out.push({
      appId: id,
      name: app.name,
      installedVersion: info.version,
      target: newest,
      sourceUrl,
    });
  }

  return out.sort((a, b) => a.name.localeCompare(b.name) || a.appId.localeCompare(b.appId));
}

export function recommendedSources(args: {
  directory: Catalog | null;
  orderedCatalogs: SourceCatalog[];
  subscribed: string[];
}): RecommendedSource[] {
  const subscribed = new Set(args.subscribed);
  const byUrl = new Map<string, RecommendedSource>();

  for (const candidate of args.directory?.recommended_sources ?? []) {
    if (subscribed.has(candidate.url) || byUrl.has(candidate.url)) continue;
    byUrl.set(candidate.url, candidate);
  }

  for (const { catalog } of args.orderedCatalogs) {
    for (const candidate of catalog.recommended_sources) {
      if (subscribed.has(candidate.url) || byUrl.has(candidate.url)) continue;
      byUrl.set(candidate.url, { ...candidate, attested: false });
    }
  }

  return [...byUrl.values()].sort((a, b) => Number(b.attested) - Number(a.attested) || a.name.localeCompare(b.name));
}

export const SETTINGS_PAGE_MIME = 'text/html; charset=utf-8';

export function settingsOriginFor(
  catalogs: SourceCatalog[],
  provenance: string | null,
  webappId: string,
  deviceSettingsHash: string | null,
): Download | null {
  if (!provenance) return null;
  const catalog = catalogs.find(source => source.url === provenance)?.catalog ?? null;
  if (!catalog) return null;
  const id = webappId.toLowerCase();
  return settingsOrigin(catalog.apps.find(app => app.id.toLowerCase() === id) ?? null, deviceSettingsHash);
}

export function settingsOrigin(app: AppEntry | null, deviceSettingsHash: string | null): Download | null {
  if (!app || !deviceSettingsHash) return null;
  const wanted = deviceSettingsHash.toLowerCase();
  for (const version of app.versions) {
    if (version.settings?.sha256.toLowerCase() === wanted) return version.settings;
  }
  return null;
}
