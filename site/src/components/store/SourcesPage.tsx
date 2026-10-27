import { useState } from 'preact/hooks';
import { failureLine } from '@bridgething/catalog';
import type { DirectoryEntry, StoreData } from '../../lib/directory-client';
import { useStore } from '../../lib/use-store';
import { Notice } from './parts';
import { SourceDirectory } from './SourceDirectory';
import { SubmitSource } from './SubmitSource';

export function SourcesPage({ data }: { data?: StoreData | null }) {
  const { data: loaded, failed } = useStore(data);
  const [submitted, setSubmitted] = useState<DirectoryEntry[]>([]);

  const directory = loaded
    ? [...loaded.directory.filter(entry => !submitted.some(fresh => fresh.url === entry.url)), ...submitted]
    : null;
  const failures = loaded?.apps.failures ?? [];

  return (
    <>
      <SubmitSource
        onSubmitted={entry =>
          setSubmitted(current => [...current.filter(existing => existing.url !== entry.url), entry])
        }
      />

      {failures.length > 0 ? (
        <div class="mb-16 border border-white/15 p-4">
          <p class="m-0 mb-2 font-mono text-sm text-white/45">{failureLine(failures.length)}</p>
          <ul class="m-0 flex list-none flex-col gap-1 p-0">
            {failures.map(failure => (
              <li key={failure.url} class="text-warn font-mono text-xs break-all">
                {failure.url}: {failure.reason}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      {directory ? (
        <SourceDirectory directory={directory} />
      ) : (
        <Notice>{failed ? 'could not reach the directory. try reloading the page.' : 'loading…'}</Notice>
      )}
    </>
  );
}
