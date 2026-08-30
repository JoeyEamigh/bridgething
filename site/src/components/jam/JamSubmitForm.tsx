import { normalizeSourceUrl, SourceUrlError, type AppEntry, type Catalog } from '@bridgething/catalog';
import { useState } from 'preact/hooks';
import { DirectoryApiError, submitSource } from '../../lib/directory-client';
import { fetchJamCatalog, submitJamEntry } from '../../lib/jam-client';
import { JAM_CATEGORIES, JAM_TIMELINE, jamClosedReason, jamWindow, type JamCategory } from '../../lib/jam';
import { CopyButton } from '../CopyButton';

const INPUT =
  'min-w-0 flex-1 border border-white/25 bg-transparent px-3 py-2 font-mono text-sm text-white placeholder:text-white/30 focus:border-white/50 focus:outline-none';

function reason(err: unknown): string {
  return err instanceof DirectoryApiError || err instanceof SourceUrlError ? err.message : String(err);
}

const SOURCE_BUDGET_SPENT = 'no new sources from this network for an hour.';

async function relayedCatalog(url: string): Promise<Catalog | null> {
  try {
    return await fetchJamCatalog(url);
  } catch (err) {
    if (err instanceof DirectoryApiError && err.status === 404) return null;
    throw err;
  }
}

async function registerSource(url: string): Promise<void> {
  try {
    await submitSource(url);
  } catch (err) {
    if (err instanceof DirectoryApiError && err.status === 429) throw new DirectoryApiError(SOURCE_BUDGET_SPENT, 429);
    throw err;
  }
}

function Entered({ claim }: { claim: string | null }) {
  return (
    <div class="border-accent/40 bg-accent-soft border p-6">
      <p class="m-0 font-medium">you are in the jam.</p>
      <p class="m-0 mt-2 text-sm text-white/70">
        the panel can see your entry. the store listing appears once a reviewer lists your source. resubmit before the
        deadline to change the video or the category.
      </p>
      {claim !== null ? (
        <div class="border-warn/40 mt-4 border p-4">
          <p class="m-0 font-mono text-xs text-white/45">your claim token</p>
          <div class="mt-2 flex flex-wrap items-center gap-3">
            <code class="border border-white/25 px-3 py-2 font-mono text-sm break-all select-all">{claim}</code>
            <CopyButton value={claim} />
          </div>
          <p class="text-warn m-0 mt-2 text-sm">shown once. you need it to update this entry.</p>
        </div>
      ) : null}
      <p class="m-0 mt-4 flex flex-wrap gap-4 font-mono text-sm">
        <a href="/apps" target="_blank" rel="noreferrer">
          the app directory
        </a>
        <a href="#gallery">the gallery</a>
      </p>
    </div>
  );
}

function AppOption({ app, picked, onPick }: { app: AppEntry; picked: boolean; onPick: (id: string) => void }) {
  return (
    <li>
      <button
        type="button"
        aria-pressed={picked}
        onClick={() => onPick(app.id)}
        class={`flex w-full items-start gap-3 border p-3 text-left transition-colors ${
          picked ? 'border-accent bg-accent-soft' : 'border-white/15 hover:border-white/40'
        }`}>
        {app.icon ? (
          <img src={app.icon} alt="" width="40" height="40" class="size-10 shrink-0 border border-white/10" />
        ) : (
          <span class="text-warn grid size-10 shrink-0 place-items-center border border-dashed border-white/20 font-mono text-xs">
            none
          </span>
        )}
        <span class="min-w-0">
          <span class="block font-medium">{app.name}</span>
          <span class="block text-sm text-white/55">{app.description}</span>
          <span class="block font-mono text-xs text-white/35">by {app.author}</span>
          {(app.screenshots?.length ?? 0) === 0 ? (
            <span class="text-warn block font-mono text-xs">no screenshots in the catalog</span>
          ) : null}
        </span>
      </button>
    </li>
  );
}

