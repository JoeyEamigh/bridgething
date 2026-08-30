import { extensionOf, type CatalogAppListing } from '@bridgething/catalog';
import { appDetailPath } from '../../lib/app-routes';
import { webHref } from '../../lib/href';
import { ExtensionBadge, ExtensionNote } from './ExtensionNote';
import { installListing, isPlaceholderDownload } from '../../lib/pending-install';
import type { StoreSource } from '../../lib/store-sources';

function SourceBadge({ source }: { source: StoreSource }) {
  return (
    <span class="flex min-w-0 items-center gap-1.5" title={source.url}>
      {source.icon ? (
        <img
          src={source.icon}
          alt=""
          width="14"
          height="14"
          class="size-3.5 shrink-0"
          loading="lazy"
          onError={event => (event.currentTarget as HTMLImageElement).remove()}
        />
      ) : null}
      <span class="truncate">{source.name}</span>
      {source.attested && !source.official ? <span class="text-accent">✓</span> : null}
    </span>
  );
}

export function AppCard({ listing, source }: { listing: CatalogAppListing; source: StoreSource | null }) {
  const newest = listing.newestCompatible;
  const extension = extensionOf(newest);
  const unpublished = newest === null || isPlaceholderDownload(newest.download);
  const sizeKb = newest ? (newest.download.size / 1024).toFixed(0) : null;
  const shot = webHref(listing.app.screenshots?.[0] ?? null);

  return (
    <article class="flex flex-col gap-3 border border-white/15 p-5">
      {shot ? (
        <a href={appDetailPath(listing.app.id)} class="-m-5 mb-0 block border-b-0">
          <img
            src={shot}
            alt={`${listing.app.name} running on a car thing`}
            width="800"
            height="480"
            loading="lazy"
            class="aspect-[5/3] w-full border-b border-white/15 object-cover"
            onError={event => (event.currentTarget as HTMLImageElement).remove()}
          />
        </a>
      ) : null}
      <header class="flex items-start gap-3">
        <div class="size-10 shrink-0 border border-dashed border-white/25" aria-hidden="true">
          {listing.app.icon ? (
            <img
              src={listing.app.icon}
              alt=""
              width="40"
              height="40"
              class="size-full"
              loading="lazy"
              onError={event => (event.currentTarget as HTMLImageElement).remove()}
            />
          ) : null}
        </div>
        <div class="min-w-0 flex-1">
          <h3 class="m-0 text-base/tight font-medium">
            <a href={appDetailPath(listing.app.id)} class="no-underline hover:underline">
              {listing.app.name}
            </a>
          </h3>
          <p class="m-0 font-mono text-sm text-white/45">
            {newest?.version ?? 'no version'} · {listing.app.author}
          </p>
        </div>
      </header>

      <p class="m-0 flex-1 text-sm text-white/65">{listing.app.description}</p>

      {extension ? <ExtensionBadge /> : null}

      {unpublished ? (
        <p class="text-warn m-0 font-mono text-xs">not published yet</p>
      ) : (
        <button type="button" class="btn self-start text-sm" onClick={() => installListing(listing)}>
          install
        </button>
      )}

      <footer class="flex flex-col gap-1 font-mono text-xs text-white/40">
        <div class="flex flex-wrap items-center gap-2">
          {sizeKb !== null && newest!.download.size > 0 ? <span>{sizeKb} KB</span> : <span>size pending</span>}
          {newest ? (
            <>
              <span class="opacity-50">·</span>
              <span>needs lib {newest.min_libbridgething_version}</span>
            </>
          ) : null}
          {newest && newest.permissions.length > 0 ? (
            <>
              <span class="opacity-50">·</span>
              <span>{newest.permissions.join(', ')}</span>
            </>
          ) : null}
          {newest?.role === 'launcher' ? (
            <>
              <span class="opacity-50">·</span>
              <span class="text-accent">launcher</span>
            </>
          ) : null}
          {newest?.provides_overlay ? (
            <>
              <span class="opacity-50">·</span>
              <span class="text-accent">overlay</span>
            </>
          ) : null}
        </div>
        {extension ? <ExtensionNote extension={extension} source={listing.app.source} compact /> : null}
        {source ? <SourceBadge source={source} /> : null}
        {listing.alsoAvailableFrom.length > 0 ? (
          <span class="text-white/30">
            also offered by {listing.alsoAvailableFrom.length} other source
            {listing.alsoAvailableFrom.length === 1 ? '' : 's'}
          </span>
        ) : null}
      </footer>
    </article>
  );
}
