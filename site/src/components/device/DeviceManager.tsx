import type { DeviceMeta, SessionPeer } from '@bridgething/companion-types';
import { Button, Field, ListGroup, ListRow, ScreenHeader, SectionHeader, SessionProvider } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';

import { BrowserSession, message } from '../../lib/browser-session';
import { useBrowser, useBrowserQuery } from '../../lib/browser-tier';
import { takePendingInstall, type PendingInstall } from '../../lib/pending-install';
import { Connect } from './Connect';
import { AddApp, StagedInstall } from './Install';
import { ErrorNote, Screen, Section } from './Screen';
import { Update } from './Update';
import { Webapps } from './Webapps';

export function DeviceManager(): VNode {
  const session = useMemo(() => new BrowserSession(), []);

  return (
    <SessionProvider session={session}>
      <Console />
    </SessionProvider>
  );
}

function Console(): VNode {
  const peers = useBrowserQuery(['peers'], s => s.peers());
  const meta = useBrowserQuery(['device-meta'], s => s.deviceMeta());
  const [pending, setPending] = useState<PendingInstall | null>(null);

  useEffect(() => {
    setPending(takePendingInstall());
  }, []);

  const linked = (peers.data ?? [])[0] ?? null;
  const held = meta.data?.[0]?.meta ?? null;
  const lib = held?.libbridgethingVersion ?? null;
  const serial = held?.serialNumber ?? null;

  if (!linked) {
    return (
      <Screen>
        <Connect pending={pending} />
      </Screen>
    );
  }

  return (
    <Screen>
      <Header peer={linked} meta={held} />
      <Update channel={held?.channel ?? null} />
      {pending ? (
        <StagedInstall pending={pending} libVersion={lib} serial={serial} onDone={() => setPending(null)} />
      ) : null}
      <Webapps />
      <AddApp libVersion={lib} />
      <DeviceInfo meta={held} />
    </Screen>
  );
}

function Header({ peer, meta }: { peer: SessionPeer; meta: DeviceMeta | null }): VNode {
  const session = useBrowser();

  return (
    <ScreenHeader
      class="mb-0"
      eyebrow="connected"
      title={meta?.nickname || meta?.modelName || peer.name}
      subtitle={
        meta
          ? `daemon ${meta.daemonVersion} · image ${meta.imageVersion} · ${meta.channel}`
          : 'the link is up, but the device never announced its version'
      }
      trailing={
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            void session.disconnect();
          }}>
          disconnect
        </Button>
      }
    />
  );
}

function DeviceInfo({ meta }: { meta: DeviceMeta | null }): VNode {
  const session = useBrowser();
  const [draft, setDraft] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const rename = async (value: string) => {
    setFailure(null);
    try {
      await session.setDeviceNickname(value.trim());
      setDraft(null);
    } catch (reason) {
      setFailure(message(reason));
    }
  };

  const rows: [string, string][] = meta
    ? [
        ['model', meta.modelName],
        ['serial', meta.serialNumber],
        ['daemon', meta.daemonVersion],
        ['libbridgething', meta.libbridgethingVersion],
        ['image', meta.imageVersion],
        ['channel', meta.channel],
        ['os', `${meta.osName} ${meta.osVersion}`],
      ]
    : [];

  return (
    <Section>
      <SectionHeader title="this device" hint="what the daemon says about itself" />
      <Field
        class="mb-4 max-w-md"
        label="nickname"
        placeholder="e.g. living room"
        value={draft ?? meta?.nickname ?? ''}
        onInput={setDraft}
        onCommit={value => void rename(value)}
        hint="shown on the device and wherever it is listed"
        clearable
      />
      <ListGroup>
        {rows.map(([label, value]) => (
          <ListRow key={label} title={label} value={value} />
        ))}
      </ListGroup>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}
