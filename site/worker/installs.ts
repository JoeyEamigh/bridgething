import { OFFICIAL_CATALOG_URL, type InstallCount } from '@bridgething/catalog';
import { isPublished, normalizeSourceUrl, SourceUrlError } from './directory.ts';
import {
  mergeIntoSnapshot,
  readRecord,
  readSnapshot,
  readSource,
  rebuildSnapshot,
  walkEntries,
  walkRecords,
  writeSnapshot,
  type KvLike,
} from './store.ts';

const INSTALL_PREFIX = 'install:';
const INSTALL_SNAPSHOT_KEY = 'directory:installs';
const DEVICE_PREFIX = 'device:';
const MAX_APPS = 200;

const APP_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const SERIAL = /^[0-9]{4}[A-Z][0-9A-Z]{7}$/;
const VERSION_MAX_LEN = 40;

export const UNKNOWN_VERSION = 'unknown';

export type InstallRecord = {
  app_id: string;
  source_url: string;
  count: number;
  versions: Record<string, number>;
  last_at: string;
};

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

export type InstalledOutcome = { ok: true; record: DeviceRecord } | { ok: false; status: number; reason: string };

export function installKeyFor(appId: string, sourceUrl: string): string {
  return `${INSTALL_PREFIX}${appId}:${sourceUrl}`;
}

export function deviceKeyFor(serial: string): string {
  return `${DEVICE_PREFIX}${serial}`;
}

export async function rebuildInstalls(kv: KvLike): Promise<InstallRecord[]> {
  return rebuildSnapshot<InstallRecord>({
    kv,
    key: INSTALL_SNAPSHOT_KEY,
    prefix: INSTALL_PREFIX,
    keyOf: record => installKeyFor(record.app_id, record.source_url),
  });
}

export async function listInstalls(kv: KvLike): Promise<InstallRecord[]> {
  return (await readSnapshot<InstallRecord>(kv, INSTALL_SNAPSHOT_KEY))?.items ?? (await rebuildInstalls(kv));
}

export function toInstallCounts(records: InstallRecord[]): InstallCount[] {
  return records
    .filter(record => Number.isFinite(record.count) && record.count > 0)
    .sort((a, b) => b.count - a.count || a.app_id.localeCompare(b.app_id) || a.source_url.localeCompare(b.source_url))
    .map(record => ({
      app_id: record.app_id,
      source_url: record.source_url,
      count: record.count,
      versions: { ...record.versions },
    }));
}

export async function recountInstalls(kv: KvLike): Promise<InstallRecord[]> {
  const devices = await walkRecords<DeviceRecord>(kv, DEVICE_PREFIX);
  const derived = new Map<string, InstallRecord>();

  for (const device of devices) {
    for (const app of Array.isArray(device.apps) ? device.apps : []) {
      if (typeof app?.app_id !== 'string' || typeof app?.source_url !== 'string') continue;
      const key = installKeyFor(app.app_id, app.source_url);
      const held = derived.get(key) ?? {
        app_id: app.app_id,
        source_url: app.source_url,
        count: 0,
        versions: {},
        last_at: device.updated_at,
      };

      held.count += 1;
      held.versions = shift(held.versions, null, app.version || UNKNOWN_VERSION);
      if (device.updated_at > held.last_at) held.last_at = device.updated_at;
      derived.set(key, held);
    }
  }

  for (const [key, record] of derived) {
    const held = await readRecord<InstallRecord>(kv, key);
    if (held && held.count === record.count && sameVersions(held.versions, record.versions)) continue;
    await kv.put(key, JSON.stringify(record));
  }

  for (const { key } of await walkEntries<InstallRecord>(kv, INSTALL_PREFIX)) {
    if (!derived.has(key)) await kv.delete(key);
  }

  const records = [...derived.values()];
  await writeSnapshot(kv, INSTALL_SNAPSHOT_KEY, records);
  return records;
}

