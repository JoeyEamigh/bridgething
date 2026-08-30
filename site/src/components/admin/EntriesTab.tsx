import { useCallback, useEffect, useState } from 'preact/hooks';
import { webHref } from '../../lib/href';
import { fetchJamReview, patchJamEntry } from '../../lib/jam-client';
import { JAM_ENTRY_STATUSES, jamCategoryLabel, type JamEntryStatus, type JamReviewEntry } from '../../lib/jam';
import { Empty, Group, INPUT, Loading, Notice, reason } from './shared';

function Row({
  entry,
  busy,
  onStatus,
  onPromote,
}: {
  entry: JamReviewEntry;
  busy: boolean;
  onStatus: (appId: string, status: JamEntryStatus) => void;
  onPromote: (appId: string) => void;
}) {
  const video = webHref(entry.video_url);
  const repo = webHref(entry.repo);

  return (
    <li class="flex flex-col gap-3 border border-white/15 p-4">
      <div class="flex items-start gap-3">
        {entry.icon ? (
          <img src={entry.icon} alt="" width="40" height="40" class="size-10 shrink-0 border border-white/10" />
        ) : (
          <span class="size-10 shrink-0 border border-dashed border-white/20" />
        )}
        <div class="min-w-0 flex-1">
          <p class="m-0 font-medium">{entry.name ?? entry.app_id}</p>
          <p class="m-0 font-mono text-xs text-white/40">
            {jamCategoryLabel(entry.category)} · {entry.installs} install{entry.installs === 1 ? '' : 's'} · source{' '}
            {entry.source_status ?? 'unknown'}
          </p>
        </div>
        <span class="text-accent font-mono text-sm">{entry.status}</span>
      </div>

      <p class="m-0 font-mono text-xs break-all text-white/40">{entry.source_url}</p>
      <p class="m-0 font-mono text-xs text-white/50">discord: {entry.discord}</p>
      {entry.wishlist ? <p class="m-0 text-sm text-pretty text-white/60">wish: {entry.wishlist}</p> : null}

      <p class="m-0 flex flex-wrap gap-4 font-mono text-sm">
        {video ? (
          <a href={video} rel="noreferrer noopener">
            video
          </a>
        ) : null}
        {repo ? (
          <a href={repo} rel="noreferrer noopener">
            repo
          </a>
        ) : null}
        <a href={`/apps/${entry.app_id}`}>listing</a>
      </p>

      <div class="flex flex-wrap items-center gap-2">
        <label class="font-mono text-xs text-white/40" for={`status-${entry.app_id}`}>
          status
        </label>
        <select
          id={`status-${entry.app_id}`}
          value={entry.status}
          disabled={busy}
          onChange={event => onStatus(entry.app_id, (event.target as HTMLSelectElement).value as JamEntryStatus)}
          class={`${INPUT} max-w-40 flex-none`}>
          {JAM_ENTRY_STATUSES.map(status => (
            <option key={status} value={status} class="bg-bg">
              {status}
            </option>
          ))}
        </select>
        <button
          type="button"
          class="btn btn-sm"
          disabled={busy || entry.source_status === 'listed' || entry.source_status === 'attested'}
          onClick={() => onPromote(entry.app_id)}>
          list the source
        </button>
      </div>
    </li>
  );
}

export function EntriesTab({ token }: { token: string }) {
  const [entries, setEntries] = useState<JamReviewEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setEntries(await fetchJamReview(token));
    } catch (err) {
      setError(reason(err));
    }
  }, [token]);

  useEffect(() => {
    void load();
  }, [load]);

  const run = useCallback(
    async (work: Promise<unknown>) => {
      setBusy(true);
      setError(null);
      try {
        await work;
        await load();
      } catch (err) {
        setError(reason(err));
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  if (entries === null && error === null) return <Loading what="entries" />;

  return (
    <>
      {error !== null ? (
        <div class="mb-6">
          <Notice kind="err">{error}</Notice>
        </div>
      ) : null}

      {(entries ?? []).length === 0 ? (
        <Empty what="nobody has entered yet." />
      ) : (
        <Group title="entries" count={(entries ?? []).length}>
          <ul class="grid list-none grid-cols-1 gap-4 p-0 md:grid-cols-2">
            {(entries ?? []).map(entry => (
              <Row
                key={entry.app_id}
                entry={entry}
                busy={busy}
                onStatus={(appId, status) => void run(patchJamEntry({ token, appId, status }))}
                onPromote={appId => void run(patchJamEntry({ token, appId, promote: true }))}
              />
            ))}
          </ul>
        </Group>
      )}
    </>
  );
}
