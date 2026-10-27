import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';
import {
  featuredListing,
  search,
  sourceLine,
  type CatalogAppListing,
  type StoreCatalog,
  type StorePin,
} from '@bridgething/catalog';
import { appDetailPath } from '../../lib/app-routes';
import { fetchFeaturedPin, fetchMergedApps, pinFeatured, unpinFeatured } from '../../lib/directory-client';
import { catalogOf } from '../../lib/use-store';
import { AppIcon } from '../store/parts';
import { Group, INPUT, Notice, reason } from './shared';

const RESULTS = 8;

function pinOf(listing: CatalogAppListing): StorePin {
  return { app_id: listing.app.id, source_url: listing.sourceUrl };
}

function samePin(pin: StorePin | null, listing: CatalogAppListing): boolean {
  return pin !== null && pin.app_id === listing.app.id.toLowerCase() && pin.source_url === listing.sourceUrl;
}

function Line({
  listing,
  catalog,
  action,
}: {
  listing: CatalogAppListing;
  catalog: StoreCatalog;
  action: ComponentChildren;
}) {
  return (
    <li class="flex items-center gap-3 px-4 py-3">
      <AppIcon app={listing.app} size={40} />
      <div class="min-w-0 flex-1">
        <a href={appDetailPath(listing.app.id)} class="block truncate border-b-0 hover:text-white">
          {listing.app.name}
        </a>
        <p class="m-0 truncate font-mono text-xs text-white/40">
          {listing.app.author} · {sourceLine(catalog.sources[listing.sourceUrl])}
        </p>
      </div>
      {action}
    </li>
  );
}

export function FeaturedPin({ token }: { token: string }) {
  const [pin, setPin] = useState<StorePin | null | undefined>(undefined);
  const [catalog, setCatalog] = useState<StoreCatalog | null>(null);
  const [query, setQuery] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const controller = new AbortController();
    Promise.all([fetchFeaturedPin(token), fetchMergedApps({ origin: '', signal: controller.signal })])
      .then(([current, apps]) => {
        setPin(current);
        setCatalog(catalogOf({ apps, directory: [] }));
      })
      .catch(err => {
        if (!controller.signal.aborted) setError(reason(err));
      });
    return () => controller.abort();
  }, [token]);

  const pinned = useMemo(
    () => (catalog && pin ? (catalog.listings.find(listing => samePin(pin, listing)) ?? null) : null),
    [catalog, pin],
  );
  const rotating = useMemo(
    () => (catalog && !pin ? featuredListing(catalog.listings, null, new Date()) : null),
    [catalog, pin],
  );
  const matches = useMemo(() => (catalog ? search(catalog.listings, query).slice(0, RESULTS) : []), [catalog, query]);

  async function run(change: () => Promise<StorePin | null>) {
    setBusy(true);
    setError(null);
    try {
      setPin(await change());
    } catch (err) {
      setError(reason(err));
    } finally {
      setBusy(false);
    }
  }

  const unpin = (
    <button
      type="button"
      class="btn btn-sm"
      disabled={busy}
      onClick={() => void run(() => unpinFeatured(token).then(() => null))}>
      unpin
    </button>
  );

  return (
    <Group title="featured" count={pin ? 1 : 0}>
      {error !== null ? (
        <div class="mb-4">
          <Notice kind="err">{error}</Notice>
        </div>
      ) : null}

      {catalog === null || pin === undefined ? (
        error === null ? (
          <p class="m-0 font-mono text-sm text-white/45">loading…</p>
        ) : null
      ) : (
        <>
          <div class="mb-4 border border-white/15">
            {pin && pinned ? (
              <ul class="m-0 list-none p-0">
                <Line listing={pinned} catalog={catalog} action={unpin} />
              </ul>
            ) : pin ? (
              <div class="flex items-center justify-between gap-3 px-4 py-3">
                <p class="text-warn m-0 font-mono text-sm break-all">
                  pinned {pin.app_id} is not in the store right now, so the rotation shows instead
                </p>
                {unpin}
              </div>
            ) : (
              <p class="m-0 px-4 py-3 font-mono text-sm text-white/50">
                nothing pinned. rotating today: {rotating ? rotating.app.name : 'nothing with a screenshot'}
              </p>
            )}
          </div>

          <input
            type="search"
            value={query}
            aria-label="find an app to feature"
            onInput={event => setQuery((event.target as HTMLInputElement).value)}
            placeholder="find an app to pin"
            class={`${INPUT} w-full`}
          />

          {matches.length > 0 ? (
            <ul class="m-0 mt-3 list-none border border-white/15 p-0 [&>li+li]:border-t [&>li+li]:border-white/10">
              {matches.map(listing => (
                <Line
                  key={`${listing.sourceUrl} ${listing.app.id}`}
                  listing={listing}
                  catalog={catalog}
                  action={
                    samePin(pin, listing) ? (
                      <span class="text-accent font-mono text-xs">pinned</span>
                    ) : (
                      <button
                        type="button"
                        class="btn btn-sm"
                        disabled={busy}
                        onClick={() => void run(() => pinFeatured(token, pinOf(listing)))}>
                        pin
                      </button>
                    )
                  }
                />
              ))}
            </ul>
          ) : query.trim() ? (
            <p class="m-0 mt-3 font-mono text-sm text-white/45">no matches</p>
          ) : null}

          <p class="m-0 mt-3 font-mono text-xs text-white/35">the store picks a change up within a minute</p>
        </>
      )}
    </Group>
  );
}