function OpenForm() {
  const [sourceUrl, setSourceUrl] = useState('');
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [appId, setAppId] = useState('');
  const [category, setCategory] = useState<JamCategory>('utility');
  const [videoUrl, setVideoUrl] = useState('');
  const [discord, setDiscord] = useState('');
  const [claim, setClaim] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [entered, setEntered] = useState<{ claim: string | null } | null>(null);

  function onSourceInput(event: Event) {
    setSourceUrl((event.target as HTMLInputElement).value);
    setCatalog(null);
    setAppId('');
  }

  async function onProbe(event: Event) {
    event.preventDefault();
    if (busy) return;

    let candidate: string;
    try {
      candidate = normalizeSourceUrl(sourceUrl);
    } catch (err) {
      setError(reason(err));
      return;
    }
    setSourceUrl(candidate);

    setBusy(true);
    setError(null);
    try {
      let found = await relayedCatalog(candidate);
      if (found === null) {
        await registerSource(candidate);
        found = await fetchJamCatalog(candidate);
      }
      setCatalog(found);
      setAppId(found.apps.length === 1 ? (found.apps[0]?.id ?? '') : '');
    } catch (err) {
      setCatalog(null);
      setError(reason(err));
    } finally {
      setBusy(false);
    }
  }

  async function onSubmit(event: Event) {
    event.preventDefault();
    if (!appId || busy) return;

    setBusy(true);
    setError(null);
    try {
      const submitted = await submitJamEntry({
        source_url: sourceUrl.trim(),
        app_id: appId,
        category,
        video_url: videoUrl.trim(),
        discord: discord.trim(),
        wishlist: '',
        claim: claim.trim() || null,
      });
      setEntered({ claim: submitted.claim });
    } catch (err) {
      setError(reason(err));
    } finally {
      setBusy(false);
    }
  }

  if (entered !== null) return <Entered claim={entered.claim} />;

  return (
    <div class="flex flex-col gap-8">
      <form class="flex flex-col gap-2" onSubmit={onProbe}>
        <label class="font-mono text-sm text-white/45" for="jam-source">
          1 - your catalog source
        </label>
        <div class="flex flex-wrap gap-3">
          <input
            id="jam-source"
            type="text"
            inputMode="url"
            autocomplete="url"
            spellcheck={false}
            value={sourceUrl}
            disabled={busy}
            onInput={onSourceInput}
            placeholder="https://example.com/catalog.json"
            class={INPUT}
          />
          <button type="submit" class="btn" disabled={busy}>
            {busy ? 'checking…' : 'load apps'}
          </button>
        </div>
        <p class="m-0 font-mono text-xs text-white/35">
          <code>catalog.v1</code>
        </p>
      </form>

      {catalog !== null ? (
        <form class="flex flex-col gap-6" onSubmit={onSubmit}>
          <fieldset class="m-0 flex flex-col gap-2 border-0 p-0">
            <legend class="mb-2 font-mono text-sm text-white/45">2 - the app</legend>
            {catalog.apps.length === 0 ? (
              <p class="text-warn m-0 font-mono text-sm">
                {catalog.repo.name} is a catalog, but its apps list is empty. add your app to it and load it again.
              </p>
            ) : (
              <ul class="m-0 grid list-none grid-cols-1 gap-3 p-0 sm:grid-cols-2">
                {catalog.apps.map(app => (
                  <AppOption key={app.id} app={app} picked={app.id === appId} onPick={setAppId} />
                ))}
              </ul>
            )}
          </fieldset>

          <div class="grid grid-cols-1 gap-6 sm:grid-cols-2">
            <div class="flex flex-col gap-2">
              <label class="font-mono text-sm text-white/45" for="jam-category">
                3 - primary category
              </label>
              <select
                id="jam-category"
                value={category}
                disabled={busy}
                onChange={event => setCategory((event.target as HTMLSelectElement).value as JamCategory)}
                class={INPUT}>
                {JAM_CATEGORIES.map(option => (
                  <option key={option.id} value={option.id} class="bg-bg">
                    {option.label}
                  </option>
                ))}
              </select>
              <p class="m-0 font-mono text-xs text-white/35">you can still win other categories</p>
            </div>

            <div class="flex flex-col gap-2">
              <label class="font-mono text-sm text-white/45" for="jam-video">
                4 - video
              </label>
              <input
                id="jam-video"
                type="text"
                inputMode="url"
                autocomplete="url"
                spellcheck={false}
                value={videoUrl}
                disabled={busy}
                onInput={event => setVideoUrl((event.target as HTMLInputElement).value)}
                placeholder="https://youtu.be/…"
                class={INPUT}
              />
              <p class="m-0 font-mono text-xs text-white/35">youtube link preferred</p>
            </div>

            <div class="flex flex-col gap-2">
              <label class="font-mono text-sm text-white/45" for="jam-discord">
                5 - your discord handle
              </label>
              <input
                id="jam-discord"
                type="text"
                value={discord}
                disabled={busy}
                onInput={event => setDiscord((event.target as HTMLInputElement).value)}
                placeholder="yourhandle"
                class={INPUT}
              />
              <p class="m-0 font-mono text-xs text-white/35">needed for prizes</p>
            </div>

            <div class="flex flex-col gap-2">
              <label class="font-mono text-sm text-white/45" for="jam-claim">
                6 - claim token
              </label>
              <input
                id="jam-claim"
                type="text"
                value={claim}
                disabled={busy}
                onInput={event => setClaim((event.target as HTMLInputElement).value)}
                placeholder="leave empty for new entries"
                class={INPUT}
              />
              <p class="m-0 font-mono text-xs text-white/35">needed to edit an existing entry</p>
            </div>
          </div>

          <div class="flex flex-wrap items-center gap-4">
            <button type="submit" class="btn btn-primary" disabled={busy || !appId}>
              {busy ? 'submitting…' : 'enter the jam'}
            </button>
            <p class="m-0 font-mono text-xs text-white/35">resubmitting the same app updates its entry.</p>
          </div>
        </form>
      ) : null}

      {error !== null ? (
        <p class="text-warn border-warn/40 m-0 border px-3 py-2 font-mono text-sm" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}

export function JamSubmitForm() {
  const submissions = jamWindow(JAM_TIMELINE);
  if (submissions.open) return <OpenForm />;

  return (
    <p class="m-0 border border-white/20 p-6 font-mono text-sm text-white/60">
      {jamClosedReason(JAM_TIMELINE, submissions)}
    </p>
  );
}
