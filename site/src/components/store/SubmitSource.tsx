import { normalizeSourceUrl, SourceUrlError } from '@bridgething/catalog';
import { useState } from 'preact/hooks';
import { DirectoryApiError, submitSource, type DirectoryEntry } from '../../lib/directory-client';

type Outcome = { kind: 'ok' | 'err'; message: string } | null;

function submitted(entry: DirectoryEntry, created: boolean): string {
  if (created)
    return `${entry.name} is listed. its apps are in the store and it is in the phone app's suggested sources.`;
  if (entry.status === 'quarantined') return `${entry.name} is refreshed and held for review.`;
  return `${entry.name} is refreshed. it is ${entry.status} in the directory.`;
}

export function SubmitSource({ onSubmitted }: { onSubmitted: (entry: DirectoryEntry) => void }) {
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<Outcome>(null);

  async function onSubmit(event: Event) {
    event.preventDefault();
    if (busy) return;

    let candidate: string;
    try {
      candidate = normalizeSourceUrl(url);
    } catch (err) {
      setOutcome({ kind: 'err', message: err instanceof SourceUrlError ? err.message : String(err) });
      return;
    }
    setUrl(candidate);

    setBusy(true);
    setOutcome(null);

    try {
      const { entry, created } = await submitSource(candidate, { origin: window.location.origin });
      setUrl('');
      setOutcome({ kind: 'ok', message: submitted(entry, created) });
      onSubmitted(entry);
    } catch (err) {
      setOutcome({
        kind: 'err',
        message: err instanceof DirectoryApiError ? err.message : `submitting failed: ${String(err)}`,
      });
    } finally {
      setBusy(false);
    }
  }

  return (
    <section class="mb-16">
      <header class="mb-4 border-b border-white/20 pb-2">
        <h2 class="m-0 text-xl">submit a source</h2>
      </header>

      <p class="mb-4 max-w-2xl text-sm text-white/60">
        the https url of your <code>catalog.v1</code> document. it must be reachable, parse, and send{' '}
        <code>Access-Control-Allow-Origin</code>. resubmit it to pick up a new version right away.{' '}
        <a href="/docs/publishing-apps">publishing docs</a>.
      </p>

      <form class="flex flex-wrap gap-3" onSubmit={onSubmit}>
        <input
          type="text"
          inputMode="url"
          autocomplete="url"
          spellcheck={false}
          value={url}
          disabled={busy}
          onInput={event => setUrl((event.target as HTMLInputElement).value)}
          placeholder="https://example.com/catalog.json"
          class="min-w-0 flex-1 border border-white/25 bg-transparent px-3 py-2 font-mono text-sm text-white placeholder:text-white/30 focus:border-white/50 focus:outline-none"
        />
        <button type="submit" class="btn btn-primary" disabled={busy}>
          {busy ? 'checking…' : 'submit'}
        </button>
      </form>

      {outcome ? (
        <p class={`mt-3 text-sm ${outcome.kind === 'ok' ? 'text-ok' : 'text-warn'}`}>{outcome.message}</p>
      ) : null}
    </section>
  );
}
