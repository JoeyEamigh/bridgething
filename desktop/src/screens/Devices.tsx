import { listedWebapps } from '@bridgething/catalog';
import type { DeviceMetaEntry, SessionPeer } from '@bridgething/companion-types';
import {
  Button,
  Dialog,
  Field,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionEmpty,
  SectionHeader,
  StatusStrip,
  cx,
  describeError,
  useSession,
  type Endpoint,
  type Tone,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';
import { useState } from 'preact/hooks';

import { ConnectFlow } from '../components/ConnectFlow.tsx';
import { OtaRunCard } from '../components/OtaRunCard.tsx';
import { ErrorNote, Hint, Screen, Section } from '../components/Screen.tsx';
import { useDesktop } from '../desktop.ts';
import { peerHost, since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { hasActivity } from '../lib/ota.ts';
import { WebappIcon } from '../lib/webapp-icon.tsx';
import { PATHS } from '../routes.ts';
import {
  deviceMeta,
  endpoints,
  knownDevices,
  otaAvailable,
  otaRuns,
  peers,
  selectedDevice,
  webappActive,
  webapps,
} from '../stores/session.ts';

export function DevicesScreen(): VNode {
  const { route } = useLocation();
  const session = useSession();

  const found = endpoints.data.value;
  const linked = peers.value;
  const meta = deviceMeta.value;
  const live = linked.filter(peer => peer.status === 'connected');
  const chosen = selectedDevice.data.value;
  const primary = live.find(peer => peer.id === chosen) ?? (live.length === 1 ? (live[0] ?? null) : null);

  const run = primary ? otaRuns.value.find(entry => entry.deviceId === primary.id) : undefined;
  const offered = primary ? otaAvailable.value.find(entry => entry.deviceId === primary.id) : undefined;

  return (
    <Screen>
      <ScreenHeader
        eyebrow="home"
        title="devices"
        subtitle="the daemons this computer can reach, and what is running on them."
        trailing={
          primary ? (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                void session.disconnect();
              }}>
              disconnect
            </Button>
          ) : undefined
        }
      />

      {primary === null ? <StatusStrip {...unattached(linked, live, found)} class="mb-6" /> : null}

      {run && hasActivity(run, offered) ? (
        <Section>
          <SectionHeader title="update" action="all releases" onAction={() => route(PATHS.updates)} />
          <OtaRunCard
            run={run}
            onDismiss={() => {
              void session.dismissOtaRun();
            }}
          />
        </Section>
      ) : offered?.releaseVersion ? (
        <StatusStrip
          tone="accent"
          title={`release ${offered.releaseVersion} is available`}
          subtitle={`daemon ${offered.daemonVersion ?? '?'} · image ${offered.imageVersion ?? '?'}`}
          onClick={() => route(PATHS.updates)}
          class="mb-6"
        />
      ) : null}

      <ConnectFlow />

      <Section>
        <SectionHeader
          title="links"
          hint={live.length > 1 ? 'the accent one is the one this app talks to' : 'one daemon at a time'}
        />
        {linked.length === 0 ? (
          <SectionEmpty>not connected to anything</SectionEmpty>
        ) : (
          <ListGroup>
            {linked.map(peer => (
              <PeerRow key={peer.id} peer={peer} meta={meta} active={peer.id === primary?.id} />
            ))}
          </ListGroup>
        )}
      </Section>

      <KnownDevices />

      {primary ? <InstalledApps /> : null}
    </Screen>
  );
}

function unattached(
  linked: SessionPeer[],
  live: SessionPeer[],
  found: Endpoint[],
): { tone: Tone; title: string; subtitle: string } {
  if (live.length > 1) {
    return { tone: 'warn', title: 'more than one daemon is linked', subtitle: 'pick the one this app talks to below' };
  }
  if (linked.length > 0) {
    return { tone: 'err', title: 'the link did not open', subtitle: linked[0]?.linkError ?? 'pick it again below' };
  }
  return {
    tone: 'warn',
    title: 'no Car Thing connected',
    subtitle: found.length > 0 ? 'pick one of the daemons below' : 'browsing the network for a daemon',
  };
}

