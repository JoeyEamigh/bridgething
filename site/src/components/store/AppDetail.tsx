import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';
import {
  aggregate,
  extensionOf,
  newestCompatible,
  sortNewestFirst,
  type AppEntry,
  type CatalogAppListing,
} from '@bridgething/catalog';
import { appIdFromPath } from '../../lib/app-routes';
import { fetchMergedApps, type InstallCount, type MergedCatalog } from '../../lib/directory-client';
import { webHref } from '../../lib/href';
import { installListing, isPlaceholderDownload } from '../../lib/pending-install';
import { orderedByTrust, sourceMap, type StoreSource } from '../../lib/store-sources';
import { ExtensionBadge, ExtensionNote } from './ExtensionNote';

export type BakedApp = { app: AppEntry; source: StoreSource };

type Resolved = { listing: CatalogAppListing; source: StoreSource | null };

function formatDate(raw: string): string {
  const at = Date.parse(raw);
  if (Number.isNaN(at)) return raw;
  return new Date(at).toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' });
}

function bakedListing(baked: BakedApp): CatalogAppListing {
  return {
    app: baked.app,
    sourceUrl: baked.source.url,
    newestCompatible: newestCompatible(baked.app, null),
    installedVersion: null,
    updateAvailable: false,
    alsoAvailableFrom: [],
    installs: 0,
  };
}

function Notice({ children }: { children: ComponentChildren }) {
  return (
    <div class="border border-dashed border-white/25 p-16 text-center">
      <p class="m-0 text-white/60">{children}</p>
    </div>
  );
}

function Trust({ source }: { source: StoreSource | null }) {
  if (!source) return <p class="m-0 font-mono text-sm text-white/40">source: unknown</p>;

  const href = webHref(source.url);

  return (
    <>
      <p class="m-0 font-mono text-sm break-all text-white/40">
        source: {href ? <a href={href}>{source.name}</a> : source.name}
      </p>
      {source.official || source.attested ? null : (
        <p class="m-0 mt-2 text-sm text-white/50">this source is unreviewed</p>
      )}
    </>
  );
}

