import { OFFICIAL_CATALOG_URL, type Catalog } from '@bridgething/catalog';
import { clamp, DESCRIPTION_MAX_LEN, NAME_MAX_LEN } from './directory.ts';
import { probeCatalog, type HeldCatalog, type ProbeResult } from './probe.ts';

export const REFRESH_BATCH = 16;

export type CatalogRow = {
  url: string;
  body: string | null;
  etag: string | null;
  last_modified: string | null;
  name: string;
  description: string | null;
  homepage: string | null;
  icon: string | null;
  app_count: number;
  downloads_cors_ok: number | null;
  checked_at: string;
  check_ok: number;
  check_error: string | null;
};

function heldOf(row: CatalogRow): HeldCatalog | null {
  return row.body === null ? null : { body: row.body, etag: row.etag, lastModified: row.last_modified };
}

export function storeCatalog(
  db: D1Database,
  url: string,
  fetched: {
    catalog: Catalog;
    body: string;
    etag: string | null;
    lastModified: string | null;
    downloadsCorsOk: boolean | null;
  },
  now: string,
): D1PreparedStatement {
  const { repo, apps } = fetched.catalog;
  return db
    .prepare(
      `INSERT INTO catalogs (url, body, etag, last_modified, name, description, homepage, icon, app_count,
         downloads_cors_ok, checked_at, check_ok, check_error)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, NULL)
       ON CONFLICT (url) DO UPDATE SET body = excluded.body, etag = excluded.etag,
         last_modified = excluded.last_modified, name = excluded.name, description = excluded.description,
         homepage = excluded.homepage, icon = excluded.icon, app_count = excluded.app_count,
         downloads_cors_ok = excluded.downloads_cors_ok, checked_at = excluded.checked_at, check_ok = 1,
         check_error = NULL`,
    )
    .bind(
      url,
      fetched.body,
      fetched.etag,
      fetched.lastModified,
      clamp(repo.name, NAME_MAX_LEN),
      repo.description ? clamp(repo.description, DESCRIPTION_MAX_LEN) : null,
      repo.homepage,
      repo.icon,
      apps.length,
      fetched.downloadsCorsOk === null ? null : Number(fetched.downloadsCorsOk),
      now,
    );
}

function recordProbe(db: D1Database, url: string, probe: ProbeResult, now: string): D1PreparedStatement {
  if (!probe.ok) {
    return db
      .prepare('UPDATE catalogs SET checked_at = ?2, check_ok = 0, check_error = ?3 WHERE url = ?1')
      .bind(url, now, probe.reason);
  }
  if (probe.changed) return storeCatalog(db, url, probe, now);
  return db
    .prepare(
      `UPDATE catalogs SET etag = ?2, last_modified = ?3, checked_at = ?4, check_ok = 1, check_error = NULL
       WHERE url = ?1`,
    )
    .bind(url, probe.etag, probe.lastModified, now);
}

export async function readCatalog(db: D1Database, url: string): Promise<CatalogRow | null> {
  return db.prepare('SELECT * FROM catalogs WHERE url = ?1').bind(url).first<CatalogRow>();
}

type Target = { url: string; known: boolean; held: HeldCatalog | null };

async function stalest(db: D1Database, budget: number): Promise<Target[]> {
  const [missing, held] = await db.batch<{ url: string } | CatalogRow>([
    db.prepare('SELECT ?1 AS url WHERE NOT EXISTS (SELECT 1 FROM catalogs WHERE url = ?1)').bind(OFFICIAL_CATALOG_URL),
    db
      .prepare(
        `SELECT c.* FROM catalogs c LEFT JOIN sources s ON s.url = c.url
         WHERE c.url = ?1 OR s.status != 'rejected'
         ORDER BY c.checked_at ASC, c.url ASC LIMIT ?2`,
      )
      .bind(OFFICIAL_CATALOG_URL, budget),
  ]);

  return [
    ...(missing!.results as { url: string }[]).map(row => ({ url: row.url, known: false, held: null })),
    ...(held!.results as CatalogRow[]).map(row => ({ url: row.url, known: true, held: heldOf(row) })),
  ].slice(0, budget);
}

export async function refreshCatalogs(args: {
  db: D1Database;
  now: string;
  budget?: number;
  fetchImpl?: typeof fetch;
}): Promise<{ url: string; probe: ProbeResult }[]> {
  const { db, now, budget = REFRESH_BATCH, fetchImpl } = args;

  const targets = await stalest(db, budget);
  const probed = await Promise.all(
    targets.map(async ({ url, known, held }) => ({ url, known, probe: await probeCatalog(url, { held, fetchImpl }) })),
  );

  const writes = probed
    .filter(({ known, probe }) => known || probe.ok)
    .map(({ url, probe }) => recordProbe(db, url, probe, now));
  if (writes.length > 0) await db.batch(writes);

  return probed.map(({ url, probe }) => ({ url, probe }));
}
