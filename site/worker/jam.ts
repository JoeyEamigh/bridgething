import type { AppEntry, Catalog } from '@bridgething/catalog';
import {
  JAM_CATEGORY_IDS,
  JAM_TIMELINE,
  JAM_ENTRY_STATUSES,
  type JamCategory,
  type JamEntry,
  type JamEntryStatus,
  type JamEntryView,
  type JamListing,
  type JamReviewEntry,
  type JamScoreView,
  type JamTallyCategory,
  type JamTallyEntry,
  type JamTimeline,
  jamClosedReason,
  jamWindow,
} from '../src/lib/jam.ts';
import { isPublished, normalizeSourceUrl, SourceUrlError } from './directory.ts';
import { listInstalls } from './installs.ts';
import { hashToken, mintToken, tokenMatches } from './principal.ts';
import { fetchCatalogResponse, parseCatalogBody, PROBE_TIMEOUT_MS } from './probe.ts';
import { setSourceStatus, submitSource } from './sources.ts';
import {
  listSources,
  mergeIntoSnapshot,
  readSnapshot,
  readRecord,
  readSource,
  rebuildSnapshot,
  walkRecords,
  type KvLike,
} from './store.ts';

export type {
  JamCategory,
  JamEntry,
  JamEntryStatus,
  JamEntryView,
  JamListing,
  JamReviewEntry,
  JamScoreView,
  JamTallyCategory,
};

export const ENTRY_PREFIX = 'jam:entry:';
export const SCORE_PREFIX = 'jam:score:';
export const ENTRY_SNAPSHOT_KEY = 'jam:snapshot:entries';

export const VIDEO_MAX_LEN = 400;
export const DISCORD_MAX_LEN = 64;
export const WISHLIST_MAX_LEN = 1200;
export const NOTE_MAX_LEN = 600;
export const SCORE_MIN = 1;
export const SCORE_MAX = 5;

export type JamScore = {
  app_id: string;
  handle: string;
  category: JamCategory;
  score: number;
  note: string | null;
  updated_at: string;
};

export function isJamCategory(value: unknown): value is JamCategory {
  return typeof value === 'string' && (JAM_CATEGORY_IDS as readonly string[]).includes(value);
}

export function isJamEntryStatus(value: unknown): value is JamEntryStatus {
  return typeof value === 'string' && (JAM_ENTRY_STATUSES as readonly string[]).includes(value);
}

export function entryKeyFor(appId: string): string {
  return `${ENTRY_PREFIX}${appId}`;
}

export function scoreKeyFor(appId: string, handle: string, category: JamCategory): string {
  return `${SCORE_PREFIX}${appId}:${handle}:${category}`;
}

export async function readEntry(kv: KvLike, appId: string): Promise<JamEntry | null> {
  return readRecord<JamEntry>(kv, entryKeyFor(appId));
}

export async function writeEntry(kv: KvLike, entry: JamEntry): Promise<void> {
  await kv.put(entryKeyFor(entry.app_id), JSON.stringify(entry));
  await mergeIntoSnapshot({
    kv,
    key: ENTRY_SNAPSHOT_KEY,
    prefix: ENTRY_PREFIX,
    record: entry,
    identity: held => held.app_id,
  });
}

export function entryView(entry: JamEntry): JamEntryView {
  const { claim_hash: _hash, ...view } = entry;
  return view;
}

export async function rebuildEntries(kv: KvLike): Promise<JamEntry[]> {
  return rebuildSnapshot<JamEntry>({
    kv,
    key: ENTRY_SNAPSHOT_KEY,
    prefix: ENTRY_PREFIX,
    keyOf: entry => entryKeyFor(entry.app_id),
  });
}

export async function listEntries(kv: KvLike): Promise<JamEntry[]> {
  return (await readSnapshot<JamEntry>(kv, ENTRY_SNAPSHOT_KEY))?.items ?? (await rebuildEntries(kv));
}

export async function listScores(kv: KvLike): Promise<JamScore[]> {
  return walkRecords<JamScore>(kv, SCORE_PREFIX);
}

function trimmed(value: unknown, max: number): string | null {
  if (typeof value !== 'string') return null;
  const cut = value.trim();
  return cut.length === 0 || cut.length > max ? null : cut;
}

