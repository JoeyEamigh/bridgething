import { OFFICIAL_CATALOG_URL, type SourceStatus } from '@bridgething/catalog';
import categoryBackfill from './category-backfill.json' with { type: 'json' };
import { byAttestedThenName, toDirectoryView } from './directory.ts';
import { installCountsStatement, toInstallCounts, toTrending, trendingStatement } from './installs.ts';
import { featuredPinStatement, toPin } from './pins.ts';
import { listSourcesStatement, toRecords } from './sources.ts';

type MergeRow = { url: string; body: string | null; name: string; status: SourceStatus; check_error: string | null };

const UNFETCHED = 'the directory has not fetched this catalog yet';

function mergeStatement(db: D1Database): D1PreparedStatement {
  return db
    .prepare(
      `SELECT c.url, c.body, c.name, s.status, c.check_error FROM catalogs c LEFT JOIN sources s ON s.url = c.url
       WHERE c.url = ?1 OR s.status IN ('listed', 'attested')`,
    )
    .bind(OFFICIAL_CATALOG_URL);
}

type MergeInputs = { rows: MergeRow[]; installs: unknown[]; trending: unknown[]; pin: unknown[] };

function merged({ rows, installs, trending, pin }: MergeInputs, now: string): string {
  const official = rows.find(row => row.url === OFFICIAL_CATALOG_URL);
  const community = rows.filter(row => row.url !== OFFICIAL_CATALOG_URL).sort(byAttestedThenName);
  const ordered = [...(official ? [official] : []), ...community];

  const catalogs = ordered.flatMap(row => {
    if (row.body === null) return [];
    const isOfficial = row === official;
    const head = JSON.stringify({
      url: row.url,
      official: isOfficial,
      attested: isOfficial || row.status === 'attested',
    });
    return [`${head.slice(0, -1)},"catalog":${row.body}}`];
  });

  const failures = [
    ...(official ? [] : [{ url: OFFICIAL_CATALOG_URL, reason: UNFETCHED }]),
    ...ordered.filter(row => row.body === null).map(row => ({ url: row.url, reason: row.check_error ?? UNFETCHED })),
  ];
  const tail = JSON.stringify({
    failures,
    installs: toInstallCounts(installs),
    trending: toTrending(trending),
    featured: toPin(pin),
    category_backfill: categoryBackfill,
  }).slice(1);

  return `{"updated_at":${JSON.stringify(now)},"catalogs":[${catalogs.join(',')}],${tail}`;
}

function mergeStatements(db: D1Database, now: string): D1PreparedStatement[] {
  return [mergeStatement(db), installCountsStatement(db), trendingStatement(db, now), featuredPinStatement(db)];
}

function mergeInputs(results: D1Result[]): MergeInputs {
  const [catalogs, installs, trending, pin] = results;
  return {
    rows: catalogs!.results as MergeRow[],
    installs: installs!.results,
    trending: trending!.results,
    pin: pin!.results,
  };
}

export async function mergedApps(args: { db: D1Database; now: string }): Promise<string> {
  const { db, now } = args;
  return merged(mergeInputs(await db.batch(mergeStatements(db, now))), now);
}

export async function storeData(args: { db: D1Database; now: string }): Promise<string> {
  const { db, now } = args;
  const results = await db.batch([...mergeStatements(db, now), listSourcesStatement(db)]);
  const sources = results[4];
  const apps = merged(mergeInputs(results), now);
  return `{"apps":${apps},"directory":${JSON.stringify(toDirectoryView(toRecords(sources!.results)))}}`;
}
