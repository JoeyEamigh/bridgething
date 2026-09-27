import {
  Button,
  Field,
  ListGroup,
  ListRow,
  Pill,
  SectionEmpty,
  SectionHeader,
  Spinner,
  describeError,
  useSession,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { Icon } from '../lib/icons.tsx';
import { defaultGateway, endpoints, peers } from '../stores/session.ts';
import { ErrorNote, Section } from './Screen.tsx';

export function ConnectFlow(): VNode {
  const session = useSession();
  const found = endpoints.data.value;
  const linked = peers.value;
  const scanning = endpoints.pending.value;

  const [busy, setBusy] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const connect = async (url: string) => {
    setBusy(url);
    setFailure(null);
    try {
      await session.connect(url);
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}

      <Section>
        <SectionHeader
          title="nearby"
          hint="network or paired Bluetooth"
          action="rescan"
          pending={scanning}
          onAction={endpoints.refresh}
        />
        {scanning && found.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : found.length === 0 ? (
          <SectionEmpty>no Car Thing found. check its power, network, or Bluetooth pairing.</SectionEmpty>
        ) : (
          <ListGroup>
            {found.map(endpoint => {
              const attached = linked.some(peer => peer.id === endpoint.url);
              return (
                <ListRow
                  key={endpoint.id}
                  icon={<Icon name="device" />}
                  iconTint={attached ? 'accent' : 'default'}
                  title={endpoint.nickname ?? endpoint.host}
                  subtitle={endpoint.url}
                  trailing={attached ? <Pill tone="ok">linked</Pill> : busy === endpoint.url ? <Spinner /> : undefined}
                  chevron={!attached}
                  disabled={busy !== null}
                  onClick={attached ? undefined : () => void connect(endpoint.url)}
                />
              );
            })}
          </ListGroup>
        )}
        {endpoints.error.value ? <ErrorNote>{endpoints.error.value}</ErrorNote> : null}
      </Section>

      <DirectConnect busy={busy} onConnect={connect} />
    </>
  );
}

function DirectConnect({ busy, onConnect }: { busy: string | null; onConnect: (url: string) => Promise<void> }): VNode {
  const found = endpoints.data.value;
  const linked = peers.value;
  const standing = defaultGateway.data.value;
  const [draft, setDraft] = useState('');

  const announced = standing !== null && found.some(endpoint => endpoint.url === standing);
  const attached = standing !== null && linked.some(peer => peer.id === standing);
  const typed = draft.trim();

  const submit = () => {
    if (typed.length > 0) void onConnect(typed);
  };

  return (
    <Section>
      <SectionHeader title="by hand" hint="for a daemon that does not announce itself" />
      {standing !== null && !announced ? (
        <ListGroup>
          <ListRow
            icon={<Icon name="plug" />}
            iconTint={attached ? 'accent' : 'default'}
            title="this computer"
            subtitle={standing}
            trailing={attached ? <Pill tone="ok">linked</Pill> : busy === standing ? <Spinner /> : undefined}
            chevron={!attached}
            disabled={busy !== null}
            onClick={attached ? undefined : () => void onConnect(standing)}
          />
        </ListGroup>
      ) : null}
      <div class="mt-3 flex items-end gap-2">
        <Field
          label="gateway url"
          value={draft}
          onInput={setDraft}
          onCommit={submit}
          placeholder="ws://bridgething.local:8892/"
          type="url"
          disabled={busy !== null}
          clearable
          class="flex-1"
        />
        <Button
          variant="primary"
          loading={busy === typed}
          disabled={typed.length === 0 || busy !== null}
          onClick={submit}>
          connect
        </Button>
      </div>
    </Section>
  );
}
