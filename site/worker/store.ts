import { KEY_PREFIX, keyFor, type SourceRecord } from './directory.ts';

export type KvLike = {
  get(key: string): Promise<string | null>;
  put(key: string, value: string, options?: { expirationTtl?: number }): Promise<unknown>;
  delete(key: string): Promise<unknown>;
  list(options: {
    prefix?: string;
    cursor?: string;
  }): Promise<{ keys: { name: string }[]; list_complete: boolean; cursor?: string }>;
};

export async function readRecord<T>(kv: KvLike, key: string): Promise<T | null> {
  const raw = await kv.get(key);
  if (raw === null) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

type Snapshot<T> = { version: number; writer: string; items: T[] };

export const SNAPSHOT_MERGE_ATTEMPTS = 4;

function isSnapshot<T>(value: unknown): value is Snapshot<T> {
  if (typeof value !== 'object' || value === null) return false;
  const held = value as Partial<Snapshot<T>>;
  return typeof held.version === 'number' && typeof held.writer === 'string' && Array.isArray(held.items);
}

export async function readSnapshot<T>(kv: KvLike, key: string): Promise<Snapshot<T> | null> {
  const parsed = await readRecord<unknown>(kv, key);
  return isSnapshot<T>(parsed) ? parsed : null;
}

async function stamp<T>(kv: KvLike, key: string, held: Snapshot<T> | null, items: T[]): Promise<string> {
  const writer = crypto.randomUUID();
  await kv.put(key, JSON.stringify({ version: (held?.version ?? 0) + 1, writer, items } satisfies Snapshot<T>));
  return writer;
}

export async function writeSnapshot<T>(kv: KvLike, key: string, items: T[]): Promise<void> {
  await stamp(kv, key, await readSnapshot<T>(kv, key), items);
}

export type KeyedRecord<T> = { key: string; record: T };

export async function walkEntries<T>(kv: KvLike, prefix: string): Promise<KeyedRecord<T>[]> {
  const out: KeyedRecord<T>[] = [];
  let cursor: string | undefined;

  do {
    const page = await kv.list({ prefix, cursor });
    for (const { name } of page.keys) {
      const record = await readRecord<T>(kv, name);
      if (record !== null) out.push({ key: name, record });
    }
    cursor = page.list_complete ? undefined : page.cursor;
  } while (cursor);

  return out;
}

export async function walkRecords<T>(kv: KvLike, prefix: string): Promise<T[]> {
  return (await walkEntries<T>(kv, prefix)).map(entry => entry.record);
}

const SOURCE_SNAPSHOT_KEY = 'directory:snapshot';

export async function readSource(kv: KvLike, url: string): Promise<SourceRecord | null> {
  return readRecord<SourceRecord>(kv, keyFor(url));
}

function messageOf(reason: unknown): string {
  return reason instanceof Error ? reason.message : String(reason);
}

async function invalidateSnapshot(kv: KvLike, key: string, name: string, reason: string): Promise<void> {
  console.warn(`snapshot ${key} not updated for ${name}: ${reason}; deleting it for a lazy rebuild`);
  try {
    await kv.delete(key);
  } catch (refused) {
    console.warn(`snapshot ${key} delete refused for ${name}: ${messageOf(refused)}`);
  }
}

export async function mergeIntoSnapshot<T>(args: {
  kv: KvLike;
  key: string;
  prefix: string;
  record: T;
  identity: (record: T) => string;
}): Promise<void> {
  const { kv, key, prefix, record, identity } = args;
  const listed = await walkRecords<T>(kv, prefix);
  const name = identity(record);

  for (let attempt = 0; attempt < SNAPSHOT_MERGE_ATTEMPTS; attempt += 1) {
    const held = await readSnapshot<T>(kv, key);

    const merged = new Map<string, T>();
    for (const each of held?.items ?? []) merged.set(identity(each), each);
    for (const each of listed) merged.set(identity(each), each);
    merged.set(name, record);

    let writer: string;
    try {
      writer = await stamp(kv, key, held, [...merged.values()]);
    } catch (refused) {
      return invalidateSnapshot(kv, key, name, `put refused: ${messageOf(refused)}`);
    }

    if ((await readSnapshot<T>(kv, key))?.writer === writer) return;
  }

  await invalidateSnapshot(kv, key, name, `lost ${SNAPSHOT_MERGE_ATTEMPTS} merge races`);
}

export async function writeSource(kv: KvLike, record: SourceRecord): Promise<void> {
  await kv.put(keyFor(record.url), JSON.stringify(record));
  await mergeIntoSnapshot({ kv, key: SOURCE_SNAPSHOT_KEY, prefix: KEY_PREFIX, record, identity: held => held.url });
}

export async function rebuildSnapshot<T>(args: {
  kv: KvLike;
  key: string;
  prefix: string;
  keyOf: (record: T) => string;
}): Promise<T[]> {
  const { kv, key, prefix, keyOf } = args;
  const held = await readSnapshot<T>(kv, key);
  const merged = new Map<string, T>((await walkEntries<T>(kv, prefix)).map(entry => [entry.key, entry.record]));

  for (const item of held?.items ?? []) {
    const backing = keyOf(item);
    if (merged.has(backing)) continue;
    const record = await readRecord<T>(kv, backing);
    if (record !== null) merged.set(backing, record);
  }

  const records = [...merged.values()];
  await writeSnapshot(kv, key, records);
  return records;
}

export async function rebuildSources(kv: KvLike): Promise<SourceRecord[]> {
  return rebuildSnapshot<SourceRecord>({
    kv,
    key: SOURCE_SNAPSHOT_KEY,
    prefix: KEY_PREFIX,
    keyOf: record => keyFor(record.url),
  });
}

export async function listSources(kv: KvLike): Promise<SourceRecord[]> {
  return (await readSnapshot<SourceRecord>(kv, SOURCE_SNAPSHOT_KEY))?.items ?? (await rebuildSources(kv));
}

const RATE_LIMIT_PREFIX = 'rl:';

export async function takeRateLimitToken(
  kv: KvLike,
  client: string,
  limit: number,
  windowSeconds: number,
): Promise<boolean> {
  const key = `${RATE_LIMIT_PREFIX}${client}`;
  const current = Number.parseInt((await kv.get(key)) ?? '0', 10);
  const used = Number.isNaN(current) ? 0 : current;
  if (used >= limit) return false;
  await kv.put(key, String(used + 1), { expirationTtl: windowSeconds });
  return true;
}
