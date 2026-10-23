import { normalizeSourceUrl, SourceUrlError } from './directory.ts';

export type RelayOutcome = { ok: true; body: string } | { ok: false; status: number; reason: string };

export async function relayCatalog(args: { db: D1Database; rawUrl: string | null }): Promise<RelayOutcome> {
  const { db, rawUrl } = args;

  if (!rawUrl) return { ok: false, status: 400, reason: 'pass the source as ?url=' };

  let url: string;
  try {
    url = normalizeSourceUrl(rawUrl);
  } catch (err) {
    if (err instanceof SourceUrlError) return { ok: false, status: 400, reason: err.message };
    throw err;
  }

  const held = await db
    .prepare(
      `SELECT c.body FROM sources s JOIN catalogs c ON c.url = s.url
       WHERE s.url = ?1 AND s.status IN ('listed', 'attested') AND c.body IS NOT NULL`,
    )
    .bind(url)
    .first<{ body: string }>();
  if (held === null) return { ok: false, status: 404, reason: `${url} is not a listed source; fetch it directly` };

  return { ok: true, body: held.body };
}
