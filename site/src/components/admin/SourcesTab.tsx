import { useCallback, useEffect, useMemo, useState } from 'preact/hooks';
import { setSourceStatus, type AdminEntry, type SourceStatus } from '../../lib/directory-client';
import { Empty, Group, INPUT, Notice, reason } from './shared';

const STATUSES: SourceStatus[] = ['listed', 'attested', 'quarantined', 'rejected'];

const NEW = 'new';

function groupOf(entry: AdminEntry): SourceStatus | typeof NEW {
  return entry.reviewed_at === null && entry.status !== 'rejected' ? NEW : entry.status;
}

function newestFirst(a: AdminEntry, b: AdminEntry): number {
  return b.submitted_at.localeCompare(a.submitted_at) || a.url.localeCompare(b.url);
}

const PROMOTIONS: { status: SourceStatus; label: string }[] = [
  { status: 'attested', label: 'attest' },
  { status: 'listed', label: 'list' },
  { status: 'quarantined', label: 'quarantine' },
  { status: 'rejected', label: 'reject' },
];

function Row({
  entry,
  busy,
  onMove,
}: {
  entry: AdminEntry;
  busy: boolean;
  onMove: (url: string, status: SourceStatus, note?: string | null) => Promise<void>;
}) {
  const [note, setNote] = useState(entry.note ?? '');
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    setNote(entry.note ?? '');
  }, [entry.note]);

  return (
    <li class="flex flex-col gap-3 border border-white/15 p-4">
      <div class="flex flex-wrap items-baseline justify-between gap-2">
        <span class="font-medium">{entry.name}</span>
        <span class="text-accent font-mono text-sm">{entry.status}</span>
      </div>

      <p class="m-0 font-mono text-xs break-all text-white/40">{entry.url}</p>
      {entry.description ? <p class="m-0 text-sm text-white/60">{entry.description}</p> : null}

      <p class="m-0 font-mono text-xs text-white/35">
        {entry.app_count} app{entry.app_count === 1 ? '' : 's'} · submitted {entry.submitted_at.slice(0, 10)} · checked{' '}
        {entry.last_checked_at.slice(0, 10)}
        {entry.last_check_ok ? '' : ' · unreachable'}
        {entry.downloads_cors_ok === false ? ' · downloads not browser-readable' : ''}
        {entry.reviewed_by ? ` · reviewed by ${entry.reviewed_by}` : ''}
      </p>

      {entry.last_check_error ? <p class="text-warn m-0 text-xs">{entry.last_check_error}</p> : null}

      {editing ? (
        <div class="flex flex-wrap gap-2">
          <input
            type="text"
            value={note}
            disabled={busy}
            aria-label={`note for ${entry.name}`}
            onInput={event => setNote((event.target as HTMLInputElement).value)}
            placeholder="moderation note"
            class={INPUT}
          />
          <button
            type="button"
            class="btn btn-sm"
            disabled={busy}
            onClick={() => void onMove(entry.url, entry.status, note.trim() || null).then(() => setEditing(false))}>
            save note
          </button>
          <button type="button" class="btn btn-sm btn-ghost" disabled={busy} onClick={() => setEditing(false)}>
            cancel
          </button>
        </div>
      ) : (
        <button
          type="button"
          class="self-start text-left font-mono text-xs text-white/50 hover:text-white"
          onClick={() => setEditing(true)}>
          note: {entry.note ?? 'none'}
        </button>
      )}

      <div class="flex flex-wrap gap-2">
        {groupOf(entry) === NEW ? (
          <button
            type="button"
            class="btn btn-sm btn-primary"
            disabled={busy}
            onClick={() => void onMove(entry.url, entry.status)}>
            looks fine
          </button>
        ) : null}
        {PROMOTIONS.filter(promotion => promotion.status !== entry.status).map(promotion => (
          <button
            key={promotion.status}
            type="button"
            class="btn btn-sm"
            disabled={busy}
            onClick={() => void onMove(entry.url, promotion.status)}>
            {promotion.label}
          </button>
        ))}
      </div>
    </li>
  );
}

export function SourcesTab({ token, initial }: { token: string; initial: AdminEntry[] }) {
  const [entries, setEntries] = useState<AdminEntry[]>(initial);
  const [filter, setFilter] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const onMove = useCallback(
    async (url: string, status: SourceStatus, note?: string | null) => {
      setBusy(true);
      setError(null);
      try {
        const updated = await setSourceStatus({ token, url, status, note });
        setEntries(current => current.map(entry => (entry.url === url ? updated : entry)));
      } catch (err) {
        setError(reason(err));
      } finally {
        setBusy(false);
      }
    },
    [token],
  );

  const matching = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    const shown = needle
      ? entries.filter(entry => `${entry.name} ${entry.url} ${entry.description ?? ''}`.toLowerCase().includes(needle))
      : entries;
    return [...shown].sort(newestFirst);
  }, [entries, filter]);

  return (
    <>
      <div class="mb-8 flex flex-wrap gap-3">
        <input
          type="search"
          value={filter}
          aria-label="filter sources"
          onInput={event => setFilter((event.target as HTMLInputElement).value)}
          placeholder="filter by name or url"
          class={INPUT}
        />
      </div>

      {error !== null ? (
        <div class="mb-6">
          <Notice kind="err">{error}</Notice>
        </div>
      ) : null}

      {matching.length === 0 ? (
        <Empty what={filter ? 'nothing matches that filter.' : 'nothing submitted yet.'} />
      ) : (
        [NEW, ...STATUSES].map(group => {
          const rows = matching.filter(entry => groupOf(entry) === group);
          if (rows.length === 0) return null;
          return (
            <Group key={group} title={group} count={rows.length}>
              <ul class="grid list-none grid-cols-1 gap-4 p-0 md:grid-cols-2">
                {rows.map(entry => (
                  <Row key={entry.url} entry={entry} busy={busy} onMove={onMove} />
                ))}
              </ul>
            </Group>
          );
        })
      )}
    </>
  );
}
