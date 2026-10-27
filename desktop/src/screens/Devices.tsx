import type { DeviceMetaEntry, SessionPeer } from '@bridgething/companion-types';
import {
  Button,
  describeError,
  Dialog,
  type Endpoint,
  Field,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionHeader,
  StatusStrip,
  type Tone,
  useSession,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useLocation } from 'preact-iso';
import { useState } from 'preact/hooks';

import { deviceName } from '@bridgething/client';
import { ConnectFlow } from '../components/ConnectFlow.tsx';
import { DeviceSettings } from '../components/DeviceSettings.tsx';
import { OtaRunCard } from '../components/OtaRunCard.tsx';
import { Disclosure, ErrorNote, Screen, Section } from '../components/Screen.tsx';
import { useDesktop } from '../desktop.ts';
import { peerHost, since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
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

  const run = primary
    ? otaRuns.value.find(entry => entry.deviceId === primary.id && entry.lane === 'firmware')
    : undefined;
  const offered = primary ? otaAvailable.value.find(entry => entry.deviceId === primary.id) : undefined;

  return (
    <Screen>
      <ScreenHeader
        title="devices"
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

      {run && run.outcome !== 'succeeded' ? (
        <Section>
          <SectionHeader title="update" action="releases" onAction={() => route(PATHS.updates)} />
          <OtaRunCard
            run={run}
            onDismiss={() => {
              void session.dismissOtaRun(run.lane);
            }}
          />
        </Section>
      ) : offered?.releaseVersion ? (
        <StatusStrip
          tone="accent"
          title={`${offered.releaseVersion} available`}
          onClick={() => route(PATHS.updates)}
          class="mb-6"
        />
      ) : null}

      {linked.length > 0 ? (
        <Section>
          <SectionHeader title="linked" />
          <ListGroup>
            {linked.map(peer => (
              <PeerRow
                key={peer.id}
                peer={peer}
                meta={meta}
                active={peer.id === primary?.id}
                choosable={live.length > 1}
              />
            ))}
          </ListGroup>
          {primary ? <OnScreen /> : null}
        </Section>
      ) : null}

      {primary ? (
        <Section>
          <SectionHeader title="settings" />
          <DeviceSettings />
        </Section>
      ) : null}

      <OfflineDevices />

      <AddDevice linked={linked.length > 0} />
    </Screen>
  );
}

function OnScreen(): VNode | null {
  const { route } = useLocation();
  const active = webappActive.data.value;
  const app = active ? webapps.data.value.find(entry => entry.id === active.id) : undefined;
  if (!app) return null;

  return (
    <ListGroup class="mt-2">
      <ListRow
        icon={<WebappIcon id={app.id} iconHash={app.iconHash} name={app.name} size="sm" />}
        title={app.name}
        subtitle="on screen"
        chevron
        onClick={() => route(PATHS.app(app.id))}
      />
    </ListGroup>
  );
}

function AddDevice({ linked }: { linked: boolean }): VNode {
  if (!linked) return <ConnectFlow />;

  return (
    <Disclosure title="add a device">
      <ConnectFlow />
    </Disclosure>
  );
}

function unattached(
  linked: SessionPeer[],
  live: SessionPeer[],
  found: Endpoint[],
): { tone: Tone; title: string; subtitle?: string } {
  if (live.length > 1) {
    return { tone: 'warn', title: 'pick a device' };
  }
  if (linked.length > 0) {
    return { tone: 'err', title: 'link failed', subtitle: linked[0]?.linkError ?? undefined };
  }
  return {
    tone: 'warn',
    title: 'no Car Thing connected',
    subtitle: found.length > 0 ? undefined : 'searching',
  };
}

function OfflineDevices(): VNode | null {
  const session = useDesktop();
  const linked = peers.value;
  const offline = knownDevices.data.value.filter(device => !linked.some(peer => peer.id === device.url));
  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  if (offline.length === 0) return null;

  const forget = async (id: string) => {
    setBusy(id);
    setFailure(null);
    try {
      await session.forgetKnownDevice(id);
      await knownDevices.refresh();
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Section>
      <SectionHeader title="offline" />
      <ListGroup>
        {offline.map(device => (
          <ListRow
            key={device.id}
            icon={<Icon name="device" />}
            title={device.name}
            subtitle={device.url}
            value={since(device.lastConnectedAt)}
            trailing={
              <Button
                size="sm"
                variant="ghost"
                icon={<Icon name="trash" size={13} />}
                disabled={busy === device.id}
                onClick={() => void forget(device.id)}>
                forget
              </Button>
            }
          />
        ))}
      </ListGroup>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}

function PeerRow({
  peer,
  meta,
  active,
  choosable,
}: {
  peer: SessionPeer;
  meta: DeviceMetaEntry[];
  active: boolean;
  choosable: boolean;
}): VNode {
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
        title="rename"
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
          onSubmit={() => void submit()}
          placeholder={peer.name}
          clearable
        />
        {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      </Dialog>

      <ListRow
        icon={<Icon name="plug" />}
        iconTint={connected ? (active ? 'accent' : 'default') : 'err'}
        title={held ? deviceName(held) : peer.name}
        subtitle={
          connected
            ? held
              ? `${held.serialNumber} · ${peerHost(peer.id)}`
              : peerHost(peer.id)
            : (peer.linkError ?? 'link failed')
        }
        value={held ? `${held.daemonVersion}+image.${held.imageVersion}` : undefined}
        trailing={
          connected ? (
            <span class="flex shrink-0 items-center gap-2">
              <Pill tone={active ? 'ok' : 'neutral'} dot={active}>
                {held?.channel ?? 'connected'}
              </Pill>
              {active ? (
                <Icon name="pencil" size={14} class="text-dim" />
              ) : choosable ? (
                <Pill tone="accent">use</Pill>
              ) : null}
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
