import {
  aggregate,
  compareVersions,
  describeExtensionPermissions,
  extensionOf,
  failureLine,
  installsLabel,
  normalizeSourceUrl,
  OFFICIAL_CATALOG_URL,
  recommendedSources,
  sortNewestFirst,
  STORE_COPY,
  versionCompatible,
  webHref,
  type AppExtension,
  type AppVersion,
  type Catalog,
  type CatalogAppListing,
  type CatalogSnapshot,
  type InstalledWebapp,
} from '@bridgething/catalog';
import {
  Button,
  describeError,
  Field,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionEmpty,
  SectionHeader,
  Spinner,
} from '@bridgething/ui';
import { openUrl } from '@tauri-apps/plugin-opener';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';
import { useState } from 'preact/hooks';

import { ConfirmInstall, InstallButton, useInstaller } from '../components/Install.tsx';
import { BackButton, Disclosure, ErrorNote, Hint, Screen, Section } from '../components/Screen.tsx';
import { useDesktop } from '../desktop.ts';
import { sideloadConsent } from '../lib/extension.ts';
import { bytes, day } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { humanizePermission } from '../lib/permissions.ts';
import { pickArtifact } from '../lib/picker.ts';
import { CatalogIcon } from '../lib/webapp-icon.tsx';
import { PATHS } from '../routes.ts';
import { catalogFor, deviceContext, mergedApps, storeListings } from '../stores/catalog.ts';
import { catalogSources } from '../stores/session.ts';

export function StoreRoute(): VNode {
  const { route } = useLocation();
  const { vouched, community, subscribed, snapshot, pending, error, refresh } = storeListings();
  const [query, setQuery] = useState('');

  const searching = query.trim().length > 0;
  const matches = (listing: CatalogAppListing) => matchesQuery(listing, query);
  const apps = vouched.filter(matches);
  const others = community.filter(matches);
  const updates = [...vouched, ...community].filter(listing => listing.updateAvailable);
  const open = (listing: CatalogAppListing) => route(PATHS.storeApp(listing.app.id));

  return (
    <Screen>
      <ScreenHeader
        title="store"
        trailing={
          <Field
            value={query}
            onInput={setQuery}
            icon={<Icon name="search" />}
            placeholder="search"
            clearable
            class="w-60"
          />
        }
      />

      {updates.length > 0 && !searching ? (
        <Section>
          <SectionHeader title="updates" />
          <ListGroup>
            {updates.map(listing => (
              <UpdateRow key={listing.app.id} listing={listing} />
            ))}
          </ListGroup>
        </Section>
      ) : null}

      {searching && apps.length === 0 ? (
        others.length === 0 ? (
          <SectionEmpty class="mb-9">no matches</SectionEmpty>
        ) : null
      ) : (
        <Section>
          <SectionHeader title={STORE_COPY.appsTitle} action="refresh" pending={pending} onAction={refresh} />
          {pending && vouched.length === 0 ? (
            <SectionEmpty>
              <Spinner class="mx-auto" />
            </SectionEmpty>
          ) : apps.length === 0 ? (
            <SectionEmpty>{STORE_COPY.appsEmpty}</SectionEmpty>
          ) : (
            <CardGrid listings={apps} onOpen={open} />
          )}
          {snapshot.failures.length > 0 ? <Hint>{failureLine(snapshot.failures.length)}</Hint> : null}
          {error ? <ErrorNote>{error}</ErrorNote> : null}
        </Section>
      )}

      {others.length > 0 ? (
        <Section>
          <SectionHeader title={STORE_COPY.communityTitle} hint={STORE_COPY.communityNote} />
          <CardGrid listings={others} onOpen={open} />
        </Section>
      ) : null}

      <Disclosure title="sources">
        <MySources subscribed={subscribed} pending={catalogSources.pending.value} />
        <SuggestedSources snapshot={snapshot} subscribed={subscribed} />
        <BrowseByUrl />
      </Disclosure>

      <Disclosure title="install from a file">
        <SideloadBundle />
      </Disclosure>
    </Screen>
  );
}

function matchesQuery(listing: CatalogAppListing, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) return true;
  const { name, description, author } = listing.app;
  return [name, description, author].some(field => field.toLowerCase().includes(needle));
}

