import { useEffect, useState } from 'preact/hooks';
import { fetchJamTally } from '../../lib/jam-client';
import { jamCategoryLabel, type JamTallyCategory } from '../../lib/jam';
import { Empty, Group, Loading, Notice, reason } from './shared';

export function TallyTab({ token }: { token: string }) {
  const [tally, setTally] = useState<JamTallyCategory[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    fetchJamTally(token)
      .then(rows => live && setTally(rows))
      .catch(err => live && setError(reason(err)));
    return () => {
      live = false;
    };
  }, [token]);

  if (error !== null) return <Notice kind="err">{error}</Notice>;
  if (tally === null) return <Loading what="the tally" />;

  return (
    <>
      {tally.map(category => (
        <Group key={category.category} title={jamCategoryLabel(category.category)} count={category.entries.length}>
          {category.entries.length === 0 ? (
            <Empty what="nothing entered or nominated here." />
          ) : (
            <ol class="m-0 grid list-none grid-cols-1 gap-3 p-0">
              {category.entries.map((entry, index) => (
                <li key={entry.app_id} class="flex flex-col gap-2 border border-white/15 p-4">
                  <div class="flex flex-wrap items-baseline justify-between gap-3">
                    <span class="flex items-baseline gap-3">
                      <span class="font-mono text-sm text-white/35 tabular-nums">
                        {String(index + 1).padStart(2, '0')}
                      </span>
                      <span class="font-medium">{entry.name ?? entry.app_id}</span>
                      {entry.primary ? null : <span class="font-mono text-xs text-white/35">nominated</span>}
                    </span>
                    <span class="font-display text-xl font-medium tabular-nums">
                      {entry.mean === null ? 'n/a' : entry.mean.toFixed(2)}
                      <span class="ml-2 font-mono text-xs text-white/40">
                        {entry.count} {entry.count === 1 ? 'score' : 'scores'}
                      </span>
                    </span>
                  </div>

                  {entry.scores.length === 0 ? null : (
                    <ul class="m-0 grid list-none grid-cols-1 gap-1 p-0 font-mono text-xs text-white/50">
                      {entry.scores.map(score => (
                        <li key={score.handle} class="flex gap-3">
                          <span class="w-32 shrink-0 truncate">{score.handle}</span>
                          <span class="text-accent tabular-nums">{score.score}</span>
                          {score.note ? <span class="min-w-0 text-pretty">{score.note}</span> : null}
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              ))}
            </ol>
          )}
        </Group>
      ))}
    </>
  );
}
