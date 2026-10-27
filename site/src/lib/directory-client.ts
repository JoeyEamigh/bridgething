import {
  unwrapDirectory,
  type DirectoryEntry,
  type MergedApps,
  type SourceStatus,
  type StorePin,
} from '@bridgething/catalog';

export type AdminEntry = DirectoryEntry & { note: string | null; reviewed_by: string | null };

export type StoreData = { apps: MergedApps; directory: DirectoryEntry[] };

export function readStoreData(): StoreData | null {
  if (typeof document === 'undefined') return null;
  const tag = document.getElementById('store-data');
  if (!tag?.textContent) return null;
  try {
    const parsed = JSON.parse(tag.textContent) as StoreData | null;
    return parsed && Array.isArray(parsed.apps?.catalogs) ? parsed : null;
  } catch {
    return null;
  }
}

export async function fetchDirectory(init?: { signal?: AbortSignal }): Promise<DirectoryEntry[]> {
  const response = await fetch('/api/directory.json', { cache: 'no-store', signal: init?.signal });
  const body = await unwrapDirectory<{ sources: DirectoryEntry[] }>(response);
  return body.sources ?? [];
}

export {
  DirectoryApiError,
  fetchMergedApps,
  reportInstalled,
  submitSource,
  type DirectoryEntry,
  type InstallCount,
  type MergedApps,
  type MergedCatalog,
  type SourceStatus,
} from '@bridgething/catalog';

export async function fetchAdminSources(token: string): Promise<AdminEntry[]> {
  const response = await fetch('/api/admin/sources', { cache: 'no-store', headers: adminHeaders(token) });
  const body = await unwrapDirectory<{ sources: AdminEntry[] }>(response);
  return body.sources ?? [];
}

export async function setSourceStatus(args: {
  token: string;
  url: string;
  status: SourceStatus;
  note?: string | null;
}): Promise<AdminEntry> {
  const response = await fetch('/api/admin/sources', {
    method: 'PATCH',
    headers: adminHeaders(args.token),
    body: JSON.stringify({ url: args.url, status: args.status, note: args.note }),
  });
  const body = await unwrapDirectory<{ source: AdminEntry }>(response);
  return body.source;
}

function adminHeaders(token: string): Record<string, string> {
  return { 'content-type': 'application/json', authorization: `Bearer ${token}` };
}

export async function fetchFeaturedPin(token: string): Promise<StorePin | null> {
  const response = await fetch('/api/admin/featured', { cache: 'no-store', headers: adminHeaders(token) });
  return (await unwrapDirectory<{ pin: StorePin | null }>(response)).pin;
}

export async function pinFeatured(token: string, pin: StorePin): Promise<StorePin | null> {
  const response = await fetch('/api/admin/featured', {
    method: 'PUT',
    headers: adminHeaders(token),
    body: JSON.stringify(pin),
  });
  return (await unwrapDirectory<{ pin: StorePin | null }>(response)).pin;
}

export async function unpinFeatured(token: string): Promise<void> {
  const response = await fetch('/api/admin/featured', { method: 'DELETE', headers: adminHeaders(token) });
  await unwrapDirectory<{ pin: null }>(response);
}
