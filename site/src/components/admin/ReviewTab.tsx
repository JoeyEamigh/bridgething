import { useCallback, useEffect, useState } from 'preact/hooks';
import { webHref } from '../../lib/href';
import { fetchJamReview, putJamScore } from '../../lib/jam-client';
import { JAM_CATEGORIES, jamCategoryLabel, type JamCategory, type JamReviewEntry } from '../../lib/jam';
import { Empty, Group, INPUT, Loading, Notice, reason } from './shared';

const SCORES = [1, 2, 3, 4, 5];

function ScoreRow({
  entry,
  category,
  token,
  onError,
}: {
  entry: JamReviewEntry;
  category: JamCategory;
  token: string;
  onError: (message: string | null) => void;
}) {
  const held = entry.scores.find(score => score.category === category);
  const [score, setScore] = useState<number | null>(held?.score ?? null);
  const [note, setNote] = useState(held?.note ?? '');
  const [saved, setSaved] = useState(held?.note ?? '');
  const [busy, setBusy] = useState(false);

  const save = useCallback(
    async (next: number, text: string) => {
      const cut = text.trim();
      setBusy(true);
      onError(null);
      try {
        await putJamScore({ token, appId: entry.app_id, category, score: next, note: cut || null });
        setScore(next);
        setSaved(cut);
      } catch (err) {
        onError(reason(err));
      } finally {
        setBusy(false);
      }
    },
    [token, entry.app_id, category, onError],
  );

  return (
    <div class="flex flex-wrap items-center gap-3 border-t border-white/10 py-2">
      <span class="w-32 shrink-0 font-mono text-sm text-white/45">{jamCategoryLabel(category)}</span>

      <div class="flex gap-1" role="group" aria-label={`score ${entry.name ?? entry.app_id} for ${category}`}>
        {SCORES.map(value => (
          <button
            key={value}
            type="button"
            aria-pressed={score === value}
            disabled={busy}
            onClick={() => void save(value, note)}
            class={`size-8 border font-mono text-sm tabular-nums ${
              score === value ? 'border-accent text-accent bg-accent-soft' : 'border-white/20 hover:border-white/50'
            }`}>
            {value}
          </button>
        ))}
      </div>

      <input
        type="text"
        value={note}
        disabled={busy}
        aria-label={`note for ${category}`}
        onInput={event => setNote((event.target as HTMLInputElement).value)}
        onBlur={() => {
          if (score !== null && note.trim() !== saved) void save(score, note);
        }}
        placeholder="note"
        class={`${INPUT} basis-48`}
      />
    </div>
  );
}

export function ReviewCard({
  entry,
  token,
  onError,
}: {
  entry: JamReviewEntry;
  token: string;
  onError: (m: string | null) => void;
}) {
  const video = webHref(entry.video_url);
  const repo = webHref(entry.repo);
  const shot = webHref(entry.screenshot);

  return (
    <li class="flex flex-col gap-3 border border-white/15 p-4">
      {shot ? (
        <img
          src={shot}
          alt={`${entry.name ?? 'the app'} running on a car thing`}
          width="800"
          height="480"
          loading="lazy"
          class="-m-4 mb-1 aspect-[5/3] w-[calc(100%+2rem)] max-w-none border-b border-white/15 object-cover"
        />
      ) : null}
      <div class="flex items-start gap-3">
        {entry.icon ? (
          <img src={entry.icon} alt="" width="48" height="48" class="size-12 shrink-0 border border-white/10" />
        ) : (
          <span class="size-12 shrink-0 border border-dashed border-white/20" />
        )}
        <div class="min-w-0">
          <p class="m-0 font-medium">{entry.name ?? entry.app_id}</p>
          <p class="m-0 font-mono text-xs text-white/40">
            {entry.author ? `by ${entry.author} · ` : ''}
            {entry.installs} install{entry.installs === 1 ? '' : 's'} · source {entry.source_status ?? 'unknown'}
            {entry.status === 'disqualified' ? ' · disqualified' : ''}
          </p>
        </div>
      </div>

      {entry.description ? <p class="m-0 text-sm text-pretty text-white/60">{entry.description}</p> : null}

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

      <div>
        {JAM_CATEGORIES.map(category => (
          <ScoreRow key={category.id} entry={entry} category={category.id} token={token} onError={onError} />
        ))}
      </div>
    </li>
  );
}

export function ReviewTab({ token }: { token: string }) {
  const [entries, setEntries] = useState<JamReviewEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    fetchJamReview(token)
      .then(rows => live && setEntries(rows))
      .catch(err => live && setError(reason(err)));
    return () => {
      live = false;
    };
  }, [token]);

  if (entries === null && error === null) return <Loading what="entries" />;

  return (
    <>
      {error !== null ? (
        <div class="mb-6">
          <Notice kind="err">{error}</Notice>
        </div>
      ) : null}

      {(entries ?? []).length === 0 ? (
        <Empty what="nothing to review yet." />
      ) : (
        JAM_CATEGORIES.map(category => {
          const rows = (entries ?? []).filter(entry => entry.category === category.id);
          if (rows.length === 0) return null;
          return (
            <Group key={category.id} title={category.label} count={rows.length}>
              <ul class="grid list-none grid-cols-1 gap-4 p-0 xl:grid-cols-2">
                {rows.map(entry => (
                  <ReviewCard key={entry.app_id} entry={entry} token={token} onError={setError} />
                ))}
              </ul>
            </Group>
          );
        })
      )}
    </>
  );
}