function isHttps(value: string): boolean {
  try {
    return new URL(value).protocol === 'https:';
  } catch {
    return false;
  }
}

function httpsUrl(value: unknown, max: number): string | null {
  const raw = trimmed(value, max);
  return raw !== null && isHttps(raw) ? raw : null;
}

export function appIn(catalog: Catalog, appId: string): AppEntry | null {
  return catalog.apps.find(app => app.id.toLowerCase() === appId) ?? null;
}

async function fetchCatalogAt(url: string, fetchImpl: typeof fetch): Promise<Catalog | null> {
  const fetched = await fetchCatalogResponse(url, fetchImpl);
  if (!fetched.ok) return null;
  const parsed = await parseCatalogBody(fetched.response, url);
  return parsed.ok ? parsed.catalog : null;
}

async function catalogsFor(urls: string[], fetchImpl: typeof fetch): Promise<Map<string, Catalog | null>> {
  const unique = [...new Set(urls)];
  const found = await Promise.all(unique.map(url => fetchCatalogAt(url, fetchImpl)));
  return new Map(unique.map((url, index) => [url, found[index] ?? null]));
}

async function headOk(url: string, fetchImpl: typeof fetch): Promise<boolean> {
  try {
    const head = await fetchImpl(url, {
      method: 'HEAD',
      redirect: 'follow',
      signal: AbortSignal.timeout(PROBE_TIMEOUT_MS),
    });
    return head.ok;
  } catch {
    return false;
  }
}

async function getOk(url: string, fetchImpl: typeof fetch): Promise<boolean> {
  try {
    const got = await fetchImpl(url, {
      method: 'GET',
      redirect: 'follow',
      signal: AbortSignal.timeout(PROBE_TIMEOUT_MS),
    });
    await got.body?.cancel();
    return got.ok;
  } catch {
    return false;
  }
}

export async function iconReachable(url: string, fetchImpl: typeof fetch): Promise<boolean> {
  return (await headOk(url, fetchImpl)) || getOk(url, fetchImpl);
}

export function firstUnreachable(urls: string[], fetchImpl: typeof fetch): Promise<string | null> {
  return urls.reduce<Promise<string | null>>(
    async (found, url) => (await found) ?? ((await iconReachable(url, fetchImpl)) ? null : url),
    Promise.resolve(null),
  );
}

function listingFor(entry: JamEntry, app: AppEntry | null): JamListing {
  return {
    app_id: entry.app_id,
    source_url: entry.source_url,
    category: entry.category,
    video_url: entry.video_url,
    status: entry.status,
    submitted_at: entry.submitted_at,
    name: app?.name ?? null,
    description: app?.description ?? null,
    author: app?.author ?? null,
    icon: app?.icon ?? null,
    screenshot: app?.screenshots?.[0] ?? null,
    repo: app?.source ?? null,
  };
}

function newestFirst(a: JamListing, b: JamListing): number {
  return b.submitted_at.localeCompare(a.submitted_at) || a.app_id.localeCompare(b.app_id);
}

async function listingsFor(entries: JamEntry[], fetchImpl: typeof fetch): Promise<JamListing[]> {
  const catalogs = await catalogsFor(
    entries.map(entry => entry.source_url),
    fetchImpl,
  );
  return entries
    .map(entry => {
      const catalog = catalogs.get(entry.source_url) ?? null;
      return listingFor(entry, catalog === null ? null : appIn(catalog, entry.app_id));
    })
    .sort(newestFirst);
}

export type JamSubmission = {
  sourceUrl: string;
  appId: string;
  category: JamCategory;
  videoUrl: string;
  discord: string;
  wishlist: string;
  claim: string | null;
};

export type JamParseOutcome = { ok: true; submission: JamSubmission } | { ok: false; status: number; reason: string };

