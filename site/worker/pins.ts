import { OFFICIAL_CATALOG_URL, type StorePin } from '@bridgething/catalog';
import { normalizeSourceUrl, SourceUrlError } from './directory.ts';

const APP_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export type PinOutcome = { ok: true; pin: StorePin | null } | { ok: false; status: number; reason: string };

export function featuredPinStatement(db: D1Database): D1PreparedStatement {
  return db.prepare(`SELECT app_id, source_url FROM store_pins WHERE slot = 'featured'`);
}

export function toPin(rows: unknown[]): StorePin | null {
  const row = (rows as StorePin[])[0];
  return row ? { app_id: row.app_id, source_url: row.source_url } : null;
}

export async function featuredPin(db: D1Database): Promise<StorePin | null> {
  return toPin((await featuredPinStatement(db).all()).results);
}

type ListedApps = { body: string | null };

async function listsApp(db: D1Database, sourceUrl: string, appId: string): Promise<boolean> {
  const row = await db
    .prepare(
      `SELECT c.body FROM catalogs c LEFT JOIN sources s ON s.url = c.url
       WHERE c.url = ?1 AND (c.url = ?2 OR s.status IN ('listed', 'attested'))`,
    )
    .bind(sourceUrl, OFFICIAL_CATALOG_URL)
    .first<ListedApps>();
  if (!row?.body) return false;
  const apps = (JSON.parse(row.body) as { apps?: { id: string }[] }).apps ?? [];
  return apps.some(app => app.id.toLowerCase() === appId);
}

export async function pinFeatured(args: {
  db: D1Database;
  body: Record<string, unknown> | null;
  now: string;
}): Promise<PinOutcome> {
  const { db, body, now } = args;

  const rawId = body?.['app_id'];
  const rawSource = body?.['source_url'];
  if (typeof rawId !== 'string' || typeof rawSource !== 'string') {
    return { ok: false, status: 400, reason: 'send a json body with "app_id" and "source_url" strings' };
  }
  const appId = rawId.trim().toLowerCase();
  if (!APP_ID.test(appId)) return { ok: false, status: 400, reason: '"app_id" must be a uuid' };

  let sourceUrl: string;
  try {
    sourceUrl = normalizeSourceUrl(rawSource);
  } catch (reason) {
    if (reason instanceof SourceUrlError) return { ok: false, status: 400, reason: reason.message };
    throw reason;
  }

  if (!(await listsApp(db, sourceUrl, appId))) {
    return { ok: false, status: 404, reason: 'no published source in the directory lists that app' };
  }

  await db
    .prepare(
      `INSERT INTO store_pins (slot, app_id, source_url, pinned_at) VALUES ('featured', ?1, ?2, ?3)
       ON CONFLICT (slot) DO UPDATE SET app_id = excluded.app_id, source_url = excluded.source_url,
         pinned_at = excluded.pinned_at`,
    )
    .bind(appId, sourceUrl, now)
    .run();
  return { ok: true, pin: { app_id: appId, source_url: sourceUrl } };
}

export async function unpinFeatured(db: D1Database): Promise<PinOutcome> {
  await db.prepare(`DELETE FROM store_pins WHERE slot = 'featured'`).run();
  return { ok: true, pin: null };
}
