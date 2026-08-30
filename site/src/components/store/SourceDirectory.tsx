import type { DirectoryEntry, SourceStatus } from '../../lib/directory-client';
import { webHref } from '../../lib/href';

const PILL_FOR: Record<SourceStatus, string> = {
  attested: 'pill pill-stable',
  listed: 'pill pill-default',
  quarantined: 'pill pill-experimental',
  rejected: 'pill pill-warn',
};

const PILL_LABEL: Record<SourceStatus, string> = {
  attested: 'attested',
  listed: 'listed',
  quarantined: 'unreviewed',
  rejected: 'removed',
};

function Health({ entry }: { entry: DirectoryEntry }) {
  if (!entry.last_check_ok) {
    return (
      <p class="text-warn m-0 mt-2 text-xs">
        unreachable as of the last check{entry.last_check_error ? `: ${entry.last_check_error}` : ''}
      </p>
    );
  }
  if (entry.downloads_cors_ok === false) {
    return (
      <p class="text-experimental m-0 mt-2 text-xs">
        this source's downloads are not readable from a browser, so installing needs the phone app.
      </p>
    );
  }
  return null;
}

function SourceRow({ entry }: { entry: DirectoryEntry }) {
  const homepage = webHref(entry.homepage);

  return (
    <li class="flex flex-col border border-white/15 p-4">
      <div class="flex items-start gap-3">
        <div class="size-8 shrink-0 border border-dashed border-white/25" aria-hidden="true">
          {entry.icon ? (
            <img
              src={entry.icon}
              alt=""
              width="32"
              height="32"
              class="size-full"
              loading="lazy"
              onError={event => (event.currentTarget as HTMLImageElement).remove()}
            />
          ) : null}
        </div>

        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-baseline justify-between gap-2">
            <span class="font-medium">{entry.name}</span>
            <span class={PILL_FOR[entry.status]}>{PILL_LABEL[entry.status]}</span>
          </div>

          {entry.description ? <p class="m-0 mt-1 text-sm text-white/60">{entry.description}</p> : null}

          <p class="m-0 mt-1 font-mono text-xs break-all text-white/35">{entry.url}</p>

          <p class="m-0 mt-1 font-mono text-xs text-white/35">
            {entry.app_count} app{entry.app_count === 1 ? '' : 's'}
            {homepage ? (
              <>
                {' · '}
                <a href={homepage} rel="noopener noreferrer nofollow" target="_blank">
                  homepage
                </a>
              </>
            ) : null}
          </p>

          <Health entry={entry} />
        </div>
      </div>
    </li>
  );
}

function Rows({ entries }: { entries: DirectoryEntry[] }) {
  return (
    <ul class="grid list-none grid-cols-1 gap-4 p-0 md:grid-cols-2">
      {entries.map(entry => (
        <SourceRow key={entry.url} entry={entry} />
      ))}
    </ul>
  );
}

export function SourceDirectory({ directory }: { directory: DirectoryEntry[] | null }) {
  if (directory === null) {
    return (
      <section class="mb-16">
        <header class="mb-4 border-b border-white/20 pb-2">
          <h2 class="m-0">where these come from</h2>
        </header>
        <p class="font-mono text-sm text-white/45">loading the directory…</p>
      </section>
    );
  }

  const published = directory.filter(entry => entry.status === 'attested' || entry.status === 'listed');
  const unreviewed = directory.filter(entry => entry.status === 'quarantined');

  return (
    <section class="mb-16">
      <header class="mb-4 flex flex-wrap items-baseline justify-between gap-3 border-b border-white/20 pb-2">
        <h2 class="m-0">where these come from</h2>
        <p class="m-0 font-mono text-sm text-white/40">{published.length}</p>
      </header>

      {published.length > 0 ? <Rows entries={published} /> : <p class="text-sm text-white/45">nothing listed yet.</p>}

      {unreviewed.length > 0 ? (
        <details class="mt-8 border border-white/15 p-4">
          <summary class="cursor-pointer font-medium">unreviewed ({unreviewed.length})</summary>
          <Rows entries={unreviewed} />
        </details>
      ) : null}
    </section>
  );
}
