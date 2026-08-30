export type SourceStatus = 'quarantined' | 'listed' | 'attested' | 'rejected';

export type DirectoryEntry = {
  url: string;
  name: string;
  description: string | null;
  homepage: string | null;
  icon: string | null;
  status: SourceStatus;
  submitted_at: string;
  reviewed_at: string | null;
  app_count: number;
  last_checked_at: string;
  last_check_ok: boolean;
  last_check_error: string | null;
  downloads_cors_ok: boolean | null;
};

export type AdminEntry = DirectoryEntry & { note: string | null; reviewed_by: string | null };

export class DirectoryApiError extends Error {
  constructor(
    message: string,
    public readonly status: number,
  ) {
    super(message);
    this.name = 'DirectoryApiError';
  }
}

export async function unwrap<T>(response: Response): Promise<T> {
  let body: unknown;
  try {
    body = await response.json();
  } catch {
    throw new DirectoryApiError(`the directory returned ${response.status} with no readable body`, response.status);
  }

  if (!response.ok) {
    const message = (body as { error?: unknown })?.error;
    throw new DirectoryApiError(
      typeof message === 'string' ? message : `the directory returned ${response.status}`,
      response.status,
    );
  }

  return body as T;
}

export async function fetchDirectory(init?: { signal?: AbortSignal }): Promise<DirectoryEntry[]> {
  const response = await fetch('/api/directory.json', { cache: 'no-store', signal: init?.signal });
  const body = await unwrap<{ sources: DirectoryEntry[] }>(response);
  return body.sources ?? [];
}

export {
  fetchMergedApps,
  reportInstall,
  type InstallCount,
  type MergedApps,
  type MergedCatalog,
} from '@bridgething/catalog';

export async function submitSource(url: string): Promise<DirectoryEntry> {
  const response = await fetch('/api/sources', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ url }),
  });
  const body = await unwrap<{ source: DirectoryEntry }>(response);
  return body.source;
}

export async function fetchAdminSources(token: string): Promise<AdminEntry[]> {
  const response = await fetch('/api/admin/sources', {
    cache: 'no-store',
    headers: { authorization: `Bearer ${token}` },
  });
  const body = await unwrap<{ sources: AdminEntry[] }>(response);
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
    headers: { 'content-type': 'application/json', authorization: `Bearer ${args.token}` },
    body: JSON.stringify({ url: args.url, status: args.status, note: args.note }),
  });
  const body = await unwrap<{ source: AdminEntry }>(response);
  return body.source;
}
