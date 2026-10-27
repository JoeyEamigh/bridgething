import {
  changelogText,
  compareVersions,
  describeExtensionPermissions,
  extensionOf,
  listingKindLabel,
  moreFromAuthor,
  sortNewestFirst,
  sourceLine,
  STORE_COPY,
  versionCompatible,
  webHref,
  type AppVersion,
  type CatalogAppListing,
} from '@bridgething/catalog';
import { Button, ListGroup, ListRow, Pill, SectionEmpty, SectionHeader, Spinner } from '@bridgething/ui';
import { openUrl } from '@tauri-apps/plugin-opener';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { InstallButton, useInstaller } from '../components/Install.tsx';
import { BackButton, ErrorNote, Screen, Section } from '../components/Screen.tsx';
import { AppTiles, Screenshot } from '../components/Storefront.tsx';
import { bytes, day, since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { humanizePermission } from '../lib/permissions.ts';
import { CatalogIcon } from '../lib/webapp-icon.tsx';
import { PATHS } from '../routes.ts';
import { deviceContext, sourceListings, storeListings } from '../stores/catalog.ts';

export function CatalogAppRoute({ appId }: { appId: string }): VNode {
  const id = decodeURIComponent(appId);
  const { listings, pending } = storeListings();

  return <AppScreen listing={listings.find(entry => entry.app.id === id) ?? null} loading={pending} />;
}

export function SourceAppRoute({ source, appId }: { source: string; appId: string }): VNode {
  const url = decodeURIComponent(source);
  const id = decodeURIComponent(appId);
  const { listings, catalog } = sourceListings(url);

  return <AppScreen listing={listings.find(entry => entry.app.id === id) ?? null} loading={catalog.pending.value} />;
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
  const { listings, sources } = storeListings();

  const { app, sourceUrl, newestCompatible, installedVersion } = listing;
  const extension = extensionOf(newestCompatible);
  const ordered = sortNewestFirst(app.versions);
  const newest = newestCompatible ?? ordered[0] ?? null;
  const notes = changelogText(newest?.changelog);
  const shown = allVersions ? ordered : ordered.slice(0, 3);
  const shots = (app.screenshots ?? []).map(webHref).filter((url): url is string => url !== null);
  const homepage = webHref(app.homepage);
  const code = webHref(app.source);
  const others = moreFromAuthor(listings, listing);
  const authorPath = PATHS.storeBrowse({ filter: { kind: 'author', author: app.author }, sort: 'popular' });
  const kind = listingKindLabel(listing);
  const stats = [
    listing.installs > 0 ? { label: 'installs', value: String(listing.installs) } : null,
    kind ? { label: kindTitle(listing), value: kind } : null,
    newest && newest.download.size > 0 ? { label: 'size', value: bytes(newest.download.size) } : null,
  ].filter(stat => stat !== null);

  return (
    <Screen>
      <BackButton>back</BackButton>

      <div class="mb-6 flex items-center gap-5">
        <CatalogIcon url={app.icon} name={app.name} size="xl" class="border border-rule" />
        <div class="flex min-w-0 flex-1 flex-col gap-1">
          <h1 class="m-0 font-display text-screen-title font-medium tracking-display wrap-break-word">{app.name}</h1>
          <a
            href={authorPath}
            class="self-start text-row text-muted transition-colors hover:text-off-white focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2">
            {app.author}
          </a>
          {installedVersion || extension ? (
            <div class="mt-1 flex flex-wrap items-center gap-1.5">
              {installedVersion ? <Pill tone="ok">{`installed v${installedVersion}`}</Pill> : null}
              {extension ? <Pill tone="accent">native extension</Pill> : null}
            </div>
          ) : null}
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

      {stats.length > 0 ? (
        <dl class="m-0 mb-6 grid auto-cols-fr grid-flow-col border border-rule bg-screen">
          {stats.map(stat => (
            <div
              key={stat.label}
              class="flex min-w-0 flex-col-reverse gap-1 border-l border-rule px-4 py-3 first:border-l-0">
              <dt class="font-mono text-eyebrow tracking-[0.18em] text-dim uppercase">{stat.label}</dt>
              <dd class="m-0 truncate text-row font-medium text-off-white">{stat.value}</dd>
            </div>
          ))}
        </dl>
      ) : null}

      {shots.length > 0 ? (
        <div class="mb-6 flex snap-x gap-4 overflow-x-auto pb-3">
          {shots.map(shot => (
            <Screenshot key={shot} listing={listing} src={shot} class="w-104 shrink-0 snap-start" />
          ))}
        </div>
      ) : null}

      <p class="mb-9 text-body leading-relaxed whitespace-pre-line text-off-white">{app.description}</p>

      {newest && notes ? (
        <Section>
          <SectionHeader title={`new in ${newest.version}`} hint={since(newest.released_at)} />
          <p class="m-0 text-body leading-relaxed whitespace-pre-line text-soft">{notes}</p>
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

      {others.length > 0 ? (
        <Section>
          <SectionHeader title={`more from ${app.author}`} />
          <AppTiles listings={others} />
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
      </Section>

      <a
        href={PATHS.storeSource(sourceUrl)}
        class="font-mono text-hint text-dim transition-colors hover:text-soft focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2">
        {sourceLine(sources[sourceUrl])}
      </a>

      {installer.dialog}
    </Screen>
  );
}

function kindTitle(listing: CatalogAppListing): string {
  const newest = listing.newestCompatible;
  return newest?.role === 'launcher' || newest?.provides_overlay ? 'kind' : 'category';
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
      subtitle={changelogText(version.changelog) ?? undefined}
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
