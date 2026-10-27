import {
  describeExtensionPermissions,
  listedWebapps,
  slotCandidates,
  type CatalogAppListing,
} from '@bridgething/catalog';
import type { WebappInfo, WebappSlot, WebappSlots } from '@bridgething/companion-types';
import {
  Button,
  Dialog,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionEmpty,
  SectionHeader,
  Spinner,
  Switch,
  describeError,
  hasSettings,
  useSession,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';
import { useState } from 'preact/hooks';

import { InstallButton, useInstaller } from '../components/Install.tsx';
import { BackButton, ErrorNote, Screen, Section } from '../components/Screen.tsx';
import { useDesktop, type ExtensionEntry } from '../desktop.ts';
import {
  EXTENSION_MISSING,
  describeExtensionStatus,
  extensionFor,
  extensionMissing,
  needsRuntime,
  orphanedExtensions,
} from '../lib/extension.ts';
import { Icon } from '../lib/icons.tsx';
import { humanizePermission } from '../lib/permissions.ts';
import { WebappIcon } from '../lib/webapp-icon.tsx';
import { PATHS } from '../routes.ts';
import { storeListings } from '../stores/catalog.ts';
import { extensions, webappActive, webappSlots, webapps } from '../stores/session.ts';

export function AppsRoute(): VNode {
  const { route } = useLocation();
  const list = listedWebapps(webapps.data.value);
  const failure = webapps.error.value;
  const pending = webapps.pending.value;
  const active = webappActive.data.value;
  const listings = installedListings();

  return (
    <Screen>
      <ScreenHeader title="apps" />

      {failure ? <ErrorNote>{failure}</ErrorNote> : null}

      <Section>
        <SectionHeader title="installed" />
        {pending && list.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : list.length === 0 ? (
          <SectionEmpty>no apps</SectionEmpty>
        ) : (
          <ListGroup>
            {list.map(app => (
              <ListRow
                key={app.id}
                icon={<WebappIcon id={app.id} iconHash={app.iconHash} name={app.name} size="sm" />}
                title={app.name}
                subtitle={app.description ?? undefined}
                value={`v${app.version}`}
                trailing={
                  <InstalledMark
                    app={app}
                    onScreen={active?.id === app.id}
                    update={listings.get(app.id.toLowerCase())?.updateAvailable ?? false}
                  />
                }
                chevron
                onClick={() => route(PATHS.app(app.id))}
              />
            ))}
          </ListGroup>
        )}
      </Section>

      <OrphanedExtensions />

      <SlotAssignment list={list} />
    </Screen>
  );
}

function installedListings(): Map<string, CatalogAppListing> {
  return new Map(
    storeListings()
      .listings.filter(listing => listing.installedVersion !== null)
      .map(listing => [listing.app.id.toLowerCase(), listing]),
  );
}

function UpdateButton({ listing }: { listing: CatalogAppListing }): VNode {
  const installer = useInstaller(listing);
  return (
    <>
      <InstallButton listing={listing} installer={installer} />
      {installer.failure ? <ErrorNote>{installer.failure}</ErrorNote> : null}
      {installer.dialog}
    </>
  );
}

function OrphanedExtensions(): VNode | null {
  const orphans = orphanedExtensions(extensions.data.value);
  if (orphans.length === 0) return null;

  return (
    <Section>
      <SectionHeader title="extensions with no app" />
      <div class="flex flex-col gap-6">
        {orphans.map(entry => (
          <div key={entry.id}>
            <span class="mb-2 block font-mono text-eyebrow tracking-[0.18em] text-muted uppercase">
              {entry.name} v{entry.version}
            </span>
            <ExtensionRow entry={entry} />
          </div>
        ))}
      </div>
    </Section>
  );
}

export function AppDetailRoute({ webappId }: { webappId: string }): VNode {
  const id = decodeURIComponent(webappId);
  const pending = webapps.pending.value;
  const webapp = webapps.data.value.find(app => app.id === id) ?? null;

  if (!webapp) {
    return (
      <Screen>
        <BackButton to={PATHS.apps}>all apps</BackButton>
        <SectionEmpty>{pending ? <Spinner class="mx-auto" /> : 'not installed'}</SectionEmpty>
      </Screen>
    );
  }

  return <AppDetail key={webapp.id} webapp={webapp} />;
}

function AppDetail({ webapp }: { webapp: WebappInfo }): VNode {
  const session = useSession();
  const { route } = useLocation();
  const active = webappActive.data.value;

  const [busy, setBusy] = useState<'switch' | 'uninstall' | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const builtin = webapp.source === 'builtin';
  const running = active?.id === webapp.id;
  const listing = installedListings().get(webapp.id.toLowerCase()) ?? null;

  const act = async (kind: 'switch' | 'uninstall') => {
    setBusy(kind);
    setFailure(null);
    try {
      if (kind === 'switch') await session.switchWebapp(webapp.id);
      else {
        await session.uninstallWebapp(webapp.id);
        route(PATHS.apps, true);
      }
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(null);
      setConfirming(false);
    }
  };

  return (
    <Screen>
      <BackButton to={PATHS.apps}>all apps</BackButton>

      <div class="mb-6 flex items-center gap-4">
        <WebappIcon id={webapp.id} iconHash={webapp.iconHash} name={webapp.name} size="lg" />
        <div class="flex min-w-0 flex-1 flex-col gap-1.5">
          <h1 class="m-0 font-display text-hero font-medium tracking-display wrap-break-word">{webapp.name}</h1>
          <div class="flex flex-wrap items-center gap-1.5">
            {builtin ? <Pill tone="neutral">built-in</Pill> : null}
            {running ? <Pill tone="ok">on screen</Pill> : null}
            {webapp.role === 'launcher' ? <Pill tone="neutral">home screen</Pill> : null}
            {webapp.overlayHash ? <Pill tone="neutral">overlay</Pill> : null}
            <span class="font-mono text-hint text-muted">v{webapp.version}</span>
          </div>
        </div>
      </div>

      {webapp.description ? <p class="mb-6 text-body leading-relaxed text-muted">{webapp.description}</p> : null}

      <div class="mb-8 flex flex-wrap gap-2">
        <Button
          variant="primary"
          icon={<Icon name="play" />}
          loading={busy === 'switch'}
          disabled={running}
          onClick={() => void act('switch')}>
          {running ? 'on screen' : 'open on device'}
        </Button>
        {listing?.updateAvailable ? <UpdateButton listing={listing} /> : null}
        {hasSettings(webapp) ? (
          <Button icon={<Icon name="gear" />} onClick={() => route(PATHS.appSettings(webapp.id))}>
            settings
          </Button>
        ) : null}
        {listing ? (
          <Button icon={<Icon name="store" />} onClick={() => route(PATHS.storeApp(listing.app.id))}>
            store page
          </Button>
        ) : null}
        {!builtin ? (
          <Button variant="ghost" icon={<Icon name="trash" />} class="ml-auto" onClick={() => setConfirming(true)}>
            uninstall
          </Button>
        ) : null}
      </div>

      {failure ? <ErrorNote>{failure}</ErrorNote> : null}

      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={`uninstall ${webapp.name}?`}
        subtitle="its settings go with it"
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              cancel
            </Button>
            <Button variant="destructive" loading={busy === 'uninstall'} onClick={() => void act('uninstall')}>
              uninstall
            </Button>
          </>
        }></Dialog>

      <Section>
        <SectionHeader title="permissions" />
        {webapp.permissions.length === 0 ? (
          <SectionEmpty>none</SectionEmpty>
        ) : (
          <ListGroup>
            {webapp.permissions.map(permission => {
              const copy = humanizePermission(permission);
              return <ListRow key={permission} icon={<Icon name={copy.icon} />} title={copy.title} />;
            })}
          </ListGroup>
        )}
      </Section>

      <ExtensionSection webapp={webapp} />

      {webapp.provenance && !listing ? (
        <Section>
          <SectionHeader title="source" />
          <ListGroup>
            <ListRow icon={<Icon name="link" />} title={webapp.provenance} />
          </ListGroup>
        </Section>
      ) : null}
    </Screen>
  );
}

