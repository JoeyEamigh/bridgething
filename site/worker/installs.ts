import { OFFICIAL_CATALOG_URL, type InstallCount, type TrendingCount } from '@bridgething/catalog';
import { normalizeSourceUrl, SourceUrlError } from './directory.ts';

const MAX_APPS = 200;

const APP_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SERIAL = /^[0-9]{4}[A-Z][0-9A-Z]{7}$/;
const VERSION_MAX_LEN = 40;

export const TRENDING_WINDOW_MS = 7 * 24 * 60 * 60 * 1000;

export const UNKNOWN_VERSION = 'unknown';

export type DeviceApp = {
  app_id: string;
  source_url: string;
  version: string;
};

export type DeviceRecord = {
  serial: string;
  apps: DeviceApp[];
  updated_at: string;
};

type Failure = { ok: false; status: number; reason: string };

export type InstalledOutcome = { ok: true; record: DeviceRecord } | Failure;

export type InventoryChange = { known: boolean; serial: string };

function appKey(app: { app_id: string; source_url: string }): string {
  return `${app.app_id} ${app.source_url}`;
}

function parsed(raw: unknown): DeviceApp | null {
  if (typeof raw !== 'object' || raw === null) return null;
  const entry = raw as Record<string, unknown>;

  const rawId = entry['app_id'];
  if (typeof rawId !== 'string') return null;
  const appId = rawId.trim().toLowerCase();
  if (!APP_ID.test(appId)) return null;

  const rawSource = entry['source_url'];
  if (typeof rawSource !== 'string') return null;
  let sourceUrl: string;
  try {
    sourceUrl = normalizeSourceUrl(rawSource);
  } catch (reason) {
    if (reason instanceof SourceUrlError) return null;
    throw reason;
  }

  const rawVersion = entry['version'];
  const version =
    (typeof rawVersion === 'string' ? rawVersion.trim().slice(0, VERSION_MAX_LEN) : '') || UNKNOWN_VERSION;

  return { app_id: appId, source_url: sourceUrl, version };
}

async function countedSources(db: D1Database, urls: string[]): Promise<Set<string>> {
  const counted = new Set<string>(urls.filter(url => url === OFFICIAL_CATALOG_URL));
  const asked = urls.filter(url => url !== OFFICIAL_CATALOG_URL);
  if (asked.length === 0) return counted;

  const { results } = await db
    .prepare(
      `SELECT s.url FROM sources s, json_each(?1) asked
       WHERE s.url = asked.value AND s.status IN ('listed', 'attested')`,
    )
    .bind(JSON.stringify(asked))
    .all<{ url: string }>();
  for (const row of results) counted.add(row.url);
  return counted;
}

function byIdThenSource(a: DeviceApp, b: DeviceApp): number {
  return a.app_id.localeCompare(b.app_id) || a.source_url.localeCompare(b.source_url);
}

