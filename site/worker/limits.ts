export type Limit = { bucket: string; limit: number; windowSeconds: number; refusal: string };

const DAY = 86_400;
const HOUR = 3_600;

export const NEW_SOURCES: Limit = {
  bucket: 'source:new',
  limit: 3,
  windowSeconds: DAY,
  refusal: 'at most 3 new sources per day from one network. try again tomorrow.',
};

export const SOURCE_REFRESHES: Limit = {
  bucket: 'source:refresh',
  limit: 6,
  windowSeconds: HOUR,
  refusal: 'this source was refreshed 6 times in the last hour. try again later.',
};

export const NEW_DEVICES: Limit = {
  bucket: 'device:new',
  limit: 5,
  windowSeconds: DAY,
  refusal: 'at most 5 new devices per day from one network.',
};

export const INVENTORY_CHANGES: Limit = {
  bucket: 'device:change',
  limit: 24,
  windowSeconds: DAY,
  refusal: 'this device changed its apps 24 times today. try again tomorrow.',
};

const PRUNE_HORIZON_SECONDS = 2 * DAY;

function windowStart(now: string, windowSeconds: number): number {
  const seconds = Math.floor(Date.parse(now) / 1000);
  return seconds - (seconds % windowSeconds);
}

export async function takeToken(db: D1Database, limit: Limit, key: string, now: string): Promise<boolean> {
  const taken = await db
    .prepare(
      `INSERT INTO rate_limits (bucket, key, window_start, used) VALUES (?1, ?2, ?3, 1)
       ON CONFLICT (bucket, key) DO UPDATE SET
         used = CASE WHEN window_start = excluded.window_start THEN used + 1 ELSE 1 END,
         window_start = excluded.window_start
       WHERE window_start != excluded.window_start OR used < ?4
       RETURNING used`,
    )
    .bind(limit.bucket, key, windowStart(now, limit.windowSeconds), limit.limit)
    .first<{ used: number }>();
  return taken !== null;
}

export async function pruneTokens(db: D1Database, now: string): Promise<void> {
  const horizon = Math.floor(Date.parse(now) / 1000) - PRUNE_HORIZON_SECONDS;
  await db.prepare('DELETE FROM rate_limits WHERE window_start < ?1').bind(horizon).run();
}