export function parseJamSubmission(body: Record<string, unknown> | null): JamParseOutcome {
  const rawSource = body?.['source_url'];
  if (typeof rawSource !== 'string' || !rawSource.trim()) {
    return { ok: false, status: 400, reason: 'send a json body with a "source_url" string' };
  }

  const rawAppId = body?.['app_id'];
  if (typeof rawAppId !== 'string' || !rawAppId.trim()) {
    return { ok: false, status: 400, reason: 'send a json body with an "app_id" string' };
  }

  const category = body?.['category'];
  if (!isJamCategory(category)) {
    return { ok: false, status: 400, reason: `"category" must be one of ${JAM_CATEGORY_IDS.join(', ')}` };
  }

  const videoUrl = httpsUrl(body?.['video_url'], VIDEO_MAX_LEN);
  if (videoUrl === null) {
    return { ok: false, status: 400, reason: '"video_url" must be an https link to a video of the app on the device' };
  }

  const discord = trimmed(body?.['discord'], DISCORD_MAX_LEN);
  if (discord === null) {
    return { ok: false, status: 400, reason: `"discord" must be a handle of at most ${DISCORD_MAX_LEN} characters` };
  }

  const rawWishlist = body?.['wishlist'];
  if (rawWishlist !== undefined && rawWishlist !== null && typeof rawWishlist !== 'string') {
    return { ok: false, status: 400, reason: '"wishlist" must be a string' };
  }

  const rawClaim = body?.['claim'];
  if (rawClaim !== undefined && rawClaim !== null && typeof rawClaim !== 'string') {
    return { ok: false, status: 400, reason: '"claim" must be the token string from your first submission' };
  }

  let sourceUrl: string;
  try {
    sourceUrl = normalizeSourceUrl(rawSource);
  } catch (err) {
    if (err instanceof SourceUrlError) return { ok: false, status: 400, reason: err.message };
    throw err;
  }

  return {
    ok: true,
    submission: {
      sourceUrl,
      appId: rawAppId.trim().toLowerCase(),
      category,
      videoUrl,
      discord,
      wishlist: typeof rawWishlist === 'string' ? rawWishlist.trim().slice(0, WISHLIST_MAX_LEN) : '',
      claim: typeof rawClaim === 'string' && rawClaim.trim() ? rawClaim.trim() : null,
    },
  };
}

export type JamSubmitOutcome =
  | { ok: true; created: true; entry: JamEntry; claim: string }
  | { ok: true; created: false; entry: JamEntry }
  | { ok: false; status: number; reason: string };

async function claimFor(held: JamEntry | null): Promise<{ hash: string; minted: string | null }> {
  if (held !== null) return { hash: held.claim_hash, minted: null };

  const minted = mintToken();
  return { hash: await hashToken(minted), minted };
}

async function holdsClaim(held: JamEntry, claim: string | null): Promise<boolean> {
  return claim === null ? false : tokenMatches(await hashToken(claim), held.claim_hash);
}

export async function submitJamEntry(args: {
  kv: KvLike;
  submission: JamSubmission;
  now: string;
  timeline?: JamTimeline;
  fetchImpl?: typeof fetch;
}): Promise<JamSubmitOutcome> {
  const { kv, submission, now, timeline = JAM_TIMELINE, fetchImpl = fetch } = args;
  const { appId } = submission;

  const window = jamWindow(timeline, new Date(now));
  if (!window.open) return { ok: false, status: 403, reason: jamClosedReason(timeline, window) };

  const held = await readEntry(kv, appId);
  if (held?.status === 'disqualified') {
    return { ok: false, status: 403, reason: 'this entry was disqualified and cannot be resubmitted' };
  }
  if (held !== null && !(await holdsClaim(held, submission.claim))) {
    return {
      ok: false,
      status: 403,
      reason: 'this app is already entered. resubmit with the claim token you were given the first time.',
    };
  }

  const source = await submitSource({ kv, rawUrl: submission.sourceUrl, now, fetchImpl });
  if (!source.ok) return { ok: false, status: source.status, reason: source.reason };

  const app = appIn(source.catalog, appId);
  if (app === null) {
    return { ok: false, status: 422, reason: `${source.record.url} does not list an app with the id ${appId}` };
  }

  if (app.icon === null) {
    return { ok: false, status: 422, reason: `"${app.name}" has no icon. a jam entry needs a square icon` };
  }
  if (!(await iconReachable(app.icon, fetchImpl))) {
    return { ok: false, status: 422, reason: `the icon for "${app.name}" could not be fetched from ${app.icon}` };
  }

  const shots = app.screenshots ?? [];
  if (shots.length === 0) {
    return {
      ok: false,
      status: 422,
      reason: `"${app.name}" has no screenshots. add a "screenshots" array to the catalog entry with at least one 800x480 capture of it running`,
    };
  }
  if (shots.some(shot => !isHttps(shot))) {
    return { ok: false, status: 422, reason: `every screenshot url for "${app.name}" must be https` };
  }
  const missing = await firstUnreachable(shots, fetchImpl);
  if (missing !== null) {
    return { ok: false, status: 422, reason: `the screenshot for "${app.name}" could not be fetched from ${missing}` };
  }

  const repo = app.source?.trim() ?? '';
  if (!repo) {
    return { ok: false, status: 422, reason: `"${app.name}" needs a "source" repo url; jam entries are open source` };
  }
  if (!isHttps(repo)) {
    return { ok: false, status: 422, reason: `the "source" repo url for "${app.name}" must be https` };
  }

  const claim = await claimFor(held);
  const entry: JamEntry = {
    app_id: appId,
    source_url: source.record.url,
    category: submission.category,
    video_url: submission.videoUrl,
    discord: submission.discord,
    wishlist: submission.wishlist,
    status: held?.status ?? 'submitted',
    submitted_at: held?.submitted_at ?? now,
    updated_at: now,
    claim_hash: claim.hash,
  };

  await writeEntry(kv, entry);
  return claim.minted === null
    ? { ok: true, created: false, entry }
    : { ok: true, created: true, entry, claim: claim.minted };
}