function CardGrid({
  listings,
  onOpen,
}: {
  listings: CatalogAppListing[];
  onOpen: (listing: CatalogAppListing) => void;
}): VNode {
  return (
    <div class="grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-3">
      {listings.map(listing => (
        <StoreCard key={`${listing.sourceUrl}:${listing.app.id}`} listing={listing} onOpen={() => onOpen(listing)} />
      ))}
    </div>
  );
}

function StoreCard({ listing, onOpen }: { listing: CatalogAppListing; onOpen: () => void }): VNode {
  const installer = useInstaller(listing);
  const { app, newestCompatible } = listing;
  const shot = webHref(app.screenshots?.[0]);
  const traits = [
    newestCompatible?.role === 'launcher' ? 'home screen' : null,
    newestCompatible?.provides_overlay ? 'overlay' : null,
    extensionOf(newestCompatible) ? 'extension' : null,
  ].filter(Boolean);
  const byline = [app.author, installsLabel(listing.installs)].filter(Boolean).join(' · ');

  return (
    <article class="flex min-w-0 flex-col border border-rule bg-screen">
      <button
        type="button"
        class="flex min-w-0 flex-1 flex-col text-left transition-colors duration-150 hover:bg-neutral-soft focus-visible:outline-2 focus-visible:outline-accent focus-visible:-outline-offset-2"
        onClick={onOpen}>
        {shot ? (
          <img
            src={shot}
            alt=""
            loading="lazy"
            class="aspect-5/3 w-full border-b border-rule bg-bg object-cover"
            onError={event => {
              (event.currentTarget as HTMLImageElement).style.display = 'none';
            }}
          />
        ) : (
          <div class="flex aspect-5/3 w-full items-center justify-center border-b border-rule bg-bg">
            <CatalogIcon url={app.icon} name={app.name} size="lg" />
          </div>
        )}
        <div class="flex min-w-0 flex-1 flex-col gap-1.5 px-4 pt-3 pb-2">
          <div class="flex min-w-0 items-center gap-2.5">
            <CatalogIcon url={app.icon} name={app.name} size="sm" />
            <div class="flex min-w-0 flex-col">
              <span class="truncate text-row text-off-white">{app.name}</span>
              <span class="truncate text-hint text-muted">{byline}</span>
            </div>
          </div>
          <p class="m-0 line-clamp-2 text-hint leading-relaxed text-soft">{app.description}</p>
        </div>
      </button>
      <div class="flex items-center justify-between gap-2 px-4 pb-3">
        <span class="truncate font-mono text-eyebrow text-dim uppercase">{traits.join(' · ')}</span>
        <InstallButton listing={listing} installer={installer} size="sm" />
      </div>
      {installer.failure ? (
        <p class="m-0 border-t border-rule px-4 py-2 text-hint text-err">{installer.failure}</p>
      ) : null}
      {installer.dialog}
    </article>
  );
}

function UpdateRow({ listing }: { listing: CatalogAppListing }): VNode {
  const installer = useInstaller(listing);
  const { app, newestCompatible, installedVersion } = listing;

  return (
    <>
      <ListRow
        icon={<CatalogIcon url={app.icon} name={app.name} size="sm" />}
        title={app.name}
        subtitle={installer.failure ?? newestCompatible?.changelog ?? undefined}
        value={`${installedVersion} → ${newestCompatible?.version}`}
        trailing={<InstallButton listing={listing} installer={installer} size="sm" />}
      />
      {installer.dialog}
    </>
  );
}

function SuggestedSources({ snapshot, subscribed }: { snapshot: CatalogSnapshot; subscribed: string[] }): VNode {
  const { route } = useLocation();
  const suggested = recommendedSources({
    directory: snapshot.directory,
    orderedCatalogs: snapshot.catalogs,
    subscribed,
  });

  return (
    <Section>
      <SectionHeader title={STORE_COPY.suggestedTitle} />
      {suggested.length === 0 ? (
        <SectionEmpty>{STORE_COPY.suggestedEmpty}</SectionEmpty>
      ) : (
        <ListGroup>
          {suggested.map(source => (
            <ListRow
              key={source.url}
              icon={<Icon name="store" />}
              title={source.name}
              subtitle={source.description ?? source.url}
              trailing={source.attested ? <Pill tone="ok">attested</Pill> : undefined}
              chevron
              onClick={() => route(PATHS.storeSource(source.url))}
            />
          ))}
        </ListGroup>
      )}
    </Section>
  );
}