function KnownDevices(): VNode | null {
  const session = useDesktop();
  const known = knownDevices.data.value;
  const linked = peers.value;
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  if (known.length === 0) return null;

  const act = async (id: string, run: () => Promise<void>) => {
    setBusy(id);
    setFailure(null);
    try {
      await run();
      await knownDevices.refresh();
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Section>
      <SectionHeader title="remembered" hint="daemons this computer has linked to before" />
      <ListGroup>
        {known.map(device => {
          const attached = linked.some(peer => peer.id === device.url);
          return (
            <ListRow
              key={device.id}
              icon={<Icon name="device" />}
              iconTint={attached ? 'accent' : 'default'}
              title={device.name}
              subtitle={device.url}
              value={attached ? 'linked' : since(device.lastConnectedAt)}
              trailing={
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Icon name="trash" size={13} />}
                  disabled={busy === device.id}
                  onClick={() => void act(device.id, () => session.forgetKnownDevice(device.id))}>
                  forget
                </Button>
              }
            />
          );
        })}
      </ListGroup>
      <Hint>
        every daemon on the link is connected as it appears; disconnect one to leave it alone until it is unplugged and
        back.
      </Hint>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}

function PeerRow({ peer, meta, active }: { peer: SessionPeer; meta: DeviceMetaEntry[]; active: boolean }): VNode {
  const session = useSession();
  const desktop = useDesktop();
  const held = meta.find(entry => entry.deviceId === peer.id)?.meta ?? null;
  const connected = peer.status === 'connected';

  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const submit = async () => {
    setSaving(true);
    setFailure(null);
    try {
      await session.setDeviceNickname(draft.trim());
      setRenaming(false);
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setSaving(false);
    }
  };

  return (
    <>
      <Dialog
        open={renaming}
        onClose={() => setRenaming(false)}
        title="rename this Car Thing"
        subtitle="the name shows on the device and wherever it is listed"
        footer={
          <>
            <Button variant="ghost" onClick={() => setRenaming(false)}>
              cancel
            </Button>
            <Button variant="primary" loading={saving} onClick={() => void submit()}>
              rename
            </Button>
          </>
        }>
        <Field
          label="name"
          value={draft}
          onInput={setDraft}
          onCommit={() => void submit()}
          placeholder={peer.name}
          clearable
        />
        {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      </Dialog>

      <ListRow
        icon={<Icon name="plug" />}
        iconTint={connected ? (active ? 'accent' : 'default') : 'err'}
        title={held?.nickname ?? peer.name}
        subtitle={
          connected
            ? held
              ? `${held.modelName} · ${held.serialNumber} · ${peerHost(peer.id)}`
              : `reading device info from ${peerHost(peer.id)}`
            : (peer.linkError ?? 'attached, but the link did not open')
        }
        value={held ? `v${held.daemonVersion}+${held.imageVersion}` : undefined}
        trailing={
          connected ? (
            <span class="flex shrink-0 items-center gap-2">
              <Pill tone={active ? 'ok' : 'neutral'} dot={active}>
                {held?.channel ?? 'connected'}
              </Pill>
              {active ? <Icon name="pencil" size={14} class="text-dim" /> : <Pill tone="accent">use</Pill>}
            </span>
          ) : (
            <Pill tone="err">link failed</Pill>
          )
        }
        onClick={
          connected
            ? active
              ? () => {
                  setDraft(held?.nickname ?? '');
                  setFailure(null);
                  setRenaming(true);
                }
              : () => void desktop.selectDevice(peer.id)
            : undefined
        }
      />
    </>
  );
}

function InstalledApps(): VNode {
  const { route } = useLocation();
  const list = listedWebapps(webapps.data.value);
  const active = webappActive.data.value;
  const failure = webapps.error.value;

  return (
    <Section>
      <SectionHeader
        title="installed apps"
        hint="the one in accent is on screen now"
        action="refresh"
        pending={webapps.pending.value}
        onAction={webapps.refresh}
      />
      {failure ? (
        <SectionEmpty>{failure}</SectionEmpty>
      ) : (
        <div class="grid grid-cols-[repeat(auto-fill,minmax(9rem,1fr))] border-t border-l border-rule">
          {list.map(webapp => {
            const running = active?.id === webapp.id;
            return (
              <button
                key={webapp.id}
                type="button"
                class={cx(
                  'flex min-w-0 flex-col items-center gap-2.5 border-r border-b border-rule px-3 py-5 transition-colors duration-150 focus-visible:outline-2 focus-visible:outline-accent focus-visible:-outline-offset-2',
                  running ? 'bg-accent-soft' : 'bg-screen hover:bg-neutral-soft active:bg-rule',
                )}
                onClick={() => route(PATHS.app(webapp.id))}>
                <WebappIcon id={webapp.id} iconHash={webapp.iconHash} name={webapp.name} size="lg" />
                <span
                  class={cx(
                    'line-clamp-2 w-full text-center wrap-break-word text-hint',
                    running ? 'text-accent' : 'text-off-white',
                  )}>
                  {webapp.name}
                </span>
                <span class="font-mono text-eyebrow text-dim uppercase">{running ? 'active' : webapp.source}</span>
              </button>
            );
          })}
          <button
            type="button"
            class="flex min-w-0 flex-col items-center justify-center gap-2.5 border-r border-b border-rule bg-screen px-3 py-5 text-dim transition-colors duration-150 hover:bg-neutral-soft hover:text-off-white active:bg-rule focus-visible:outline-2 focus-visible:outline-accent focus-visible:-outline-offset-2"
            onClick={() => route(PATHS.store)}>
            <Icon name="plus" size={22} />
            <span class="text-hint">add app</span>
          </button>
        </div>
      )}
      {list.length === 0 && !webapps.pending.value && !failure ? (
        <Hint>this Car Thing reports no webapps yet.</Hint>
      ) : null}
    </Section>
  );
}