export async function jamGallery(args: { kv: KvLike; fetchImpl?: typeof fetch }): Promise<JamListing[]> {
  const { kv, fetchImpl = fetch } = args;
  const entries = (await listEntries(kv)).filter(entry => entry.status !== 'disqualified');
  return listingsFor(entries, fetchImpl);
}

export async function jamReview(args: {
  kv: KvLike;
  handle: string | null;
  fetchImpl?: typeof fetch;
}): Promise<JamReviewEntry[]> {
  const { kv, handle, fetchImpl = fetch } = args;

  const entries = await listEntries(kv);
  const listings = await listingsFor(entries, fetchImpl);
  const byId = new Map(entries.map(entry => [entry.app_id, entry]));

  const installs = await listInstalls(kv);
  const sources = await listSources(kv);
  const scores = handle === null ? [] : (await listScores(kv)).filter(score => score.handle === handle);

  return listings.map(listing => {
    const entry = byId.get(listing.app_id)!;
    const install = installs.find(
      record => record.app_id === listing.app_id && record.source_url === listing.source_url,
    );
    const source = sources.find(record => record.url === listing.source_url);

    return {
      ...listing,
      discord: entry.discord,
      wishlist: entry.wishlist,
      installs: install?.count ?? 0,
      source_status: source?.status ?? null,
      scores: scores
        .filter(score => score.app_id === listing.app_id)
        .map(score => ({ category: score.category, score: score.score, note: score.note }))
        .sort((a, b) => a.category.localeCompare(b.category)),
    };
  });
}

export type JamScoreOutcome = { ok: true; score: JamScore } | { ok: false; status: number; reason: string };

export async function putJamScore(args: {
  kv: KvLike;
  body: Record<string, unknown> | null;
  handle: string;
  now: string;
}): Promise<JamScoreOutcome> {
  const { kv, body, handle, now } = args;

  const rawAppId = body?.['app_id'];
  if (typeof rawAppId !== 'string' || !rawAppId.trim()) {
    return { ok: false, status: 400, reason: 'send a json body with an "app_id" string' };
  }
  const appId = rawAppId.trim().toLowerCase();

  const category = body?.['category'];
  if (!isJamCategory(category)) {
    return { ok: false, status: 400, reason: `"category" must be one of ${JAM_CATEGORY_IDS.join(', ')}` };
  }

  const raw = body?.['score'];
  if (typeof raw !== 'number' || !Number.isInteger(raw) || raw < SCORE_MIN || raw > SCORE_MAX) {
    return { ok: false, status: 400, reason: `"score" must be a whole number from ${SCORE_MIN} to ${SCORE_MAX}` };
  }

  const rawNote = body?.['note'];
  if (rawNote !== undefined && rawNote !== null && typeof rawNote !== 'string') {
    return { ok: false, status: 400, reason: '"note" must be a string or null' };
  }
  const cut = typeof rawNote === 'string' ? rawNote.trim().slice(0, NOTE_MAX_LEN) : '';

  if ((await readEntry(kv, appId)) === null) {
    return { ok: false, status: 404, reason: `${appId} is not a jam entry` };
  }

  const score: JamScore = { app_id: appId, handle, category, score: raw, note: cut || null, updated_at: now };
  await kv.put(scoreKeyFor(appId, handle, category), JSON.stringify(score));

  return { ok: true, score };
}

