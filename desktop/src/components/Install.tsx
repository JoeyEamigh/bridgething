import {
  describeExtensionPermissions,
  extensionOf,
  extensionRepoLabel,
  type AppExtension,
  type AppVersion,
  type CatalogAppListing,
} from '@bridgething/catalog';
import type { WebappSlot } from '@bridgething/companion-types';
import { Button, describeError, Dialog, useSession } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { useDesktop } from '../desktop.ts';
import { Icon } from '../lib/icons.tsx';
import { catalogSources, otaQueues, otaRuns, webappActive, webapps, webappSlots } from '../stores/session.ts';

export type InstallState = 'install' | 'update' | 'installed' | 'queued' | 'installing' | 'incompatible';

export type Installer = {
  state: InstallState;
  label: string;
  waitingVersion: string | null;
  busy: boolean;
  failure: string | null;
  start: (version: AppVersion) => void;
  dialog: VNode;
};

export function useInstaller(listing: CatalogAppListing): Installer {
  const session = useDesktop();
  const [confirming, setConfirming] = useState<AppVersion | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const { app, sourceUrl, newestCompatible, installedVersion, updateAvailable } = listing;
  const waiting = otaQueues.value.flatMap(queue => queue.installs).find(entry => entry.webappId === app.id) ?? null;
  const running = otaRuns.value.some(run => !run.outcome && run.webappId === app.id);
  const busy = waiting !== null || running;

  const state: InstallState = running
    ? 'installing'
    : waiting
      ? 'queued'
      : !newestCompatible
        ? 'incompatible'
        : updateAvailable
          ? 'update'
          : installedVersion
            ? 'installed'
            : 'install';

  const install = async (version: AppVersion) => {
    setConfirming(null);
    setFailure(null);
    try {
      await session.installWebappFromUrl(
        version.download.url,
        sourceUrl,
        { size: version.download.size, sha256: version.download.sha256 },
        extensionOf(version)?.permissions,
        { id: app.id, name: app.name, version: version.version },
      );
      if (!catalogSources.data.value.includes(sourceUrl)) {
        await session.addCatalogSource(sourceUrl);
        await catalogSources.refresh();
      }
    } catch (reason) {
      setFailure(describeError(reason));
    }
  };

  const start = (version: AppVersion) => {
    const older = newestCompatible !== null && version.version !== newestCompatible.version;
    if (older || version.extension) setConfirming(version);
    else void install(version);
  };

  const older = confirming !== null && newestCompatible !== null && confirming.version !== newestCompatible.version;

  return {
    state,
    label: LABELS[state],
    waitingVersion: waiting?.version ?? null,
    busy,
    failure,
    start,
    dialog: (
      <ConfirmInstall
        open={confirming !== null}
        title={confirming ? `install v${confirming.version}?` : ''}
        extension={extensionOf(confirming)}
        source={app.source}
        older={older}
        busy={busy}
        onCancel={() => setConfirming(null)}
        onConfirm={() => {
          if (confirming) void install(confirming);
        }}
      />
    ),
  };
}

const LABELS: Record<InstallState, string> = {
  install: 'install',
  update: 'update',
  installed: 'installed',
  queued: 'queued',
  installing: 'installing',
  incompatible: 'needs newer firmware',
};

export function InstallButton({
  listing,
  installer,
  size,
}: {
  listing: CatalogAppListing;
  installer: Installer;
  size?: 'sm';
}): VNode {
  const { state, label, start } = installer;
  const actionable = state === 'install' || state === 'update';

  if (state === 'installed') return <UseButton listing={listing} size={size} />;

  return (
    <Button
      size={size}
      variant={actionable ? 'primary' : 'secondary'}
      icon={actionable ? <Icon name="download" size={size === 'sm' ? 13 : undefined} /> : undefined}
      loading={state === 'installing'}
      disabled={!actionable}
      onClick={() => {
        if (listing.newestCompatible) start(listing.newestCompatible);
      }}>
      {label}
    </Button>
  );
}

function UseButton({ listing, size }: { listing: CatalogAppListing; size?: 'sm' }): VNode {
  const session = useSession();
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const installed = webapps.data.value.find(app => app.id.toLowerCase() === listing.app.id.toLowerCase());
  const newest = listing.newestCompatible;
  const slot: WebappSlot | null =
    newest?.role === 'launcher' ? 'launcher' : newest?.provides_overlay ? 'overlay' : null;

  if (!installed) {
    return (
      <Button size={size} variant="secondary" disabled>
        installed
      </Button>
    );
  }

  const using = slot ? webappSlots.data.value[slot] === installed.id : webappActive.data.value?.id === installed.id;
  const run = async () => {
    setBusy(true);
    setFailed(false);
    try {
      if (slot) await session.setWebappSlot(slot, installed.id);
      else await session.switchWebapp(installed.id);
    } catch {
      setFailed(true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Button
      size={size}
      variant="secondary"
      icon={using ? undefined : <Icon name="play" size={size === 'sm' ? 13 : undefined} />}
      loading={busy}
      disabled={using}
      onClick={() => void run()}>
      {using ? USING[slot ?? 'screen'] : failed ? 'retry' : USE[slot ?? 'screen']}
    </Button>
  );
}

const USE: Record<WebappSlot | 'screen', string> = {
  launcher: 'use as home',
  overlay: 'use as overlay',
  screen: 'open',
};

const USING: Record<WebappSlot | 'screen', string> = {
  launcher: 'home screen',
  overlay: 'overlay',
  screen: 'on screen',
};

export function ConfirmInstall({
  open,
  title,
  extension,
  source,
  older,
  busy,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  title: string;
  extension: AppExtension | null;
  source: string | null;
  older: boolean;
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
      subtitle={older ? 'not the newest version' : extension ? 'runs native code on this computer' : undefined}
      footer={
        <>
          <Button variant="ghost" onClick={onCancel}>
            cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={onConfirm}>
            install
          </Button>
        </>
      }>
      {extension ? (
        <>
          <ul class="m-0 flex list-none flex-col gap-1.5 p-0">
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
            {repo ? `unreviewed · code at ${repo}` : 'unreviewed · no repository'}
          </p>
        </>
      ) : null}
    </Dialog>
  );
}
