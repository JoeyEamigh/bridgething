import type { Catalog } from '@bridgething/catalog';
import {
  clamp,
  DESCRIPTION_MAX_LEN,
  NAME_MAX_LEN,
  normalizeSourceUrl,
  SourceUrlError,
  type SourceRecord,
  type SourceStatus,
} from './directory.ts';
import { probeSource } from './probe.ts';
import { readSource, writeSource, type KvLike } from './store.ts';

export type SubmitOutcome =
  | { ok: true; created: boolean; record: SourceRecord; catalog: Catalog }
  | { ok: false; status: number; reason: string };

export async function submitSource(args: {
  kv: KvLike;
  rawUrl: string;
  now: string;
  fetchImpl?: typeof fetch;
}): Promise<SubmitOutcome> {
  const { kv, rawUrl, now, fetchImpl } = args;

  let url: string;
  try {
    url = normalizeSourceUrl(rawUrl);
  } catch (err) {
    if (err instanceof SourceUrlError) return { ok: false, status: 400, reason: err.message };
    throw err;
  }

  const existing = await readSource(kv, url);
  if (existing?.status === 'rejected') {
    return { ok: false, status: 403, reason: `${url} was removed from the directory and cannot be resubmitted` };
  }

  const probe = await probeSource(url, fetchImpl);
  if (!probe.ok) return { ok: false, status: 422, reason: probe.reason };

  const { catalog, downloadsCorsOk } = probe;
  const record: SourceRecord = {
    url,
    name: clamp(catalog.repo.name, NAME_MAX_LEN),
    description: catalog.repo.description ? clamp(catalog.repo.description, DESCRIPTION_MAX_LEN) : null,
    homepage: catalog.repo.homepage,
    icon: catalog.repo.icon,
    status: existing?.status ?? 'quarantined',
    submitted_at: existing?.submitted_at ?? now,
    reviewed_at: existing?.reviewed_at ?? null,
    reviewed_by: existing?.reviewed_by ?? null,
    app_count: catalog.apps.length,
    last_checked_at: now,
    last_check_ok: true,
    last_check_error: null,
    downloads_cors_ok: downloadsCorsOk,
    note: existing?.note ?? null,
  };

  await writeSource(kv, record);
  return { ok: true, created: existing === null, record, catalog };
}

export async function recheckSource(args: {
  kv: KvLike;
  record: SourceRecord;
  now: string;
  fetchImpl?: typeof fetch;
}): Promise<SourceRecord> {
  const { kv, record, now, fetchImpl } = args;
  const probe = await probeSource(record.url, fetchImpl);

  const updated: SourceRecord = probe.ok
    ? {
        ...record,
        name: clamp(probe.catalog.repo.name, NAME_MAX_LEN),
        description: probe.catalog.repo.description ? clamp(probe.catalog.repo.description, DESCRIPTION_MAX_LEN) : null,
        homepage: probe.catalog.repo.homepage,
        icon: probe.catalog.repo.icon,
        app_count: probe.catalog.apps.length,
        last_checked_at: now,
        last_check_ok: true,
        last_check_error: null,
        downloads_cors_ok: probe.downloadsCorsOk,
      }
    : { ...record, last_checked_at: now, last_check_ok: false, last_check_error: probe.reason };

  await writeSource(kv, updated);
  return updated;
}

export type ModerateOutcome = { ok: true; record: SourceRecord } | { ok: false; status: number; reason: string };

export async function setSourceStatus(args: {
  kv: KvLike;
  rawUrl: string;
  status: SourceStatus;
  reviewedBy: string;
  note?: string | null;
  now: string;
}): Promise<ModerateOutcome> {
  const { kv, rawUrl, status, reviewedBy, note, now } = args;

  let url: string;
  try {
    url = normalizeSourceUrl(rawUrl);
  } catch (err) {
    if (err instanceof SourceUrlError) return { ok: false, status: 400, reason: err.message };
    throw err;
  }

  const existing = await readSource(kv, url);
  if (!existing) return { ok: false, status: 404, reason: `${url} is not in the directory` };

  const record: SourceRecord = {
    ...existing,
    status,
    reviewed_at: now,
    reviewed_by: reviewedBy,
    note: note === undefined ? existing.note : note,
  };

  await writeSource(kv, record);
  return { ok: true, record };
}
