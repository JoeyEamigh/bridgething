import {
  normalizeSourceUrl as normalizeCatalogUrl,
  SourceUrlError,
  type Catalog,
  type RecommendedSource,
} from '@bridgething/catalog';

export { SourceUrlError };

export type SourceStatus = 'quarantined' | 'listed' | 'attested' | 'rejected';

export const SOURCE_STATUSES: SourceStatus[] = ['quarantined', 'listed', 'attested', 'rejected'];

export type SourceRecord = {
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
  last_checked_at: string;
  last_check_ok: boolean;
  last_check_error: string | null;
  downloads_cors_ok: boolean | null;
  note: string | null;
};

export type DirectoryEntry = Omit<SourceRecord, 'note' | 'reviewed_by'>;

export const KEY_PREFIX = 'source:';

export const NAME_MAX_LEN = 120;
export const DESCRIPTION_MAX_LEN = 400;

export function keyFor(url: string): string {
  return `${KEY_PREFIX}${url}`;
}

export function normalizeSourceUrl(raw: string): string {
  const url = normalizeCatalogUrl(raw);
  if (new URL(url).protocol !== 'https:') {
    throw new SourceUrlError('a source url must be https; a browser will not read an http catalog from this page');
  }
  return url;
}

export function clamp(value: string, max: number): string {
  const trimmed = value.trim();
  return trimmed.length <= max ? trimmed : `${trimmed.slice(0, max - 1)}…`;
}

export function isPublished(record: SourceRecord): boolean {
  return record.status === 'listed' || record.status === 'attested';
}

export function isVisible(record: SourceRecord): boolean {
  return record.status !== 'rejected';
}

export function byAttestedThenName(a: SourceRecord, b: SourceRecord): number {
  if (a.status !== b.status) {
    if (a.status === 'attested') return -1;
    if (b.status === 'attested') return 1;
  }
  return a.name.localeCompare(b.name) || a.url.localeCompare(b.url);
}

export function toCatalogDocument(records: SourceRecord[], updatedAt: string): Catalog {
  const recommended: RecommendedSource[] = records
    .filter(isPublished)
    .sort(byAttestedThenName)
    .map(record => ({
      name: record.name,
      url: record.url,
      description: record.description,
      attested: record.status === 'attested',
    }));

  return {
    schema: 'catalog.v1',
    updated_at: updatedAt,
    repo: {
      name: 'bridgething source directory',
      description: 'Catalog sources submitted by the community. Listing is not an endorsement.',
      homepage: 'https://bridgething.com/apps',
      icon: null,
    },
    apps: [],
    recommended_sources: recommended,
  };
}

export function toDirectoryView(records: SourceRecord[]): DirectoryEntry[] {
  return records
    .filter(isVisible)
    .sort(byAttestedThenName)
    .map(({ note: _note, reviewed_by: _reviewedBy, ...rest }) => rest);
}
