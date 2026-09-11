import { listedWebapps } from '@bridgething/catalog';
import type { WebappInfo, WebappSlot, WebappSlots } from '@bridgething/companion-types';
import { Button, Dialog, ListGroup, ListRow, Pill, SectionEmpty, SectionHeader, Spinner } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { message } from '../../lib/browser-session';
import { useBrowser, useBrowserQuery } from '../../lib/browser-tier';
import { reportInstalled } from '../../lib/directory-client';
import { ErrorNote, Hint, Section } from './Screen';

export function Webapps({ serial }: { serial: string | null }): VNode {
  const webapps = useBrowserQuery(['webapps'], s => s.webapps());
  const active = useBrowserQuery(['webapps'], s => s.webappActive());
  const [opened, setOpened] = useState<string | null>(null);

  const held = webapps.data;
  useEffect(() => {
    if (!serial || !held) return;
    reportInstalled({
      deviceId: serial,
      apps: held
        .filter(webapp => webapp.source === 'installed' && webapp.provenance)
        .map(webapp => ({ appId: webapp.id, sourceUrl: webapp.provenance!, version: webapp.version })),
    });
  }, [serial, held]);

  const list = webapps.data ?? [];
  const listed = listedWebapps(list);
  const detail = list.find(webapp => webapp.id === opened) ?? null;

  return (
    <>
      <Section>
        <SectionHeader
          title="installed"
          hint="the one marked live is on the screen now"
          action="refresh"
          pending={webapps.loading}
          onAction={webapps.refetch}
        />
        {webapps.loading && listed.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : listed.length === 0 ? (
          <SectionEmpty>nothing installed</SectionEmpty>
        ) : (
          <ListGroup>
            {listed.map(webapp => (
              <ListRow
                key={webapp.id}
                title={webapp.name}
                subtitle={webapp.description ?? webapp.id}
                value={`v${webapp.version}`}
                trailing={
                  active.data?.id === webapp.id ? (
                    <Pill tone="ok" dot>
                      live
                    </Pill>
                  ) : webapp.source === 'builtin' ? (
                    <Pill tone="neutral">built-in</Pill>
                  ) : undefined
                }
                chevron
                onClick={() => setOpened(webapp.id)}
              />
            ))}
          </ListGroup>
        )}
        {webapps.error ? <ErrorNote>{webapps.error}</ErrorNote> : null}
      </Section>

      {detail ? (
        <WebappDialog webapp={detail} running={active.data?.id === detail.id} onClose={() => setOpened(null)} />
      ) : null}

      <Slots webapps={list} />
    </>
  );
}

