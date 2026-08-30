import { useEffect, useState } from 'preact/hooks';
import { appDetailPath } from '../../lib/app-routes';
import { webHref } from '../../lib/href';
import { fetchJamGallery } from '../../lib/jam-client';
import { jamCategoryLabel, type JamListing } from '../../lib/jam';

export function JamEntryCard({ listing }: { listing: JamListing }) {
  const video = webHref(listing.video_url);
  const repo = webHref(listing.repo);

  const shot = webHref(listing.screenshot);

  return (
    <li class="flex flex-col gap-3 border border-white/15 p-4">
      {shot ? (
        <img
          src={shot}
          alt={`${listing.name ?? 'the app'} running on a car thing`}
          width="800"
          height="480"
          loading="lazy"
          class="-m-4 mb-1 aspect-[5/3] w-[calc(100%+2rem)] max-w-none border-b border-white/15 object-cover"
        />
      ) : null}
      <div class="flex items-start gap-3">
        {listing.icon ? (
          <img src={listing.icon} alt="" width="48" height="48" class="size-12 shrink-0 border border-white/10" />
        ) : (
          <span class="size-12 shrink-0 border border-dashed border-white/20" />
        )}
        <div class="min-w-0">
          <p class="m-0 font-medium">{listing.name ?? 'unnamed entry'}</p>
          <p class="m-0 font-mono text-xs text-white/40">
            {listing.author ? `by ${listing.author} · ` : ''}
            {jamCategoryLabel(listing.category)}
          </p>
        </div>
      </div>

      {listing.description ? <p class="m-0 text-sm text-pretty text-white/60">{listing.description}</p> : null}

      <p class="m-0 mt-auto flex flex-wrap gap-4 font-mono text-sm">
        {video ? (
          <a href={video} rel="noreferrer noopener">
            video
          </a>
        ) : null}
        {repo ? (
          <a href={repo} rel="noreferrer noopener">
            code
          </a>
        ) : null}
        <a href={appDetailPath(listing.app_id)}>listing</a>
      </p>
    </li>
  );
}

export function JamGallery() {
  const [listings, setListings] = useState<JamListing[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    fetchJamGallery({ signal: controller.signal })
      .then(setListings)
      .catch(err => {
        if (!controller.signal.aborted) {
          setListings([]);
          setError(String(err));
        }
      });
    return () => controller.abort();
  }, []);

  if (listings === null) return <p class="m-0 font-mono text-sm text-white/40">loading entries…</p>;

  if (listings.length === 0) {
    return <p class="m-0 font-mono text-sm text-white/40">{error ?? 'no entries.'}</p>;
  }

  return (
    <>
      <p class="m-0 mb-6 font-mono text-sm text-white/40">
        {listings.length} {listings.length === 1 ? 'entry' : 'entries'}
      </p>
      <ul class="m-0 grid list-none grid-cols-1 gap-4 p-0 sm:grid-cols-2 lg:grid-cols-3">
        {listings.map(listing => (
          <JamEntryCard key={listing.app_id} listing={listing} />
        ))}
      </ul>
    </>
  );
}