function InstalledMark({
  app,
  onScreen,
  update,
}: {
  app: WebappInfo;
  onScreen: boolean;
  update: boolean;
}): VNode | null {
  if (update) return <Pill tone="accent">update</Pill>;
  if (onScreen) return <Pill tone="ok">on screen</Pill>;
  const entry = extensionFor(extensions.data.value, app.id);
  if (entry) {
    const copy = describeExtensionStatus(entry.status);
    return (
      <Pill tone={entry.enabled ? copy.tone : 'neutral'} dot>
        {entry.enabled ? copy.label : 'extension off'}
      </Pill>
    );
  }
  if (extensionMissing(extensions.data.value, app)) {
    return (
      <Pill tone={EXTENSION_MISSING.tone} dot>
        {EXTENSION_MISSING.label}
      </Pill>
    );
  }
  return app.source === 'builtin' ? <Pill tone="neutral">built-in</Pill> : null;
}

function ExtensionSection({ webapp }: { webapp: WebappInfo }): VNode | null {
  const entry = extensionFor(extensions.data.value, webapp.id);
  const declared = webapp.extension;

  if (!entry && !declared) return null;

  return (
    <Section>
      <SectionHeader title="native extension" hint="runs on this computer" />
      {entry ? <ExtensionRow entry={entry} /> : <MissingExtensionRow permissions={declared?.permissions ?? []} />}
    </Section>
  );
}

