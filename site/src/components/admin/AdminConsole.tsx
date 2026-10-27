import { useCallback, useEffect, useState } from 'preact/hooks';
import { fetchAdminSources, type AdminEntry } from '../../lib/directory-client';
import { INPUT, Notice, reason } from './shared';
import { FeaturedPin } from './FeaturedPin';
import { SourcesTab } from './SourcesTab';

const TOKEN_KEY = 'bridgething:admin-token';

export function AdminConsole() {
  const [draft, setDraft] = useState('');
  const [token, setToken] = useState('');
  const [entries, setEntries] = useState<AdminEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const sign = useCallback(async (candidate: string, remember: boolean) => {
    if (!candidate) return;
    setBusy(true);
    setError(null);
    try {
      setEntries(await fetchAdminSources(candidate));
      setToken(candidate);
      if (remember) localStorage.setItem(TOKEN_KEY, candidate);
    } catch (err) {
      setEntries(null);
      setToken('');
      if (remember) setError(reason(err));
      localStorage.removeItem(TOKEN_KEY);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    const stored = typeof localStorage === 'undefined' ? null : localStorage.getItem(TOKEN_KEY);
    if (stored) {
      setDraft(stored);
      void sign(stored, false);
    }
  }, [sign]);

  if (entries === null) {
    return (
      <>
        <form
          class="mb-6 flex flex-wrap gap-3"
          onSubmit={event => {
            event.preventDefault();
            void sign(draft.trim(), true);
          }}>
          <input
            type="password"
            required
            value={draft}
            disabled={busy}
            aria-label="admin token"
            onInput={event => setDraft((event.target as HTMLInputElement).value)}
            placeholder="admin token"
            class={INPUT}
          />
          <button type="submit" class="btn btn-primary" disabled={busy}>
            {busy ? 'checking…' : 'sign in'}
          </button>
        </form>

        {error !== null ? <Notice kind="err">{error}</Notice> : null}
      </>
    );
  }

  return (
    <>
      <p class="mb-8 flex justify-end border-b border-white/20 pb-3">
        <button
          type="button"
          class="btn btn-sm btn-ghost"
          onClick={() => {
            localStorage.removeItem(TOKEN_KEY);
            setEntries(null);
            setToken('');
            setDraft('');
          }}>
          sign out
        </button>
      </p>

      <FeaturedPin token={token} />
      <SourcesTab token={token} initial={entries} />
    </>
  );
}