export async function recordInstalled(args: {
  db: D1Database;
  body: Record<string, unknown> | null;
  now: string;
  admit: (change: InventoryChange) => Promise<Failure | null>;
}): Promise<InstalledOutcome> {
  const { db, body, now, admit } = args;

  const rawSerial = body?.['device_id'];
  if (typeof rawSerial !== 'string') {
    return { ok: false, status: 400, reason: 'send a json body with a "device_id" string' };
  }
  const serial = rawSerial.trim().toUpperCase();
  if (!SERIAL.test(serial)) return { ok: false, status: 400, reason: '"device_id" must be a car thing serial' };

  const rawApps = body?.['apps'];
  if (!Array.isArray(rawApps)) return { ok: false, status: 400, reason: 'send a json body with an "apps" array' };
  if (rawApps.length > MAX_APPS) {
    return { ok: false, status: 400, reason: `a device reports at most ${MAX_APPS} apps` };
  }

  const candidates = new Map<string, DeviceApp>();
  for (const raw of rawApps) {
    const app = parsed(raw);
    if (app) candidates.set(appKey(app), app);
  }

  const counted = await countedSources(db, [...new Set([...candidates.values()].map(app => app.source_url))]);
  const apps = [...candidates.values()].filter(app => counted.has(app.source_url)).sort(byIdThenSource);
  const record: DeviceRecord = { serial, apps, updated_at: now };

  const [device, held] = await db.batch([
    db.prepare('SELECT updated_at FROM devices WHERE serial = ?1').bind(serial),
    db.prepare('SELECT app_id, source_url, version, added_at FROM device_apps WHERE serial = ?1').bind(serial),
  ]);
  const known = device!.results.length > 0;
  const before = new Map((held!.results as (DeviceApp & { added_at: string | null })[]).map(app => [appKey(app), app]));

  const dropped = [...before.values()].filter(app => {
    const kept = candidates.get(appKey(app));
    return kept === undefined || !counted.has(kept.source_url) || kept.version !== app.version;
  });
  const added = apps
    .filter(app => before.get(appKey(app))?.version !== app.version)
    .map(app => {
      const prior = before.get(appKey(app));
      return { ...app, added_at: prior ? prior.added_at : now };
    });

  if (known && dropped.length === 0 && added.length === 0) return { ok: true, record };

  const refused = await admit({ known, serial });
  if (refused !== null) return refused;

  await db.batch([
    db
      .prepare(
        `INSERT INTO devices (serial, updated_at) VALUES (?1, ?2)
         ON CONFLICT (serial) DO UPDATE SET updated_at = excluded.updated_at`,
      )
      .bind(serial, now),
    db
      .prepare(
        `DELETE FROM device_apps WHERE serial = ?1 AND EXISTS (
           SELECT 1 FROM json_each(?2) gone
           WHERE gone.value ->> 'app_id' = device_apps.app_id AND gone.value ->> 'source_url' = device_apps.source_url)`,
      )
      .bind(serial, JSON.stringify(dropped)),
    db
      .prepare(
        `INSERT INTO device_apps (serial, app_id, source_url, version, added_at)
         SELECT ?1, value ->> 'app_id', value ->> 'source_url', value ->> 'version', value ->> 'added_at'
         FROM json_each(?2)`,
      )
      .bind(serial, JSON.stringify(added)),
  ]);

  return { ok: true, record };
}

export function installCountsStatement(db: D1Database): D1PreparedStatement {
  return db.prepare('SELECT app_id, source_url, version, devices FROM install_counts');
}

type CountRow = { app_id: string; source_url: string; version: string; devices: number };

export function toInstallCounts(rows: unknown[]): InstallCount[] {
  const tallies = new Map<string, InstallCount>();
  for (const row of rows as CountRow[]) {
    const key = appKey(row);
    const held = tallies.get(key) ?? { app_id: row.app_id, source_url: row.source_url, count: 0, versions: {} };
    held.count += row.devices;
    held.versions[row.version] = row.devices;
    tallies.set(key, held);
  }

  return [...tallies.values()].sort(
    (a, b) => b.count - a.count || a.app_id.localeCompare(b.app_id) || a.source_url.localeCompare(b.source_url),
  );
}

export function trendingStatement(db: D1Database, now: string): D1PreparedStatement {
  const since = new Date(Date.parse(now) - TRENDING_WINDOW_MS).toISOString();
  return db
    .prepare(
      `SELECT app_id, source_url, COUNT(*) AS count FROM device_apps
       WHERE added_at >= ?1 GROUP BY app_id, source_url`,
    )
    .bind(since);
}

export function toTrending(rows: unknown[]): TrendingCount[] {
  return (rows as TrendingCount[])
    .map(row => ({ app_id: row.app_id, source_url: row.source_url, count: Number(row.count) }))
    .sort((a, b) => b.count - a.count || a.app_id.localeCompare(b.app_id) || a.source_url.localeCompare(b.source_url));
}

export async function listInstallCounts(db: D1Database): Promise<InstallCount[]> {
  return toInstallCounts((await installCountsStatement(db).all()).results);
}
