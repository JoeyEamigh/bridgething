import {
  aggregate,
  blendStoreListings,
  compareVersions,
  describeExtensionPermissions,
  extensionOf,
  extensionRepoLabel,
  normalizeSourceUrl,
  OFFICIAL_CATALOG_URL,
  recommendedSources,
  reportInstall,
  sortNewestFirst,
  versionCompatible,
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
  Dialog,
  Field,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionEmpty,
  SectionHeader,
  Spinner,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';
import { useState } from 'preact/hooks';

import { BackButton, ErrorNote, Hint, Screen, Section } from '../components/Screen.tsx';
import { useDesktop } from '../desktop.ts';
import { toInstalled } from '../lib/catalog.ts';
import { sideloadConsent } from '../lib/extension.ts';
import { bytes, day } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { humanizePermission } from '../lib/permissions.ts';
import { pickArtifact } from '../lib/picker.ts';
import { CatalogIcon } from '../lib/webapp-icon.tsx';
import { PATHS } from '../routes.ts';
import { catalogFor, catalogsFor, mergedApps } from '../stores/catalog.ts';
import { catalogSources, selectedMeta, webapps } from '../stores/session.ts';

function deviceContext(): { installed: InstalledWebapp[]; libVersion: string | null } {
  return { installed: toInstalled(webapps.data.value), libVersion: selectedMeta.value?.libbridgethingVersion ?? null };
}

type Blend = {
  vouched: CatalogAppListing[];
  community: CatalogAppListing[];
  subscribed: string[];
  snapshot: CatalogSnapshot;
  pending: boolean;
  error: string | undefined;
  refresh: () => void;
};

function blend(): Blend {
  const { installed, libVersion } = deviceContext();
  const subscribed = subscribedUrls(catalogSources.data.value);
  const catalogs = catalogsFor(subscribed);
  const directory = mergedApps();
  const snapshot = catalogs.data.value;
  const merged = directory.data.value;

  const { vouched, community } = blendStoreListings({
    catalogs: snapshot.catalogs,
    merged: merged.catalogs,
    installed,
    deviceLibVersion: libVersion,
    installs: merged.installs,
    subscribed,
    extensions: 'listed',
  });

  return {
    vouched,
    community,
    subscribed,
    snapshot,
    pending: catalogs.pending.value || directory.pending.value,
    error: catalogs.error.value,
    refresh: () => {
      void catalogs.refresh();
      void directory.refresh();
    },
  };
}

export function StoreRoute(): VNode {
  const { route } = useLocation();
  const { vouched, community, subscribed, snapshot, pending, error, refresh } = blend();

  const suggested = recommendedSources({
    directory: snapshot.directory,
    orderedCatalogs: snapshot.catalogs,
    subscribed,
  });

  return (
    <Screen>
      <ScreenHeader
        eyebrow="catalog"
        title="store"
        subtitle="webapps published by catalog sources. nothing here is reviewed."
      />

      <Section>
        <SectionHeader
          title="available apps"
          hint={`your ${subscribed.length} source${subscribed.length === 1 ? '' : 's'} and the bridgething directory`}
          action="refresh"
          pending={pending}
          onAction={refresh}
        />
        {pending && vouched.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : vouched.length === 0 ? (
          <SectionEmpty>no apps available from your sources yet</SectionEmpty>
        ) : (
          <ListGroup>
            {vouched.map(listing => (
              <ListingRow key={listing.app.id} listing={listing} onOpen={() => route(PATHS.storeApp(listing.app.id))} />
            ))}
          </ListGroup>
        )}
        {snapshot.failures.length > 0 ? (
          <Hint>
            {snapshot.failures.length} source{snapshot.failures.length === 1 ? '' : 's'} could not be read. anything
            already installed from them keeps working.
          </Hint>
        ) : null}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
      </Section>

      {community.length > 0 ? (
        <Section>
          <SectionHeader title="community" hint="from directory sources you have not added" />
          <ListGroup>
            {community.map(listing => (
              <ListingRow key={listing.app.id} listing={listing} onOpen={() => route(PATHS.storeApp(listing.app.id))} />
            ))}
          </ListGroup>
          <Hint>installing one of these adds its source to yours. listed, never reviewed.</Hint>
        </Section>
      ) : null}

      <MySources subscribed={subscribed} pending={catalogSources.pending.value} />

      <Section>
        <SectionHeader title="suggested sources" hint="listed in the bridgething directory" />
        {suggested.length === 0 ? (
          <SectionEmpty>the directory listed nothing you are not already subscribed to</SectionEmpty>
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
        <Hint>a listing means someone checked it is a real catalog, never that its apps are safe.</Hint>
      </Section>

      <BrowseByUrl />
      <SideloadBundle />
    </Screen>
  );
}

function subscribedUrls(held: string[] | undefined): string[] {
  return [OFFICIAL_CATALOG_URL, ...(held ?? []).filter(url => url !== OFFICIAL_CATALOG_URL)];
}

function MySources({ subscribed, pending }: { subscribed: string[]; pending: boolean }): VNode {
  const session = useDesktop();
  const { route } = useLocation();
  const [busy, setBusy] = useState<string | null>(null);

  const remove = async (url: string) => {
    setBusy(url);
    try {
      await session.removeCatalogSource(url);
      await catalogSources.refresh();
    } finally {
      setBusy(null);
    }
  };

  return (
    <Section>
      <SectionHeader title="my sources" hint="tap one to browse it" />
      <ListGroup>
        {subscribed.map(url => {
          const official = url === OFFICIAL_CATALOG_URL;
          return (
            <ListRow
              key={url}
              icon={<Icon name="store" />}
              iconTint={official ? 'accent' : 'default'}
              title={official ? 'the bridgething catalog' : url}
              subtitle={official ? url : undefined}
              trailing={
                official ? (
                  <Pill tone="neutral">built in</Pill>
                ) : busy === url ? (
                  <Spinner />
                ) : (
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<Icon name="trash" size={13} />}
                    onClick={() => void remove(url)}>
                    remove
                  </Button>
                )
              }
              disabled={pending}
              onClick={() => route(PATHS.storeSource(url))}
            />
          );
        })}
      </ListGroup>
      <Hint>removing a source hides its apps here. anything already installed from it keeps working.</Hint>
    </Section>
  );
}

function ListingRow({ listing, onOpen }: { listing: CatalogAppListing; onOpen: () => void }): VNode {
  const { app, newestCompatible, installedVersion, updateAvailable } = listing;

  return (
    <ListRow
      icon={<CatalogIcon url={app.icon} name={app.name} size="sm" />}
      title={app.name}
      subtitle={app.description}
      value={newestCompatible ? `v${newestCompatible.version}` : undefined}
      trailing={
        <span class="flex shrink-0 items-center gap-1.5">
          {extensionOf(newestCompatible) ? <Pill tone="accent">extension</Pill> : null}
          {updateAvailable ? (
            <Pill tone="accent">update</Pill>
          ) : installedVersion ? (
            <Pill tone="ok">installed</Pill>
          ) : !newestCompatible ? (
            <Pill tone="warn">needs newer firmware</Pill>
          ) : null}
        </span>
      }
      chevron
      onClick={onOpen}
    />
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
      <BackButton>store</BackButton>
      <ScreenHeader
        eyebrow="source"
        title={held?.repo.name ?? 'catalog source'}
        subtitle={held?.repo.description ?? url}
        trailing={<SubscribeToggle url={url} />}
      />

      <Section>
        <SectionHeader title="apps" hint={url} action="refresh" pending={pending} onAction={catalog.refresh} />
        {catalog.error.value ? (
          <ErrorNote>{catalog.error.value}</ErrorNote>
        ) : pending && listings.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : listings.length === 0 ? (
          <SectionEmpty>this source publishes no apps</SectionEmpty>
        ) : (
          <ListGroup>
            {listings.map(listing => (
              <ListingRow
                key={listing.app.id}
                listing={listing}
                onOpen={() => route(PATHS.storeSourceApp(url, listing.app.id))}
              />
            ))}
          </ListGroup>
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
  const { vouched, community, pending } = blend();
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

  const official = url === OFFICIAL_CATALOG_URL;
  const held = subscribedUrls(catalogSources.data.value).includes(url);

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

  if (official) return <Pill tone="neutral">built in</Pill>;

  return (
    <Button
      size="sm"
      variant={held ? 'secondary' : 'primary'}
      icon={<Icon name={held ? 'trash' : 'plus'} size={14} />}
      loading={busy || catalogSources.pending.value}
      onClick={() => void toggle()}>
      {held ? 'remove from my sources' : 'add to my sources'}
    </Button>
  );
}

function AppScreen({ listing, loading }: { listing: CatalogAppListing | null; loading: boolean }): VNode {
  const session = useDesktop();
  const [installing, setInstalling] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<AppVersion | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [allVersions, setAllVersions] = useState(false);

  const { libVersion } = deviceContext();
  const subscribed = subscribedUrls(catalogSources.data.value);

  if (!listing) {
    return (
      <Screen>
        <BackButton>back</BackButton>
        <SectionEmpty>
          {loading ? <Spinner class="mx-auto" /> : 'this app is no longer listed by that source'}
        </SectionEmpty>
      </Screen>
    );
  }

  const { app, sourceUrl, newestCompatible, installedVersion, updateAvailable } = listing;
  const extension = extensionOf(newestCompatible);
  const actionable = newestCompatible !== null && (installedVersion === null || updateAvailable);

  const install = async (version: AppVersion) => {
    setInstalling(version.version);
    setConfirming(null);
    setFailure(null);
    try {
      await session.installWebappFromUrl(
        version.download.url,
        sourceUrl,
        { size: version.download.size, sha256: version.download.sha256 },
        extensionOf(version)?.permissions,
      );
      reportInstall({ appId: app.id, sourceUrl, version: version.version });
      if (!subscribed.includes(sourceUrl)) {
        await session.addCatalogSource(sourceUrl);
        await catalogSources.refresh();
      }
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setInstalling(null);
    }
  };

  const start = (version: AppVersion) => {
    const older = newestCompatible !== null && version.version !== newestCompatible.version;
    if (older || version.extension) setConfirming(version);
    else void install(version);
  };

  const ordered = sortNewestFirst(app.versions);
  const shown = allVersions ? ordered : ordered.slice(0, 1);

  return (
    <Screen>
      <BackButton>back</BackButton>

      <div class="mb-6 flex items-center gap-4">
        <CatalogIcon url={app.icon} name={app.name} size="lg" />
        <div class="flex min-w-0 flex-1 flex-col gap-1.5">
          <h1 class="m-0 font-display text-hero font-medium tracking-display wrap-break-word">{app.name}</h1>
          <span class="wrap-break-word text-body text-muted">{app.author}</span>
          <div class="flex flex-wrap items-center gap-1.5">
            {installedVersion ? <Pill tone="ok">{`installed v${installedVersion}`}</Pill> : null}
            {newestCompatible?.role === 'launcher' ? <Pill tone="neutral">home screen</Pill> : null}
            {newestCompatible?.provides_overlay ? <Pill tone="neutral">overlay</Pill> : null}
            {extension ? <Pill tone="accent">native extension</Pill> : null}
          </div>
        </div>
      </div>

      <div class="mb-6">
        <Button
          variant="primary"
          icon={<Icon name="download" />}
          loading={installing !== null && installing === newestCompatible?.version}
          disabled={!actionable || installing !== null}
          onClick={() => {
            if (newestCompatible) start(newestCompatible);
          }}>
          {!newestCompatible
            ? 'needs newer firmware'
            : updateAvailable
              ? `update to v${newestCompatible.version}`
              : installedVersion
                ? 'installed'
                : `install v${newestCompatible.version}`}
        </Button>
      </div>

      {failure ? <ErrorNote>{failure}</ErrorNote> : null}

      <p class="mb-8 text-body leading-relaxed text-off-white">{app.description}</p>

      {newestCompatible ? (
        <Section>
          <SectionHeader title="what this app can do" />
          {newestCompatible.permissions.length === 0 ? (
            <SectionEmpty>nothing beyond drawing on the screen</SectionEmpty>
          ) : (
            <ListGroup>
              {newestCompatible.permissions.map(permission => {
                const copy = humanizePermission(permission);
                return (
                  <ListRow
                    key={permission}
                    icon={<Icon name={copy.icon} />}
                    title={copy.title}
                    subtitle={copy.subtitle}
                    value={permission}
                  />
                );
              })}
            </ListGroup>
          )}
        </Section>
      ) : null}

      {extension ? (
        <Section>
          <SectionHeader title="native extension" hint="native code this app runs on this computer" />
          <ListGroup>
            {describeExtensionPermissions(extension).map(grant => (
              <ListRow key={grant} icon={<Icon name="shield" />} iconTint="accent" title={grant} />
            ))}
            <ListRow
              icon={<Icon name="file" />}
              title="read it before you install"
              subtitle={extensionRepoLabel(app.source) ?? 'this app lists no repository'}
            />
          </ListGroup>
          <Hint>
            it runs outside the browser sandbox whenever this app is running, with the access above. your phone has no
            extension host, so it never runs there.
          </Hint>
        </Section>
      ) : null}

      <Section>
        <SectionHeader
          title="versions"
          action={app.versions.length > 1 ? (allVersions ? 'show fewer' : `all ${app.versions.length}`) : undefined}
          onAction={() => setAllVersions(open => !open)}
        />
        <ListGroup>
          {shown.map(version => (
            <VersionRow
              key={version.version}
              version={version}
              installedVersion={installedVersion}
              libVersion={libVersion}
              busy={installing}
              onInstall={() => start(version)}
            />
          ))}
        </ListGroup>
        <Hint>
          newest needs libbridgething {ordered[0]?.min_libbridgething_version} · {bytes(ordered[0]?.download.size ?? 0)}
        </Hint>
      </Section>

      <ConfirmInstall
        open={confirming !== null}
        title={confirming ? `install v${confirming.version}?` : 'install this version?'}
        extension={extensionOf(confirming)}
        source={app.source}
        older={confirming !== null && newestCompatible !== null && confirming.version !== newestCompatible.version}
        newest={newestCompatible}
        busy={installing !== null}
        onCancel={() => setConfirming(null)}
        onConfirm={() => {
          if (confirming) void install(confirming);
        }}
      />

      <Section>
        <SectionHeader title="where this came from" />
        <ListGroup>
          <ListRow icon={<Icon name="store" />} title="source" subtitle={sourceUrl} />
          {app.homepage ? <ListRow icon={<Icon name="globe" />} title="homepage" subtitle={app.homepage} /> : null}
          {app.source ? <ListRow icon={<Icon name="file" />} title="source code" subtitle={app.source} /> : null}
        </ListGroup>
        <Hint>
          apps are not reviewed. a listing means a source published it, never that anyone checked what it does.
        </Hint>
      </Section>
    </Screen>
  );
}

function ConfirmInstall({
  open,
  title,
  extension,
  source,
  older,
  newest,
  busy,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  title: string;
  extension: AppExtension | null;
  source: string | null;
  older: boolean;
  newest: AppVersion | null;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}): VNode {
  const repo = extensionRepoLabel(source);

  return (
    <Dialog
      open={open}
      onClose={onCancel}
      title={title}
      subtitle={
        older && newest
          ? `v${newest.version} is the newest build this Car Thing can run`
          : extension
            ? 'this app runs native code on this computer'
            : undefined
      }
      footer={
        <>
          <Button variant="ghost" onClick={onCancel}>
            cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={onConfirm}>
            {extension ? 'install and let it run' : 'install it anyway'}
          </Button>
        </>
      }>
      {extension ? (
        <>
          <p class="m-0 text-body text-muted">
            this app ships a native extension. the desktop app starts it outside the browser sandbox and keeps it
            running, granting it:
          </p>
          <ul class="mt-3 mb-0 flex list-none flex-col gap-1.5 p-0">
            {describeExtensionPermissions(extension).map(grant => (
              <li key={grant} class="flex items-center gap-2 text-body text-off-white">
                <span class="shrink-0 text-accent">
                  <Icon name="shield" size={14} />
                </span>
                {grant}
              </li>
            ))}
          </ul>
          <p class="mt-3 mb-0 text-hint text-muted">
            {repo ? `nobody reviews store apps. the code is at ${repo}.` : 'this app lists no repository to read.'}
          </p>
        </>
      ) : null}
      {older ? (
        <p class={extension ? 'mt-3 mb-0 text-body text-muted' : 'm-0 text-body text-muted'}>
          this replaces whatever is installed. an older build misses whatever the newer ones fixed, and the next update
          offer puts you back on the newest.
        </p>
      ) : null}
    </Dialog>
  );
}

function VersionRow({
  version,
  installedVersion,
  libVersion,
  busy,
  onInstall,
}: {
  version: AppVersion;
  installedVersion: string | null;
  libVersion: string | null;
  busy: string | null;
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
          <Pill tone="warn">needs newer firmware</Pill>
        ) : (
          <Button
            size="sm"
            variant="ghost"
            icon={<Icon name="download" size={13} />}
            loading={busy === version.version}
            disabled={busy !== null}
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
      <SectionHeader title="browse a source by url" />
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          value={url}
          onInput={setUrl}
          onCommit={open}
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
      <SectionHeader title="install a local bundle" hint="a webapp zip from this computer" />
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          value={path}
          onInput={setPath}
          onCommit={value => value.trim() && void start(value.trim())}
          icon={<Icon name="file" />}
          placeholder="/path/to/my-webapp.zip"
          clearable
        />
        <Button icon={<Icon name="upload" />} loading={busy} onClick={() => void browse()}>
          pick a file
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
        newest={null}
        busy={busy}
        onCancel={() => setConfirming(null)}
        onConfirm={() => {
          if (confirming) void install(confirming.bundle, confirming.extension.permissions);
        }}
      />
    </Section>
  );
}
