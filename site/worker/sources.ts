import type { SourceStatus } from '@bridgething/catalog';
import { normalizeSourceUrl, SourceUrlError, type SourceRecord } from './directory.ts';
import { storeCatalog } from './catalogs.ts';
import { probeCatalog } from './probe.ts';

type SourceRow = {
  url: string;
  name: string;
  description: string | null;
  homepage: string | null;
  icon: string | null;
  status: SourceStatus;
  submitted_at: string;
  reviewed_at: string | null;
  reviewed_by: string | null;
  app_count: number;
  checked_at: string;
  check_ok: number;
  check_error: string | null;
  downloads_cors_ok: number | null;
  note: string | null;
};

const SOURCE_SELECT = `SELECT s.url, c.name, c.description, c.homepage, c.icon, s.status, s.submitted_at, s.reviewed_at,
  s.reviewed_by, c.app_count, c.checked_at, c.check_ok, c.check_error, c.downloads_cors_ok, s.note
  FROM sources s JOIN catalogs c ON c.url = s.url`;

function toRecord(row: SourceRow): SourceRecord {
  return {
    url: row.url,
    name: row.name,
    description: row.description,
    homepage: row.homepage,
    icon: row.icon,
    status: row.status,
    submitted_at: row.submitted_at,
    reviewed_at: row.reviewed_at,
    reviewed_by: row.reviewed_by,
    app_count: row.app_count,
    last_checked_at: row.checked_at,
    last_check_ok: row.check_ok === 1,
    last_check_error: row.check_error,
    downloads_cors_ok: row.downloads_cors_ok === null ? null : row.downloads_cors_ok === 1,
    note: row.note,
  };
}

export function listSourcesStatement(db: D1Database): D1PreparedStatement {
  return db.prepare(SOURCE_SELECT);
}

export function toRecords(rows: unknown[]): SourceRecord[] {
  return (rows as SourceRow[]).map(toRecord);
}

export async function listSources(db: D1Database): Promise<SourceRecord[]> {
  return toRecords((await listSourcesStatement(db).all()).results);
}

export async function readSource(db: D1Database, url: string): Promise<SourceRecord | null> {
  const row = await db.prepare(`${SOURCE_SELECT} WHERE s.url = ?1`).bind(url).first<SourceRow>();
  return row === null ? null : toRecord(row);
}

type Failure = { ok: false; status: number; reason: string };

function normalized(rawUrl: string): { ok: true; url: string } | Failure {
  try {
    return { ok: true, url: normalizeSourceUrl(rawUrl) };
  } catch (err) {
    if (err instanceof SourceUrlError) return { ok: false, status: 400, reason: err.message };
    throw err;
  }
}

export type SubmitOutcome = { ok: true; created: boolean; record: SourceRecord } | Failure;

export async function submitSource(args: {
  db: D1Database;
  rawUrl: string;
  now: string;
  admit: (existing: SourceRecord | null, url: string) => Promise<Failure | null>;
  fetchImpl?: typeof fetch;
}): Promise<SubmitOutcome> {
  const { db, rawUrl, now, admit, fetchImpl } = args;

  const target = normalized(rawUrl);
  if (!target.ok) return target;
  const { url } = target;

  const existing = await readSource(db, url);
  if (existing?.status === 'rejected') {
    return { ok: false, status: 403, reason: `${url} was removed from the directory and cannot be resubmitted` };
  }

  const refused = await admit(existing, url);
  if (refused !== null) return refused;

  const probe = await probeCatalog(url, { fetchImpl });
  if (!probe.ok) return { ok: false, status: 422, reason: probe.reason };
  if (!probe.changed) throw new Error('an unconditional probe always reports a change');

  await db.batch([
    storeCatalog(db, url, probe, now),
    db
      .prepare(
        `INSERT INTO sources (url, status, submitted_at) VALUES (?1, 'listed', ?2)
         ON CONFLICT (url) DO NOTHING`,
      )
      .bind(url, now),
  ]);

  const record = await readSource(db, url);
  if (record === null) throw new Error(`${url} vanished between its write and its read`);
  return { ok: true, created: existing === null, record };
}

export type ModerateOutcome = { ok: true; record: SourceRecord } | Failure;

export async function setSourceStatus(args: {
  db: D1Database;
  rawUrl: string;
  status: SourceStatus;
  reviewedBy: string;
  note?: string | null;
  now: string;
}): Promise<ModerateOutcome> {
  const { db, rawUrl, status, reviewedBy, note, now } = args;

  const target = normalized(rawUrl);
  if (!target.ok) return target;
  const { url } = target;

  const updated = await db
    .prepare(
      `UPDATE sources SET status = ?2, reviewed_at = ?3, reviewed_by = ?4,
         note = CASE WHEN ?5 THEN ?6 ELSE note END
       WHERE url = ?1 RETURNING url`,
    )
    .bind(url, status, now, reviewedBy, Number(note !== undefined), note ?? null)
    .first<{ url: string }>();
  if (updated === null) return { ok: false, status: 404, reason: `${url} is not in the directory` };

  const record = await readSource(db, url);
  if (record === null) throw new Error(`${url} vanished between its write and its read`);
  return { ok: true, record };
}
