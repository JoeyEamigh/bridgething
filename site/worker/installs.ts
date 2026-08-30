import { OFFICIAL_CATALOG_URL, type InstallCount } from '@bridgething/catalog';
import { isPublished, normalizeSourceUrl, SourceUrlError } from './directory.ts';
import { mergeIntoSnapshot, readRecord, readSnapshot, readSource, rebuildSnapshot, type KvLike } from './store.ts';

const INSTALL_PREFIX = 'install:';
const INSTALL_SNAPSHOT_KEY = 'directory:installs';

const APP_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const VERSION_MAX_LEN = 40;

export type InstallRecord = {
  app_id: string;
  source_url: string;
  count: number;
  last_at: string;
  last_version: string | null;
};

export type InstallOutcome = { ok: true; record: InstallRecord } | { ok: false; status: number; reason: string };

export function installKeyFor(appId: string, sourceUrl: string): string {
  return `${INSTALL_PREFIX}${appId}:${sourceUrl}`;
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
    .map(record => ({ app_id: record.app_id, source_url: record.source_url, count: record.count }));
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
  const trimmedVersion = typeof rawVersion === 'string' ? rawVersion.trim().slice(0, VERSION_MAX_LEN) : '';

  if (!(await counted(kv, sourceUrl))) {
    return { ok: false, status: 404, reason: 'only sources published in the directory are counted' };
  }

  return { ok: true, record: await bump(kv, { appId, sourceUrl, version: trimmedVersion || null, now }) };
}

async function counted(kv: KvLike, sourceUrl: string): Promise<boolean> {
  if (sourceUrl === OFFICIAL_CATALOG_URL) return true;
  const record = await readSource(kv, sourceUrl);
  return record !== null && isPublished(record);
}

async function bump(
  kv: KvLike,
  args: { appId: string; sourceUrl: string; version: string | null; now: string },
): Promise<InstallRecord> {
  const key = installKeyFor(args.appId, args.sourceUrl);
  const held = await readRecord<InstallRecord>(kv, key);
  const count = Number.isFinite(held?.count) ? Math.max(0, Math.trunc(held!.count)) : 0;

  const next: InstallRecord = {
    app_id: args.appId,
    source_url: args.sourceUrl,
    count: count + 1,
    last_at: args.now,
    last_version: args.version,
  };
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
