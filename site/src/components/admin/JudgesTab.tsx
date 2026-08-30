import { useCallback, useEffect, useState } from 'preact/hooks';
import { createJudge, fetchJudges, revokeJudge } from '../../lib/jam-client';
import type { JudgeRecord } from '../../lib/principal';
import { CopyButton } from '../CopyButton';
import { Empty, INPUT, Loading, Notice, reason } from './shared';

function Minted({ handle, token }: { handle: string; token: string }) {
  return (
    <div class="border-accent/40 bg-accent-soft mb-8 border p-4">
      <p class="m-0 font-medium">{handle} is a judge.</p>
      <p class="m-0 mt-1 text-sm text-white/70">
        this token is shown once. hand it over now; only its hash is stored, so it cannot be read again.
      </p>
      <div class="mt-3 flex flex-wrap items-center gap-3">
        <code class="border border-white/25 px-3 py-2 font-mono text-sm break-all">{token}</code>
        <CopyButton value={token} />
      </div>
    </div>
  );
}

export function JudgesTab({ token }: { token: string }) {
  const [judges, setJudges] = useState<JudgeRecord[] | null>(null);
  const [handle, setHandle] = useState('');
  const [minted, setMinted] = useState<{ handle: string; token: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setJudges(await fetchJudges(token));
    } catch (err) {
      setError(reason(err));
    }
  }, [token]);

  useEffect(() => {
    void load();
  }, [load]);

  async function onCreate(event: Event) {
    event.preventDefault();
    const candidate = handle.trim();
    if (!candidate || busy) return;

    setBusy(true);
    setError(null);
    try {
      const created = await createJudge({ token, handle: candidate });
      setMinted({ handle: created.judge.handle, token: created.token });
      setHandle('');
      await load();
    } catch (err) {
      setError(reason(err));
    } finally {
      setBusy(false);
    }
  }

  async function onRevoke(target: string) {
    setBusy(true);
    setError(null);
    try {
      await revokeJudge({ token, handle: target });
      setMinted(current => (current?.handle === target ? null : current));
      await load();
    } catch (err) {
      setError(reason(err));
    } finally {
      setBusy(false);
    }
  }

  if (judges === null && error === null) return <Loading what="judges" />;

  return (
    <>
      <form class="mb-6 flex flex-wrap gap-3" onSubmit={onCreate}>
        <input
          type="text"
          required
          value={handle}
          disabled={busy}
          aria-label="new judge handle"
          onInput={event => setHandle((event.target as HTMLInputElement).value)}
          placeholder="discord handle"
          class={INPUT}
        />
        <button type="submit" class="btn btn-primary" disabled={busy}>
          {busy ? 'working…' : 'mint a token'}
        </button>
      </form>

      {error !== null ? (
        <div class="mb-6">
          <Notice kind="err">{error}</Notice>
        </div>
      ) : null}

      {minted !== null ? <Minted handle={minted.handle} token={minted.token} /> : null}

      {(judges ?? []).length === 0 ? (
        <Empty what="no judges yet." />
      ) : (
        <ul class="grid list-none grid-cols-1 gap-3 p-0 md:grid-cols-2">
          {(judges ?? []).map(judge => (
            <li key={judge.handle} class="flex items-center justify-between gap-3 border border-white/15 p-4">
              <span>
                <span class="block font-medium">{judge.handle}</span>
                <span class="block font-mono text-xs text-white/40">since {judge.created_at.slice(0, 10)}</span>
              </span>
              <button type="button" class="btn btn-sm" disabled={busy} onClick={() => void onRevoke(judge.handle)}>
                revoke
              </button>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
