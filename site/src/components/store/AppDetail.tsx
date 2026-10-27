import type { ComponentChildren } from 'preact';
import { useMemo, useState } from 'preact/hooks';
import {
  alsoAvailableLabel,
  appCategories,
  changelogText,
  extensionOf,
  listingKindLabel,
  moreFromAuthor,
  newestCompatible,
  OFFICIAL_CATALOG_URL,
  permissionLabel,
  sortNewestFirst,
  sourceLine,
  type AppEntry,
  type AppVersion,
  type CatalogAppListing,
  type StoreSource,
  webHref,
} from '@bridgething/catalog';
import { filterPath, storeRoute, STORE_PATH } from '../../lib/app-routes';
import type { StoreData } from '../../lib/directory-client';
import { installListing } from '../../lib/pending-install';
import { currentHref, useStore } from '../../lib/use-store';
import { ExtensionBadge, ExtensionNote } from './ExtensionNote';
import { AppFrame, AppIcon, IconTile, installable, Notice, Row, screenshots, ShelfHeader } from './parts';

export type BakedApp = { app: AppEntry; source: StoreSource };

type Resolved = { listing: CatalogAppListing; source: StoreSource | null; more: CatalogAppListing[] };

function formatDate(raw: string): string {
  const at = Date.parse(raw);
  if (Number.isNaN(at)) return raw;
  return new Date(at).toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric', timeZone: 'UTC' });
}