export async function recordInstalled(args: {
  kv: KvLike;
  body: Record<string, unknown> | null;
  now: string;
}): Promise<InstalledOutcome> {
  const { kv, body, now } = args;

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

  const held = new Map<string, DeviceApp>();
  for (const raw of rawApps) {
    const app = await accept(kv, raw);
    if (app) held.set(installKeyFor(app.app_id, app.source_url), app);
  }

  const apps = [...held.values()].sort(
    (a, b) => a.app_id.localeCompare(b.app_id) || a.source_url.localeCompare(b.source_url),
  );
  const record: DeviceRecord = { serial, apps, updated_at: now };

  const key = deviceKeyFor(serial);
  const before = await readRecord<DeviceRecord>(kv, key);
  const known = new Map(
    (Array.isArray(before?.apps) ? before.apps : []).map(app => [installKeyFor(app.app_id, app.source_url), app]),
  );
  if (before !== null && sameApps(known, held)) return { ok: true, record: before };

  for (const [id, app] of held) {
    const was = known.get(id);
    if (was === undefined) {
      await writeInstall(kv, app.app_id, app.source_url, now, tally => ({
        ...tally,
        count: tally.count + 1,
        versions: shift(tally.versions, null, app.version),
      }));
    } else if (was.version !== app.version) {
      await writeInstall(kv, app.app_id, app.source_url, now, tally => ({
        ...tally,
        versions: shift(tally.versions, was.version, app.version),
      }));
    }
  }

  for (const [id, was] of known) {
    if (held.has(id)) continue;
    await writeInstall(kv, was.app_id, was.source_url, now, tally => ({
      ...tally,
      count: Math.max(0, tally.count - 1),
      versions: shift(tally.versions, was.version, null),
    }));
  }

  await kv.put(key, JSON.stringify(record));
  return { ok: true, record };
}

async function accept(kv: KvLike, raw: unknown): Promise<DeviceApp | null> {
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
  if (!(await counted(kv, sourceUrl))) return null;

  const rawVersion = entry['version'];
  const version =
    (typeof rawVersion === 'string' ? rawVersion.trim().slice(0, VERSION_MAX_LEN) : '') || UNKNOWN_VERSION;

  return { app_id: appId, source_url: sourceUrl, version };
}

function sameApps(before: Map<string, DeviceApp>, after: Map<string, DeviceApp>): boolean {
  if (before.size !== after.size) return false;
  for (const [id, app] of after) {
    if (before.get(id)?.version !== app.version) return false;
  }
  return true;
}

async function counted(kv: KvLike, sourceUrl: string): Promise<boolean> {
  if (sourceUrl === OFFICIAL_CATALOG_URL) return true;
  const record = await readSource(kv, sourceUrl);
  return record !== null && isPublished(record);
}

function blank(appId: string, sourceUrl: string, now: string): InstallRecord {
  return { app_id: appId, source_url: sourceUrl, count: 0, versions: {}, last_at: now };
}

function sane(held: InstallRecord | null, appId: string, sourceUrl: string, now: string): InstallRecord {
  if (held === null) return blank(appId, sourceUrl, now);
  return {
    app_id: appId,
    source_url: sourceUrl,
    count: Number.isFinite(held.count) ? Math.max(0, Math.trunc(held.count)) : 0,
    versions: shift(held.versions, null, null),
    last_at: typeof held.last_at === 'string' ? held.last_at : now,
  };
}

async function heldInstall(kv: KvLike, appId: string, sourceUrl: string, now: string): Promise<InstallRecord> {
  return sane(await readRecord<InstallRecord>(kv, installKeyFor(appId, sourceUrl)), appId, sourceUrl, now);
}

async function writeInstall(
  kv: KvLike,
  appId: string,
  sourceUrl: string,
  now: string,
  mutate: (held: InstallRecord) => InstallRecord,
): Promise<InstallRecord> {
  const key = installKeyFor(appId, sourceUrl);
  const next = { ...mutate(await heldInstall(kv, appId, sourceUrl, now)), last_at: now };

  await kv.put(key, JSON.stringify(next));
  await mergeIntoSnapshot({
    kv,
    key: INSTALL_SNAPSHOT_KEY,
    prefix: INSTALL_PREFIX,
    record: next,
    identity: held => installKeyFor(held.app_id, held.source_url),
  });

  return next;
}

function shift(versions: Record<string, number>, from: string | null, to: string | null): Record<string, number> {
  const next: Record<string, number> = {};
  for (const [key, value] of Object.entries(versions ?? {})) {
    const tally = Number.isFinite(Number(value)) ? Math.max(0, Math.trunc(Number(value))) : 0;
    if (tally > 0) next[key] = tally;
  }

  if (from !== null && next[from]) {
    const left = next[from]! - 1;
    if (left > 0) next[from] = left;
    else delete next[from];
  }
  if (to !== null) next[to] = (next[to] ?? 0) + 1;

  return next;
}

function sameVersions(a: Record<string, number>, b: Record<string, number>): boolean {
  const left = Object.entries(a ?? {}).sort(([x], [y]) => x.localeCompare(y));
  const right = Object.entries(b ?? {}).sort(([x], [y]) => x.localeCompare(y));
  if (left.length !== right.length) return false;
  return left.every(([key, value], i) => right[i]![0] === key && right[i]![1] === value);
}
