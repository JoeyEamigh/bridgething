import { normalizeSourceUrl, SourceUrlError } from '@bridgething/catalog';
import { useState } from 'preact/hooks';
import { DirectoryApiError, submitSource, type DirectoryEntry } from '../../lib/directory-client';

type Outcome = { kind: 'ok' | 'err'; message: string } | null;

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
      const entry = await submitSource(candidate);
      setUrl('');
      setOutcome({
        kind: 'ok',
        message:
          entry.status === 'quarantined'
            ? `${entry.name} is in the directory as unreviewed. it reaches the phone app once a reviewer lists it.`
            : `${entry.name} is already in the directory as ${entry.status}.`,
      });
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
    <details class="mb-10 border border-white/15 p-4">
      <summary class="cursor-pointer font-medium">submit a source</summary>

      <p class="mt-2 mb-4 max-w-2xl text-sm text-white/60">
        the https url of your <code>catalog.v1</code> document. it must be reachable, parse, and send{' '}
        <code>Access-Control-Allow-Origin</code>. <a href="/docs/publishing-apps">publishing docs</a>.
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
    </details>
  );
}