function MySources({ subscribed, pending }: { subscribed: string[]; pending: boolean }): VNode {
  const { route } = useLocation();

  return (
    <Section>
      <SectionHeader title="my sources" />
      <ListGroup>
        {subscribed.map(url => {
          const official = url === OFFICIAL_CATALOG_URL;
          return (
            <ListRow
              key={url}
              icon={<Icon name="store" />}
              iconTint={official ? 'accent' : 'default'}
              title={official ? 'bridgething' : url}
              subtitle={official ? url : undefined}
              chevron
              disabled={pending}
              onClick={() => route(PATHS.storeSource(url))}
            />
          );
        })}
      </ListGroup>
    </Section>
  );
}

export function SourceRoute({ source }: { source: string }): VNode {
  const url = decodeURIComponent(source);
  const { route } = useLocation();
  const { installed, libVersion } = deviceContext();
  const catalog = catalogFor(url);
  const held = catalog.data.value;
  const pending = catalog.pending.value;
  const listings = listingsFor(url, held, installed, libVersion);

  return (
    <Screen>
      <BackButton to={PATHS.store}>store</BackButton>
      <ScreenHeader
        title={held?.repo.name ?? url}
        subtitle={held?.repo.description ?? undefined}
        trailing={<SubscribeToggle url={url} />}
      />

      <Section>
        <SectionHeader title="apps" action="refresh" pending={pending} onAction={catalog.refresh} />
        {catalog.error.value ? (
          <ErrorNote>{catalog.error.value}</ErrorNote>
        ) : pending && listings.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : listings.length === 0 ? (
          <SectionEmpty>no apps</SectionEmpty>
        ) : (
          <CardGrid listings={listings} onOpen={listing => route(PATHS.storeSourceApp(url, listing.app.id))} />
        )}
      </Section>
    </Screen>
  );
}

export function SourceAppRoute({ source, appId }: { source: string; appId: string }): VNode {
  const url = decodeURIComponent(source);
  const id = decodeURIComponent(appId);
  const { installed, libVersion } = deviceContext();
  const catalog = catalogFor(url);
  const listings = listingsFor(url, catalog.data.value, installed, libVersion);

  return <AppScreen listing={listings.find(entry => entry.app.id === id) ?? null} loading={catalog.pending.value} />;
}

export function CatalogAppRoute({ appId }: { appId: string }): VNode {
  const id = decodeURIComponent(appId);
  const { vouched, community, pending } = storeListings();
  const listings = [...vouched, ...community];

  return <AppScreen listing={listings.find(entry => entry.app.id === id) ?? null} loading={pending} />;
}

function listingsFor(
  url: string,
  catalog: Catalog | null,
  installed: InstalledWebapp[],
  libVersion: string | null,
): CatalogAppListing[] {
  if (!catalog) return [];
  return aggregate({
    orderedCatalogs: [{ url, catalog }],
    installed,
    deviceLibVersion: libVersion,
    installs: mergedApps().data.value.installs,
    extensions: 'listed',
  });
}

function SubscribeToggle({ url }: { url: string }): VNode {
  const session = useDesktop();
  const [busy, setBusy] = useState(false);

  const held = catalogSources.data.value.includes(url);

  const toggle = async () => {
    setBusy(true);
    try {
      if (held) await session.removeCatalogSource(url);
      else await session.addCatalogSource(url);
      await catalogSources.refresh();
    } finally {
      setBusy(false);
    }
  };

  return (
    <Button
      size="sm"
      variant={held ? 'secondary' : 'primary'}
      icon={<Icon name={held ? 'trash' : 'plus'} size={14} />}
      loading={busy || catalogSources.pending.value}
      onClick={() => void toggle()}>
      {held ? 'remove source' : 'add source'}
    </Button>
  );
}

function AppScreen({ listing, loading }: { listing: CatalogAppListing | null; loading: boolean }): VNode {
  if (!listing) {
    return (
      <Screen>
        <BackButton>back</BackButton>
        <SectionEmpty>{loading ? <Spinner class="mx-auto" /> : 'no longer listed'}</SectionEmpty>
      </Screen>
    );
  }

  return <AppDetail key={`${listing.sourceUrl}:${listing.app.id}`} listing={listing} />;
}