export async function jamTally(args: { kv: KvLike; fetchImpl?: typeof fetch }): Promise<JamTallyCategory[]> {
  const { kv, fetchImpl = fetch } = args;

  const entries = (await listEntries(kv)).filter(entry => entry.status !== 'disqualified');
  const listings = await listingsFor(entries, fetchImpl);
  const live = new Set(entries.map(entry => entry.app_id));
  const scores = (await listScores(kv)).filter(score => live.has(score.app_id));

  return JAM_CATEGORY_IDS.map(category => {
    const inCategory = scores.filter(score => score.category === category);
    const scored = new Set(inCategory.map(score => score.app_id));

    const rows: JamTallyEntry[] = listings
      .filter(listing => listing.category === category || scored.has(listing.app_id))
      .map(listing => {
        const mine = inCategory
          .filter(score => score.app_id === listing.app_id)
          .sort((a, b) => a.handle.localeCompare(b.handle))
          .map(score => ({ handle: score.handle, score: score.score, note: score.note }));

        const total = mine.reduce((sum, score) => sum + score.score, 0);
        return {
          app_id: listing.app_id,
          name: listing.name,
          primary: listing.category === category,
          mean: mine.length === 0 ? null : Math.round((total / mine.length) * 100) / 100,
          count: mine.length,
          scores: mine,
        };
      })
      .sort((a, b) => {
        if (a.mean === null && b.mean === null) return a.app_id.localeCompare(b.app_id);
        if (a.mean === null) return 1;
        if (b.mean === null) return -1;
        return b.mean - a.mean || b.count - a.count || a.app_id.localeCompare(b.app_id);
      });

    return { category, entries: rows };
  });
}

export type JamPatchOutcome =
  | { ok: true; entry: JamEntry; promoted: boolean }
  | { ok: false; status: number; reason: string };

export async function patchJamEntry(args: {
  kv: KvLike;
  body: Record<string, unknown> | null;
  reviewedBy: string;
  now: string;
}): Promise<JamPatchOutcome> {
  const { kv, body, reviewedBy, now } = args;

  const rawAppId = body?.['app_id'];
  if (typeof rawAppId !== 'string' || !rawAppId.trim()) {
    return { ok: false, status: 400, reason: 'send a json body with an "app_id" string' };
  }
  const appId = rawAppId.trim().toLowerCase();

  const status = body?.['status'];
  if (status !== undefined && !isJamEntryStatus(status)) {
    return { ok: false, status: 400, reason: `"status" must be one of ${JAM_ENTRY_STATUSES.join(', ')}` };
  }

  const promote = body?.['promote'];
  if (promote !== undefined && typeof promote !== 'boolean') {
    return { ok: false, status: 400, reason: '"promote" must be a boolean' };
  }

  if (status === undefined && promote !== true) {
    return { ok: false, status: 400, reason: 'send a "status" to set, or "promote": true to list the entry source' };
  }

  const held = await readEntry(kv, appId);
  if (held === null) return { ok: false, status: 404, reason: `${appId} is not a jam entry` };

  let entry = held;
  if (status !== undefined) {
    entry = { ...held, status, updated_at: now };
    await writeEntry(kv, entry);
  }

  if (promote === true) {
    const source = await readSource(kv, entry.source_url);
    if (source === null || !isPublished(source)) {
      const outcome = await setSourceStatus({ kv, rawUrl: entry.source_url, status: 'listed', reviewedBy, now });
      if (!outcome.ok) return { ok: false, status: outcome.status, reason: outcome.reason };
    }
  }

  return { ok: true, entry, promoted: promote === true };
}
