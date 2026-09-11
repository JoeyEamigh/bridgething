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
const SEEN_PREFIX = 'seen:';

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

export type SeenRecord = {
  app_id: string;
  serial: string;
  source_url: string;
  version: string;
  first_at: string;
  updated_at: string;
};

export type InstallOutcome = { ok: true; record: InstallRecord } | { ok: false; status: number; reason: string };

export function installKeyFor(appId: string, sourceUrl: string): string {
  return `${INSTALL_PREFIX}${appId}:${sourceUrl}`;
}

export function seenKeyFor(appId: string, serial: string): string {
  return `${SEEN_PREFIX}${appId}:${serial}`;
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
  const markers = await walkRecords<SeenRecord>(kv, SEEN_PREFIX);
  const derived = new Map<string, InstallRecord>();

  for (const marker of markers) {
    if (typeof marker.app_id !== 'string' || typeof marker.source_url !== 'string') continue;
    const key = installKeyFor(marker.app_id, marker.source_url);
    const held = derived.get(key) ?? {
      app_id: marker.app_id,
      source_url: marker.source_url,
      count: 0,
      versions: {},
      last_at: marker.updated_at,
    };

    held.count += 1;
    held.versions = shift(held.versions, null, marker.version || UNKNOWN_VERSION);
    if (marker.updated_at > held.last_at) held.last_at = marker.updated_at;
    derived.set(key, held);
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

export async function recordInstall(args: {
  kv: KvLike;
  body: Record<string, unknown> | null;
  now: string;
}): Promise<InstallOutcome> {
  const { kv, body, now } = args;

  const rawId = body?.['app_id'];
  if (typeof rawId !== 'string') return { ok: false, status: 400, reason: 'send a json body with an "app_id" string' };
  const appId = rawId.trim().toLowerCase();
  if (!APP_ID.test(appId)) return { ok: false, status: 400, reason: '"app_id" must be a catalog app uuid' };

  const rawSerial = body?.['device_id'];
  if (typeof rawSerial !== 'string') {
    return { ok: false, status: 400, reason: 'send a json body with a "device_id" string' };
  }
  const serial = rawSerial.trim().toUpperCase();
  if (!SERIAL.test(serial)) return { ok: false, status: 400, reason: '"device_id" must be a car thing serial' };

  const rawSource = body?.['source_url'];
  if (typeof rawSource !== 'string') {
    return { ok: false, status: 400, reason: 'send a json body with a "source_url" string' };
  }

  let sourceUrl: string;
  try {
    sourceUrl = normalizeSourceUrl(rawSource);
  } catch (reason) {
    if (reason instanceof SourceUrlError) return { ok: false, status: 400, reason: reason.message };
    throw reason;
  }

  const rawVersion = body?.['version'];
  if (rawVersion !== undefined && rawVersion !== null && typeof rawVersion !== 'string') {
    return { ok: false, status: 400, reason: '"version" must be a string or null' };
  }
  const version =
    (typeof rawVersion === 'string' ? rawVersion.trim().slice(0, VERSION_MAX_LEN) : '') || UNKNOWN_VERSION;

  if (!(await counted(kv, sourceUrl))) {
    return { ok: false, status: 404, reason: 'only sources published in the directory are counted' };
  }

  return { ok: true, record: await bump(kv, { appId, sourceUrl, serial, version, now }) };
}

async function counted(kv: KvLike, sourceUrl: string): Promise<boolean> {
  if (sourceUrl === OFFICIAL_CATALOG_URL) return true;
  const record = await readSource(kv, sourceUrl);
  return record !== null && isPublished(record);
}

type Bump = { appId: string; sourceUrl: string; serial: string; version: string; now: string };

async function bump(kv: KvLike, args: Bump): Promise<InstallRecord> {
  const seenKey = seenKeyFor(args.appId, args.serial);
  const seen = await readRecord<SeenRecord>(kv, seenKey);

  if (seen === null) {
    const record = await writeInstall(kv, args.appId, args.sourceUrl, args.now, held => ({
      ...held,
      count: held.count + 1,
      versions: shift(held.versions, null, args.version),
    }));
    await kv.put(
      seenKey,
      JSON.stringify({
        app_id: args.appId,
        serial: args.serial,
        source_url: args.sourceUrl,
        version: args.version,
        first_at: args.now,
        updated_at: args.now,
      } satisfies SeenRecord),
    );
    return record;
  }

  const home = seen.source_url;
  if (seen.version === args.version) return await heldInstall(kv, args.appId, home, args.now);

  const record = await writeInstall(kv, args.appId, home, args.now, held => ({
    ...held,
    versions: shift(held.versions, seen.version, args.version),
  }));
  await kv.put(seenKey, JSON.stringify({ ...seen, version: args.version, updated_at: args.now } satisfies SeenRecord));
  return record;
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