function WebappDialog({
  webapp,
  running,
  onClose,
}: {
  webapp: WebappInfo;
  running: boolean;
  onClose: () => void;
}): VNode {
  const session = useBrowser();
  const [busy, setBusy] = useState<'switch' | 'uninstall' | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const builtin = webapp.source === 'builtin';

  const act = async (kind: 'switch' | 'uninstall') => {
    setBusy(kind);
    setFailure(null);
    try {
      if (kind === 'switch') await session.switchWebapp(webapp.id);
      else await session.uninstallWebapp(webapp.id);
      onClose();
    } catch (reason) {
      setFailure(message(reason));
    } finally {
      setBusy(null);
      setConfirming(false);
    }
  };

  return (
    <>
      <Dialog
        open={!confirming}
        onClose={onClose}
        title={webapp.name}
        subtitle={`${webapp.id} · v${webapp.version}`}
        footer={
          <>
            {!builtin ? (
              <Button variant="destructive" onClick={() => setConfirming(true)}>
                uninstall
              </Button>
            ) : null}
            <Button variant="primary" loading={busy === 'switch'} disabled={running} onClick={() => void act('switch')}>
              {running ? 'already on screen' : 'switch to this'}
            </Button>
          </>
        }>
        <div class="flex flex-col gap-4">
          <div class="flex flex-wrap items-center gap-1.5">
            <Pill tone={builtin ? 'neutral' : 'accent'}>{builtin ? 'built-in' : 'installed'}</Pill>
            {running ? <Pill tone="ok">on screen</Pill> : null}
            {webapp.role === 'launcher' ? <Pill tone="neutral">home screen</Pill> : null}
            {webapp.overlayHash ? <Pill tone="neutral">overlay</Pill> : null}
          </div>

          {webapp.description ? <p class="text-body text-muted m-0 leading-relaxed">{webapp.description}</p> : null}

          <div>
            <SectionHeader title="what it can do" />
            {webapp.permissions.length === 0 ? (
              <SectionEmpty>nothing beyond drawing on the screen</SectionEmpty>
            ) : (
              <ListGroup>
                {webapp.permissions.map(permission => (
                  <ListRow key={permission} title={permission} />
                ))}
              </ListGroup>
            )}
          </div>

          {webapp.provenance ? (
            <div>
              <SectionHeader title="where it came from" />
              <ListGroup>
                <ListRow title={webapp.provenance} />
              </ListGroup>
            </div>
          ) : null}

          {webapp.config.length > 0 ? <Hint>this app's settings are edited on the device itself.</Hint> : null}
          {failure ? <ErrorNote>{failure}</ErrorNote> : null}
        </div>
      </Dialog>

      <Dialog
        open={confirming}
        onClose={() => setConfirming(false)}
        title={`uninstall ${webapp.name}?`}
        subtitle={`v${webapp.version} comes off the device, and its settings go with it.`}
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirming(false)}>
              keep it
            </Button>
            <Button variant="destructive" loading={busy === 'uninstall'} onClick={() => void act('uninstall')}>
              uninstall
            </Button>
          </>
        }>
        <p class="text-body text-muted m-0">reinstalling pulls the bundle down again.</p>
        {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      </Dialog>
    </>
  );
}

function Slots({ webapps }: { webapps: WebappInfo[] }): VNode {
  const session = useBrowser();
  const slots = useBrowserQuery(['webapps'], s => s.webappSlots());
  const [busy, setBusy] = useState<WebappSlot | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const assign = async (slot: WebappSlot, id: string | null) => {
    setBusy(slot);
    setFailure(null);
    try {
      await session.setWebappSlot(slot, id);
    } catch (reason) {
      setFailure(message(reason));
    } finally {
      setBusy(null);
    }
  };

  const held: WebappSlots = slots.data ?? { launcher: null, overlay: null };

  return (
    <Section>
      <SectionHeader title="roles" hint="which installed app provides each system surface" />
      {slots.error ? (
        <SectionEmpty>{slots.error}</SectionEmpty>
      ) : (
        <div class="flex flex-col gap-6">
          <SlotPicker
            title="home screen"
            builtinLabel="built-in hub"
            builtinDetail="the launcher that ships with bridgething"
            candidates={webapps.filter(app => app.role === 'launcher' && app.source === 'installed')}
            selected={held.launcher}
            busy={busy === 'launcher'}
            onAssign={id => void assign('launcher', id)}
          />
          <SlotPicker
            title="system overlay"
            builtinLabel="built-in overlay"
            builtinDetail="notifications, calls, pairing, volume"
            candidates={webapps.filter(app => app.overlayHash !== null && app.source === 'installed')}
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
    busy ? <Spinner /> : chosen ? <Pill tone="accent">in use</Pill> : <span class="size-4" />;

  return (
    <div>
      <span class="text-eyebrow text-muted mb-2 block font-mono tracking-[0.18em] uppercase">{title}</span>
      <ListGroup>
        <ListRow
          title={builtinLabel}
          subtitle={builtinDetail}
          trailing={mark(selected === null)}
          disabled={busy}
          onClick={() => onAssign(null)}
        />
        {candidates.map(app => (
          <ListRow
            key={app.id}
            title={app.name}
            subtitle={`v${app.version}`}
            trailing={mark(selected === app.id)}
            disabled={busy}
            onClick={() => onAssign(app.id)}
          />
        ))}
      </ListGroup>
      {candidates.length === 0 ? <Hint>no installed app offers this yet</Hint> : null}
    </div>
  );
}