function AppDetail({ listing }: { listing: CatalogAppListing }): VNode {
  const installer = useInstaller(listing);
  const [allVersions, setAllVersions] = useState(false);
  const { libVersion } = deviceContext();

  const { app, sourceUrl, newestCompatible, installedVersion } = listing;
  const extension = extensionOf(newestCompatible);
  const ordered = sortNewestFirst(app.versions);
  const newest = newestCompatible ?? ordered[0] ?? null;
  const shown = allVersions ? ordered : ordered.slice(0, 3);
  const shots = (app.screenshots ?? []).map(webHref).filter((url): url is string => url !== null);
  const homepage = webHref(app.homepage);
  const code = webHref(app.source);
  const facts = [
    app.author,
    newest ? `v${newest.version}` : null,
    newest ? day(newest.released_at) : null,
    newest && newest.download.size > 0 ? bytes(newest.download.size) : null,
    installsLabel(listing.installs),
  ].filter(Boolean);

  return (
    <Screen>
      <BackButton>back</BackButton>

      <div class="mb-6 flex items-center gap-4">
        <CatalogIcon url={app.icon} name={app.name} size="lg" />
        <div class="flex min-w-0 flex-1 flex-col gap-1.5">
          <h1 class="m-0 font-display text-hero font-medium tracking-display wrap-break-word">{app.name}</h1>
          <span class="font-mono text-hint wrap-break-word text-muted">{facts.join(' · ')}</span>
          <div class="flex flex-wrap items-center gap-1.5">
            {installedVersion ? <Pill tone="ok">{`installed v${installedVersion}`}</Pill> : null}
            {newestCompatible?.role === 'launcher' ? <Pill tone="neutral">home screen</Pill> : null}
            {newestCompatible?.provides_overlay ? <Pill tone="neutral">overlay</Pill> : null}
            {extension ? <Pill tone="accent">native extension</Pill> : null}
          </div>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          {homepage ? (
            <Button
              variant="ghost"
              size="sm"
              icon={<Icon name="globe" size={13} />}
              onClick={() => void openUrl(homepage)}>
              homepage
            </Button>
          ) : null}
          {code && code !== homepage ? (
            <Button variant="ghost" size="sm" icon={<Icon name="file" size={13} />} onClick={() => void openUrl(code)}>
              code
            </Button>
          ) : null}
          <InstallButton listing={listing} installer={installer} />
        </div>
      </div>

      {installer.failure ? <ErrorNote>{installer.failure}</ErrorNote> : null}

      {shots.length > 0 ? (
        <div class="mb-6 flex gap-3 overflow-x-auto pb-1">
          {shots.map(shot => (
            <img
              key={shot}
              src={shot}
              alt=""
              loading="lazy"
              class="aspect-5/3 h-56 shrink-0 border border-rule bg-bg object-cover"
            />
          ))}
        </div>
      ) : null}

      <p class="mb-8 text-body leading-relaxed text-off-white">{app.description}</p>

      {newest?.changelog ? (
        <Section>
          <SectionHeader title="what's new" />
          <p class="m-0 text-body leading-relaxed whitespace-pre-wrap text-soft">{newest.changelog}</p>
        </Section>
      ) : null}

      {newestCompatible && newestCompatible.permissions.length > 0 ? (
        <Section>
          <SectionHeader title="permissions" />
          <ListGroup>
            {newestCompatible.permissions.map(permission => {
              const copy = humanizePermission(permission);
              return <ListRow key={permission} icon={<Icon name={copy.icon} />} title={copy.title} />;
            })}
          </ListGroup>
        </Section>
      ) : null}

      {extension ? (
        <Section>
          <SectionHeader title="native extension" hint="runs on this computer" />
          <ListGroup>
            {describeExtensionPermissions(extension).map(grant => (
              <ListRow key={grant} icon={<Icon name="shield" />} iconTint="accent" title={grant} />
            ))}
          </ListGroup>
        </Section>
      ) : null}

      <Section>
        <SectionHeader
          title="versions"
          action={app.versions.length > 3 ? (allVersions ? 'fewer' : `all ${app.versions.length}`) : undefined}
          onAction={() => setAllVersions(open => !open)}
        />
        <ListGroup>
          {shown.map(version => (
            <VersionRow
              key={version.version}
              version={version}
              installedVersion={installedVersion}
              libVersion={libVersion}
              installing={installer.waitingVersion}
              busy={installer.busy}
              onInstall={() => installer.start(version)}
            />
          ))}
        </ListGroup>
        <Hint>{sourceUrl}</Hint>
      </Section>

      {installer.dialog}
    </Screen>
  );
}