function Detail({ listing, source }: Resolved) {
  const app = listing.app;
  const versions = sortNewestFirst(app.versions);
  const newest = listing.newestCompatible ?? versions[0] ?? null;
  const extension = extensionOf(newest);
  const unpublished = newest === null || isPlaceholderDownload(newest.download);
  const homepage = webHref(app.homepage);
  const repo = webHref(app.source);
  const shots = (app.screenshots ?? []).map(webHref).filter((url): url is string => url !== null);

  return (
    <>
      <header class="mb-10 flex flex-wrap items-start gap-5">
        <div class="size-16 shrink-0 border border-dashed border-white/25" aria-hidden="true">
          {app.icon ? (
            <img
              src={app.icon}
              alt=""
              width="64"
              height="64"
              class="size-full"
              onError={event => (event.currentTarget as HTMLImageElement).remove()}
            />
          ) : null}
        </div>
        <div class="min-w-0 flex-1">
          <h1 class="mb-1">{app.name.toLowerCase()}</h1>
          <p class="m-0 text-white/60">{app.description}</p>
          {extension ? (
            <div class="mt-3 flex">
              <ExtensionBadge />
            </div>
          ) : null}
          <p class="m-0 mt-3 flex flex-wrap items-baseline gap-2 font-mono text-sm text-white/45">
            <span>{app.author}</span>
            {newest ? (
              <>
                <span class="opacity-50">·</span>
                <span>{newest.version}</span>
                <span class="opacity-50">·</span>
                <time datetime={newest.released_at}>{formatDate(newest.released_at)}</time>
              </>
            ) : null}
          </p>
        </div>
      </header>

      {shots.length > 0 ? (
        <section class="mb-12">
          <ul class="m-0 flex list-none gap-4 overflow-x-auto p-0">
            {shots.map(shot => (
              <li key={shot} class="shrink-0">
                <img
                  src={shot}
                  alt={`${app.name} running on a car thing`}
                  width="800"
                  height="480"
                  loading="lazy"
                  class="h-48 w-auto border border-white/15 sm:h-64"
                />
              </li>
            ))}
          </ul>
        </section>
      ) : null}

      {extension ? (
        <section class="mb-12">
          <h2 class="mb-3 border-b border-white/20 pb-2 text-base">native extension</h2>
          <ExtensionNote extension={extension} source={app.source} />
        </section>
      ) : null}

      <section class="mb-12 grid grid-cols-1 gap-8 md:grid-cols-2">
        <div>
          <h2 class="mb-3 border-b border-white/20 pb-2 text-base">install</h2>
          {unpublished ? (
            <p class="mb-4 text-white/65">this version is in the catalog but has no installation candidate</p>
          ) : (
            <>
              <p class="mb-4 text-white/65">install from the bridgething app or your browser</p>
              <button type="button" class="btn btn-primary mb-4" onClick={() => installListing(listing)}>
                install {newest!.version}
              </button>
            </>
          )}
          <Trust source={source} />
          {listing.alsoAvailableFrom.length > 0 ? (
            <p class="m-0 mt-2 font-mono text-sm text-white/35">
              also offered by {listing.alsoAvailableFrom.length} other source
              {listing.alsoAvailableFrom.length === 1 ? '' : 's'}. this is unusual
            </p>
          ) : null}
        </div>

        <div>
          <h2 class="mb-3 border-b border-white/20 pb-2 text-base">details</h2>
          <dl class="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 font-mono text-sm">
            <dt class="text-white/40">id</dt>
            <dd class="m-0 break-all">{app.id}</dd>
            {newest ? (
              <>
                <dt class="text-white/40">needs</dt>
                <dd class="m-0">libbridgething {newest.min_libbridgething_version}</dd>
                <dt class="text-white/40">permissions</dt>
                <dd class="m-0">{newest.permissions.length > 0 ? newest.permissions.join(', ') : 'none'}</dd>
                {newest.role === 'launcher' || newest.provides_overlay ? (
                  <>
                    <dt class="text-white/40">provides</dt>
                    <dd class="m-0">
                      {[
                        newest.role === 'launcher' ? 'home screen' : null,
                        newest.provides_overlay ? 'system overlay' : null,
                      ]
                        .filter(Boolean)
                        .join(', ')}
                      <span class="block text-white/40">
                        assign it on <a href="/device">device</a> after installing
                      </span>
                    </dd>
                  </>
                ) : null}
              </>
            ) : null}
            {app.homepage ? (
              <>
                <dt class="text-white/40">homepage</dt>
                <dd class="m-0 break-all">{homepage ? <a href={homepage}>{app.homepage}</a> : app.homepage}</dd>
              </>
            ) : null}
            {app.source ? (
              <>
                <dt class="text-white/40">source</dt>
                <dd class="m-0 break-all">{repo ? <a href={repo}>{app.source}</a> : app.source}</dd>
              </>
            ) : null}
          </dl>
        </div>
      </section>

      <section>
        <h2 class="mb-4 border-b border-white/20 pb-2 text-base">versions</h2>
        <ul class="m-0 flex list-none flex-col gap-4 p-0">
          {versions.map(v => (
            <li key={v.version} class="border border-white/15 p-5">
              <div class="mb-1 flex flex-wrap items-baseline gap-3">
                <h3 class="m-0 font-mono text-base/none font-medium">{v.version}</h3>
                <time class="font-mono text-sm text-white/45" datetime={v.released_at}>
                  {formatDate(v.released_at)}
                </time>
              </div>
              <p class="m-0 mb-2 font-mono text-sm text-white/45">
                needs lib {v.min_libbridgething_version}
                {v.download.size > 0 ? ` · ${(v.download.size / 1024).toFixed(0)} KB` : ' · size pending'}
                {v.download.sha256.replaceAll('0', '') === '' ? '' : ` · ${v.download.sha256.slice(0, 12)}`}
              </p>
              {v.changelog ? <p class="m-0 text-white/70">{v.changelog}</p> : null}
            </li>
          ))}
        </ul>
      </section>
    </>
  );
}

export function AppDetail({ baked }: { baked: BakedApp | null }) {
  const [id, setId] = useState<string | null | undefined>(baked?.app.id);
  const [catalogs, setCatalogs] = useState<MergedCatalog[] | null>(null);
  const [installs, setInstalls] = useState<InstallCount[]>([]);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    setId(appIdFromPath(window.location.pathname) ?? baked?.app.id ?? null);

    const controller = new AbortController();
    fetchMergedApps({ signal: controller.signal })
      .then(merged => {
        setCatalogs(orderedByTrust(merged.catalogs));
        setInstalls(merged.installs);
      })
      .catch(() => {
        if (!controller.signal.aborted) setFailed(true);
      });
    return () => controller.abort();
  }, []);

  const resolved = useMemo<Resolved | null>(() => {
    if (catalogs) {
      const listings = aggregate({
        orderedCatalogs: catalogs,
        installed: [],
        deviceLibVersion: null,
        installs,
        extensions: 'listed',
      });
      const listing = listings.find(entry => entry.app.id === id);
      if (listing) return { listing, source: sourceMap(catalogs).get(listing.sourceUrl) ?? null };
    }
    if (baked && baked.app.id === id) return { listing: bakedListing(baked), source: baked.source };
    return null;
  }, [catalogs, installs, id, baked]);

  if (resolved) return <Detail listing={resolved.listing} source={resolved.source} />;
  if (id === undefined || (catalogs === null && !failed)) return <Notice>looking this app up…</Notice>;
  if (id === null)
    return (
      <Notice>
        no app id in this url. pick one from the <a href="/apps">apps page</a>.
      </Notice>
    );
  if (failed) return <Notice>could not reach the catalog. try reloading the page.</Notice>;

  return <Notice>no source in the directory lists an app with this id.</Notice>;
}
