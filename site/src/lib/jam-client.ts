import type { Catalog } from '@bridgething/catalog';
import { unwrap } from './directory-client';
import type { JamCategory, JamEntryStatus, JamEntryView, JamListing, JamReviewEntry, JamTallyCategory } from './jam';
import type { JudgeRecord, Principal } from './principal';

export type JamSubmission = {
  source_url: string;
  app_id: string;
  category: JamCategory;
  video_url: string;
  discord: string;
  wishlist: string;
  claim: string | null;
};

export type JamSubmitted = { entry: JamEntryView; claim: string | null };

function authed(token: string, init: RequestInit = {}): RequestInit {
  return {
    ...init,
    cache: 'no-store',
    headers: { 'content-type': 'application/json', authorization: `Bearer ${token}`, ...init.headers },
  };
}

export async function fetchPrincipal(token: string): Promise<Principal> {
  const response = await fetch('/api/admin/me', authed(token));
  return (await unwrap<{ principal: Principal }>(response)).principal;
}

export async function fetchJudges(token: string): Promise<JudgeRecord[]> {
  const response = await fetch('/api/admin/judges', authed(token));
  return (await unwrap<{ judges: JudgeRecord[] }>(response)).judges ?? [];
}

export async function createJudge(args: {
  token: string;
  handle: string;
}): Promise<{ judge: JudgeRecord; token: string }> {
  const response = await fetch(
    '/api/admin/judges',
    authed(args.token, { method: 'POST', body: JSON.stringify({ handle: args.handle }) }),
  );
  return unwrap<{ judge: JudgeRecord; token: string }>(response);
}

export async function revokeJudge(args: { token: string; handle: string }): Promise<string> {
  const response = await fetch(
    '/api/admin/judges',
    authed(args.token, { method: 'DELETE', body: JSON.stringify({ handle: args.handle }) }),
  );
  return (await unwrap<{ revoked: string }>(response)).revoked;
}

export async function fetchJamGallery(init?: { signal?: AbortSignal }): Promise<JamListing[]> {
  const response = await fetch('/api/jam/entries.json', { cache: 'no-store', signal: init?.signal });
  return (await unwrap<{ entries: JamListing[] }>(response)).entries ?? [];
}

export async function fetchJamCatalog(url: string, init?: { signal?: AbortSignal }): Promise<Catalog> {
  const response = await fetch(`/api/jam/catalog?url=${encodeURIComponent(url)}`, {
    cache: 'no-store',
    signal: init?.signal,
  });
  return (await unwrap<{ catalog: Catalog }>(response)).catalog;
}

export async function submitJamEntry(submission: JamSubmission): Promise<JamSubmitted> {
  const response = await fetch('/api/jam/entries', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(submission),
  });

  const body = await unwrap<{ entry: JamEntryView; claim?: string }>(response);
  return { entry: body.entry, claim: body.claim ?? null };
}

export async function fetchJamReview(token: string): Promise<JamReviewEntry[]> {
  const response = await fetch('/api/jam/review', authed(token));
  return (await unwrap<{ entries: JamReviewEntry[] }>(response)).entries ?? [];
}

export async function putJamScore(args: {
  token: string;
  appId: string;
  category: JamCategory;
  score: number;
  note?: string | null;
}): Promise<void> {
  const response = await fetch(
    '/api/jam/scores',
    authed(args.token, {
      method: 'PUT',
      body: JSON.stringify({ app_id: args.appId, category: args.category, score: args.score, note: args.note ?? null }),
    }),
  );
  await unwrap<unknown>(response);
}

export async function fetchJamTally(token: string): Promise<JamTallyCategory[]> {
  const response = await fetch('/api/jam/tally', authed(token));
  return (await unwrap<{ tally: JamTallyCategory[] }>(response)).tally ?? [];
}

export async function patchJamEntry(args: {
  token: string;
  appId: string;
  status?: JamEntryStatus;
  promote?: boolean;
}): Promise<JamEntryView> {
  const body: Record<string, unknown> = { app_id: args.appId };
  if (args.status !== undefined) body['status'] = args.status;
  if (args.promote !== undefined) body['promote'] = args.promote;

  const response = await fetch('/api/jam/entries', authed(args.token, { method: 'PATCH', body: JSON.stringify(body) }));
  return (await unwrap<{ entry: JamEntryView }>(response)).entry;
}