function MissingExtensionRow({ permissions }: { permissions: string[] }): VNode {
  return (
    <ListGroup>
      <ListRow
        icon={<Icon name={EXTENSION_MISSING.icon} />}
        iconTint={EXTENSION_MISSING.tint}
        title={EXTENSION_MISSING.label}
        subtitle={EXTENSION_MISSING.detail}
      />
      {describeExtensionPermissions({ desktop: true, permissions }).map((line, at) => (
        <ListRow
          key={permissions[at]}
          icon={<Icon name="shield" />}
          iconTint="warn"
          title={line}
          value={permissions[at]}
        />
      ))}
    </ListGroup>
  );
}

type Act = 'toggle' | 'folder' | 'runtime' | 'remove';

function ExtensionRow({ entry }: { entry: ExtensionEntry }): VNode {
  const session = useDesktop();
  const [busy, setBusy] = useState<Act | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const copy = describeExtensionStatus(entry.status);

  const act = async (kind: Act, run: () => Promise<void>) => {
    setBusy(kind);
    setFailure(null);
    try {
      await run();
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <ListGroup>
        <ListRow
          icon={<Icon name={copy.icon} />}
          iconTint={entry.enabled ? copy.tint : 'default'}
          title={entry.enabled ? copy.label : 'turned off'}
          subtitle={entry.enabled ? copy.detail : undefined}
          value={`api v${entry.api}`}
          trailing={
            busy === 'toggle' ? (
              <Spinner />
            ) : (
              <Switch
                checked={entry.enabled}
                label={`run the ${entry.name} extension`}
                onChange={next => void act('toggle', () => session.setExtensionEnabled(entry.id, next))}
              />
            )
          }
        />
        {entry.permissions.length === 0 ? (
          <ListRow icon={<Icon name="shield" />} title="no access to this computer" />
        ) : (
          describeExtensionPermissions({ desktop: true, permissions: entry.permissions }).map((line, at) => (
            <ListRow
              key={entry.permissions[at]}
              icon={<Icon name="shield" />}
              iconTint="warn"
              title={line}
              value={entry.permissions[at]}
            />
          ))
        )}
      </ListGroup>

      <div class="mt-2 flex flex-wrap gap-2">
        <Button
          size="sm"
          icon={<Icon name="file" />}
          loading={busy === 'folder'}
          onClick={() => void act('folder', () => session.openExtensionData(entry.id))}>
          data folder
        </Button>
        {needsRuntime(entry.status) ? (
          <Button
            size="sm"
            variant="primary"
            icon={<Icon name="download" />}
            loading={busy === 'runtime'}
            onClick={() => void act('runtime', () => session.retryExtensionRuntime())}>
            get the runtime
          </Button>
        ) : null}
        {entry.orphaned ? (
          <Button
            size="sm"
            variant="destructive"
            icon={<Icon name="trash" />}
            loading={busy === 'remove'}
            onClick={() => void act('remove', () => session.removeExtension(entry.id))}>
            remove
          </Button>
        ) : null}
      </div>

      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </>
  );
}

function SlotAssignment({ list }: { list: WebappInfo[] }): VNode | null {
  const session = useSession();
  const slots = webappSlots;
  const [busy, setBusy] = useState<WebappSlot | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const assign = async (slot: WebappSlot, id: string | null) => {
    setBusy(slot);
    setFailure(null);
    try {
      await session.setWebappSlot(slot, id);
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(null);
    }
  };

  const launchers = slotCandidates(list, 'launcher');
  const overlays = slotCandidates(list, 'overlay');
  const held: WebappSlots = slots.data.value;

  if (launchers.length === 0 && overlays.length === 0 && held.launcher === null && held.overlay === null) return null;

  return (
    <Section>
      <SectionHeader title="roles" />
      {slots.error.value ? (
        <SectionEmpty>{slots.error.value}</SectionEmpty>
      ) : (
        <div class="flex flex-col gap-6">
          <SlotPicker
            title="home screen"
            builtinLabel="hub"
            builtinDetail="built-in"
            candidates={launchers}
            selected={held.launcher}
            busy={busy === 'launcher'}
            onAssign={id => void assign('launcher', id)}
          />
          <SlotPicker
            title="system overlay"
            builtinLabel="overlay"
            builtinDetail="built-in"
            candidates={overlays}
            selected={held.overlay}
            busy={busy === 'overlay'}
            onAssign={id => void assign('overlay', id)}
          />
        </div>
      )}
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}

function SlotPicker({
  title,
  builtinLabel,
  builtinDetail,
  candidates,
  selected,
  busy,
  onAssign,
}: {
  title: string;
  builtinLabel: string;
  builtinDetail: string;
  candidates: WebappInfo[];
  selected: string | null;
  busy: boolean;
  onAssign: (id: string | null) => void;
}): VNode {
  const mark = (chosen: boolean) =>
    busy ? <Spinner /> : chosen ? <Icon name="check" class="text-accent" /> : <span class="size-4" />;

  return (
    <div>
      <span class="mb-2 block font-mono text-eyebrow tracking-[0.18em] text-muted uppercase">{title}</span>
      <ListGroup>
        <ListRow
          icon={<Icon name="layers" />}
          iconTint={selected === null ? 'accent' : 'default'}
          title={builtinLabel}
          subtitle={builtinDetail}
          trailing={mark(selected === null)}
          disabled={busy}
          onClick={() => onAssign(null)}
        />
        {candidates.map(app => (
          <ListRow
            key={app.id}
            icon={<WebappIcon id={app.id} iconHash={app.iconHash} name={app.name} size="sm" />}
            title={app.name}
            subtitle={`v${app.version}`}
            trailing={mark(selected === app.id)}
            disabled={busy}
            onClick={() => onAssign(app.id)}
          />
        ))}
      </ListGroup>
    </div>
  );
}