function formatSize(bytes: number): string {
  if (bytes <= 0) return 'pending';
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function bakedListing(app: AppEntry, sourceUrl: string): CatalogAppListing {
  return {
    app,
    sourceUrl,
    newestCompatible: newestCompatible(app, null),
    installedVersion: null,
    updateAvailable: false,
    alsoAvailableFrom: [],
    installs: 0,
    trending: 0,
    official: sourceUrl === OFFICIAL_CATALOG_URL,
  };
}

function Section({
  title,
  aside,
  children,
}: {
  title: string;
  aside?: ComponentChildren;
  children: ComponentChildren;
}) {
  return (
    <section class="mb-10">
      <header class="mb-3 flex items-baseline justify-between gap-3 border-b border-white/20 pb-2">
        <h2 class="m-0 text-base">{title}</h2>
        {aside ? <span class="font-mono text-sm text-white/40">{aside}</span> : null}
      </header>
      {children}
    </section>
  );
}

function Stat({ value, label, href }: { value: string; label: string; href?: string | null }) {
  return (
    <div class="min-w-0 px-4 py-3">
      <p class="m-0 truncate font-medium">
        {href ? (
          <a href={href} class="border-b-0 hover:text-white">
            {value}
          </a>
        ) : (
          value
        )}
      </p>
      <p class="m-0 font-mono text-xs text-white/40">{label}</p>
    </div>
  );
}

const RECENT_VERSIONS = 5;

function Versions({ versions, newest }: { versions: AppVersion[]; newest: AppVersion | null }) {
  return (
    <ul class="m-0 flex list-none flex-col p-0 [&>li+li]:border-t [&>li+li]:border-white/10">
      {versions.map(v => {
        const notes = v === newest ? null : changelogText(v.changelog);
        return (
          <li key={v.version} class="py-3">
            <p class="m-0 flex flex-wrap items-baseline gap-x-3 font-mono text-sm">
              <span class="text-white/85">{v.version}</span>
              <time class="text-white/45" datetime={v.released_at}>
                {formatDate(v.released_at)}
              </time>
              <span class="text-white/35">
                lib {v.min_libbridgething_version} · {formatSize(v.download.size)}
              </span>
            </p>
            {notes ? (
              <p class="m-0 mt-1 line-clamp-3 max-w-[90ch] text-sm whitespace-pre-line text-white/55">{notes}</p>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

function Detail({ listing, source, more }: Resolved) {
  const app = listing.app;
  const versions = sortNewestFirst(app.versions);
  const newest = listing.newestCompatible ?? versions[0] ?? null;
  const extension = extensionOf(newest);
  const homepage = webHref(app.homepage);
  const repo = webHref(app.source);
  const shots = screenshots(app);
  const kind = listingKindLabel(listing);
  const category = appCategories(app)[0];
  const kindHref =
    newest?.role === 'launcher'
      ? filterPath({ kind: 'launchers' })
      : newest?.provides_overlay
        ? filterPath({ kind: 'overlays' })
        : category
          ? filterPath({ kind: 'category', category })
          : null;
  const permissions = [...new Set((newest?.permissions ?? []).map(permissionLabel))];
  const provides = [
    newest?.role === 'launcher' ? 'home screen' : null,
    newest?.provides_overlay ? 'system overlay' : null,
  ].filter((entry): entry is string => entry !== null);
  const author = filterPath({ kind: 'author', author: app.author });
  const also = alsoAvailableLabel(listing.alsoAvailableFrom);

  return (
    <>
      <header class="mb-6 flex flex-wrap items-center gap-5">
        <AppIcon app={app} size={88} />
        <div class="min-w-0 flex-1">
          <h1 class="m-0 break-words">{app.name}</h1>
          <p class="m-0 mt-1 font-mono text-sm text-white/50">
            <a href={author}>{app.author}</a>
          </p>
          {extension ? (
            <div class="mt-3 flex">
              <ExtensionBadge />
            </div>
          ) : null}
        </div>
        <div class="flex w-full flex-col gap-2 sm:w-auto sm:items-end">
          {installable(listing) ? (
            <button type="button" class="btn btn-primary btn-lg justify-center" onClick={() => installListing(listing)}>
              install {newest!.version}
            </button>
          ) : (
            <p class="text-warn m-0 font-mono text-sm">this version has no installable download yet</p>
          )}
          <p class="m-0 font-mono text-xs text-white/40">from the bridgething app or this browser</p>
        </div>
      </header>

      <div class="bg-screen mb-8 grid grid-cols-3 divide-x divide-white/10 border border-white/15">
        <Stat value={String(listing.installs)} label={listing.installs === 1 ? 'install' : 'installs'} />
        <Stat value={kind ?? 'app'} label="category" href={kindHref} />
        <Stat value={newest ? formatSize(newest.download.size) : 'pending'} label="size" />
      </div>

      {shots.length > 0 ? (
        <div class="mb-10">
          <Row>
            {shots.map((shot, index) => (
              <div key={shot} class="w-72 shrink-0 snap-start sm:w-96 lg:w-[calc((100%-3rem)/3)]">
                <AppFrame app={app} shot={shot} size="md" eager={index === 0} />
              </div>
            ))}
          </Row>
        </div>
      ) : null}

      <div class="grid grid-cols-1 gap-x-12 lg:grid-cols-[3fr_2fr]">
        <div class="min-w-0">
          <p class="m-0 mb-10 max-w-[65ch] text-lg text-white/75">{app.description}</p>

          {extension ? (
            <Section title="native extension">
              <ExtensionNote extension={extension} source={app.source} />
            </Section>
          ) : null}

          {newest ? (
            <Section
              title={`new in ${newest.version}`}
              aside={<time datetime={newest.released_at}>{formatDate(newest.released_at)}</time>}>
              <p class="m-0 max-w-[65ch] whitespace-pre-line text-white/60">
                {changelogText(newest.changelog) ?? 'no release notes'}
              </p>
            </Section>
          ) : null}
        </div>

        <div class="min-w-0">
          <Section title="permissions">
            {permissions.length > 0 ? (
              <ul class="bg-screen m-0 list-none border border-white/15 p-0 [&>li+li]:border-t [&>li+li]:border-white/10">
                {permissions.map(permission => (
                  <li key={permission} class="px-4 py-2.5">
                    {permission}
                  </li>
                ))}
              </ul>
            ) : (
              <p class="m-0 text-white/50">no permissions</p>
            )}
          </Section>

          {provides.length > 0 ? (
            <Section title="provides">
              <p class="m-0 text-white/75">{provides.join(', ')}</p>
              <p class="m-0 mt-1 font-mono text-sm text-white/40">
                assign it on <a href="/device">device</a> after installing
              </p>
            </Section>
          ) : null}

          <Section title="details">
            <dl class="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 font-mono text-sm">
              <dt class="text-white/40">id</dt>
              <dd class="m-0 break-all">{app.id}</dd>
              {newest ? (
                <>
                  <dt class="text-white/40">needs</dt>
                  <dd class="m-0">libbridgething {newest.min_libbridgething_version}</dd>
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
          </Section>
        </div>
      </div>

      <Section title="versions" aside={versions.length}>
        <Versions versions={versions.slice(0, RECENT_VERSIONS)} newest={newest} />
        {versions.length > RECENT_VERSIONS ? (
          <details class="mt-2 border-t border-white/10 pt-3">
            <summary class="cursor-pointer font-mono text-sm text-white/50 hover:text-white">
              {versions.length - RECENT_VERSIONS} older
            </summary>
            <Versions versions={versions.slice(RECENT_VERSIONS)} newest={newest} />
          </details>
        ) : null}
      </Section>
      {more.length > 0 ? (
        <section class="mb-10">
          <ShelfHeader title={`more from ${app.author}`} href={author} />
          <Row>
            {more.map(other => (
              <IconTile key={other.app.id} listing={other} />
            ))}
          </Row>
        </section>
      ) : null}

      <footer class="flex flex-col gap-1 border-t border-white/15 pt-4 font-mono text-xs text-white/40">
        <p class="m-0 break-all" title={source?.url}>
          {sourceLine(source)}
        </p>
        {also ? <p class="m-0">{also}</p> : null}
      </footer>
    </>
  );
}

function idOf(href: string | null, baked: BakedApp | null): string | null | undefined {
  if (href === null) return baked?.app.id;
  const url = new URL(href, 'https://bridgething.com');
  const route = storeRoute(url.pathname);
  return route?.page === 'app' ? route.id : (baked?.app.id ?? null);
}

export function AppDetail({ baked, data, href }: { baked: BakedApp | null; data?: StoreData | null; href?: string }) {
  const { catalog, failed, seeded } = useStore(data);
  const [id] = useState(() => idOf(href ?? currentHref(), baked));

  const resolved = useMemo<Resolved | null>(() => {
    if (catalog) {
      const listing = catalog.listings.find(entry => entry.app.id === id);
      if (listing) {
        return {
          listing,
          source: catalog.sources[listing.sourceUrl] ?? null,
          more: moreFromAuthor(catalog.listings, listing),
        };
      }
    }
    if (baked && baked.app.id === id && !seeded) {
      return { listing: bakedListing(baked.app, baked.source.url), source: baked.source, more: [] };
    }
    return null;
  }, [catalog, id, baked, seeded]);

  if (resolved) return <Detail {...resolved} />;
  if (id === undefined || (catalog === null && !failed)) return <Notice>looking this app up…</Notice>;
  if (id === null) {
    return (
      <Notice>
        no app id in this url. pick one from the <a href={STORE_PATH}>apps page</a>.
      </Notice>
    );
  }
  if (failed) return <Notice>could not reach the catalog. try reloading the page.</Notice>;
  return <Notice>no source in the directory lists an app with this id.</Notice>;
}