function VersionRow({
  version,
  installedVersion,
  libVersion,
  installing,
  busy,
  onInstall,
}: {
  version: AppVersion;
  installedVersion: string | null;
  libVersion: string | null;
  installing: string | null;
  busy: boolean;
  onInstall: () => void;
}): VNode {
  const here = version.version === installedVersion;
  const older = installedVersion !== null && compareVersions(version.version, installedVersion) < 0;

  return (
    <ListRow
      title={`v${version.version}`}
      subtitle={version.changelog ?? undefined}
      value={day(version.released_at)}
      trailing={
        here ? (
          <Pill tone="ok">installed</Pill>
        ) : !versionCompatible(version, libVersion) ? (
          <Pill tone="warn">{STORE_COPY.needsFirmware}</Pill>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            icon={<Icon name="download" size={13} />}
            loading={installing === version.version}
            disabled={busy}
            onClick={onInstall}>
            {older ? 'roll back' : 'install'}
          </Button>
        )
      }
    />
  );
}

function BrowseByUrl(): VNode {
  const { route } = useLocation();
  const [url, setUrl] = useState('');
  const [failure, setFailure] = useState<string | null>(null);

  const open = (value: string) => {
    try {
      const normalized = normalizeSourceUrl(value);
      setFailure(null);
      route(PATHS.storeSource(normalized));
    } catch (reason) {
      setFailure(describeError(reason));
    }
  };

  return (
    <Section>
      <SectionHeader title="add by url" />
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          value={url}
          onInput={setUrl}
          onSubmit={open}
          icon={<Icon name="link" />}
          type="url"
          placeholder="https://example.com/catalog.json"
          clearable
        />
        <Button disabled={url.trim().length === 0} onClick={() => open(url)}>
          browse
        </Button>
      </div>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}

function SideloadBundle(): VNode {
  const session = useDesktop();
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState<{ bundle: string; extension: AppExtension } | null>(null);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const install = async (bundle: string, confirmed?: string[]) => {
    setBusy(true);
    setConfirming(null);
    setOutcome(null);
    setFailure(null);
    try {
      const answer = await session.otaInstallWebapp(bundle, undefined, confirmed);
      if (answer.kind === 'installed') setOutcome('installed');
      else setFailure(answer.reason);
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(false);
    }
  };

  const start = async (bundle: string) => {
    setBusy(true);
    setOutcome(null);
    setFailure(null);
    let extension: AppExtension | null;
    try {
      extension = sideloadConsent(await session.webappBundleExtension(bundle));
    } catch (reason) {
      setFailure(describeError(reason));
      setBusy(false);
      return;
    }
    setBusy(false);
    if (extension) setConfirming({ bundle, extension });
    else await install(bundle);
  };

  const browse = async () => {
    const picked = await pickArtifact('webapp');
    if (!picked) return;
    setPath(picked);
    await start(picked);
  };

  return (
    <Section>
      <SectionHeader title="webapp zip" />
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          value={path}
          onInput={setPath}
          onSubmit={value => value.trim() && void start(value.trim())}
          icon={<Icon name="file" />}
          placeholder="/path/to/my-webapp.zip"
          clearable
        />
        <Button icon={<Icon name="upload" />} loading={busy} onClick={() => void browse()}>
          choose
        </Button>
      </div>
      {outcome ? <Hint>{outcome}</Hint> : null}
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      <ConfirmInstall
        open={confirming !== null}
        title="install this bundle?"
        extension={confirming?.extension ?? null}
        source={null}
        older={false}
        busy={busy}
        onCancel={() => setConfirming(null)}
        onConfirm={() => {
          if (confirming) void install(confirming.bundle, confirming.extension.permissions);
        }}
      />
    </Section>
  );
}
