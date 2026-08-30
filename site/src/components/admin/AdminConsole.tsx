import { useCallback, useEffect, useState } from 'preact/hooks';
import { fetchPrincipal } from '../../lib/jam-client';
import type { Principal } from '../../lib/principal';
import { EntriesTab } from './EntriesTab';
import { JudgesTab } from './JudgesTab';
import { ReviewTab } from './ReviewTab';
import { INPUT, Notice, reason } from './shared';
import { SourcesTab } from './SourcesTab';
import { TallyTab } from './TallyTab';

const TOKEN_KEY = 'bridgething:admin-token';

const ADMIN_TABS = ['sources', 'entries', 'judges', 'tally'] as const;
const JUDGE_TABS = ['review'] as const;

type Tab = (typeof ADMIN_TABS)[number] | (typeof JUDGE_TABS)[number];

function tabsFor(caller: Principal): readonly Tab[] {
  return caller.role === 'admin' ? ADMIN_TABS : JUDGE_TABS;
}

function Panel({ tab, token }: { tab: Tab; token: string }) {
  switch (tab) {
    case 'sources':
      return <SourcesTab token={token} />;
    case 'entries':
      return <EntriesTab token={token} />;
    case 'judges':
      return <JudgesTab token={token} />;
    case 'tally':
      return <TallyTab token={token} />;
    case 'review':
      return <ReviewTab token={token} />;
  }
}

export function AdminConsole() {
  const [draft, setDraft] = useState('');
  const [token, setToken] = useState('');
  const [caller, setCaller] = useState<Principal | null>(null);
  const [tab, setTab] = useState<Tab>('sources');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const sign = useCallback(async (candidate: string, remember: boolean) => {
    if (!candidate) return;
    setBusy(true);
    setError(null);
    try {
      const resolved = await fetchPrincipal(candidate);
      setCaller(resolved);
      setToken(candidate);
      setTab(tabsFor(resolved)[0]!);
      if (remember) localStorage.setItem(TOKEN_KEY, candidate);
    } catch (err) {
      setCaller(null);
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

  if (caller === null) {
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
            aria-label="admin or judge token"
            onInput={event => setDraft((event.target as HTMLInputElement).value)}
            placeholder="admin or judge token"
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

  const tabs = tabsFor(caller);

  return (
    <>
      <div class="mb-8 flex flex-wrap items-center justify-between gap-4 border-b border-white/20 pb-3">
        <nav class="flex flex-wrap gap-1" aria-label="admin sections">
          {tabs.map(name => (
            <button
              key={name}
              type="button"
              aria-current={tab === name ? 'page' : undefined}
              onClick={() => setTab(name)}
              class={`border px-4 py-2 font-mono text-sm ${
                tab === name
                  ? 'border-accent text-accent bg-accent-soft'
                  : 'border-transparent text-white/50 hover:text-white'
              }`}>
              {name}
            </button>
          ))}
        </nav>

        <p class="m-0 flex items-center gap-4 font-mono text-sm text-white/40">
          <span>{caller.role === 'admin' ? 'admin' : `judge ${caller.handle}`}</span>
          <button
            type="button"
            class="btn btn-sm btn-ghost"
            onClick={() => {
              localStorage.removeItem(TOKEN_KEY);
              setCaller(null);
              setToken('');
              setDraft('');
            }}>
            sign out
          </button>
        </p>
      </div>

      <Panel tab={tab} token={token} />
    </>
  );
}
