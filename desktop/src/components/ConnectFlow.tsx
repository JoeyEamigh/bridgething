import {
  Button,
  cx,
  describeError,
  Field,
  ListGroup,
  ListRow,
  SectionEmpty,
  SectionHeader,
  Spinner,
  useSession,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { Icon } from '../lib/icons.tsx';
import { defaultGateway, endpoints, peers } from '../stores/session.ts';
import { ErrorNote, Section } from './Screen.tsx';

export function ConnectFlow(): VNode {
  const session = useSession();
  const linked = peers.value;
  const found = endpoints.data.value.filter(endpoint => !linked.some(peer => peer.id === endpoint.url));
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
        <SectionHeader title="nearby" action="rescan" pending={scanning} onAction={endpoints.refresh} />
        {scanning && found.length === 0 ? (
          <SectionEmpty>
            <Spinner class="mx-auto" />
          </SectionEmpty>
        ) : found.length === 0 ? (
          <SectionEmpty>none found</SectionEmpty>
        ) : (
          <ListGroup>
            {found.map(endpoint => (
              <ListRow
                key={endpoint.id}
                icon={<Icon name="device" />}
                title={endpoint.nickname ?? endpoint.host}
                subtitle={endpoint.url}
                trailing={busy === endpoint.url ? <Spinner /> : undefined}
                chevron
                disabled={busy !== null}
                onClick={() => void connect(endpoint.url)}
              />
            ))}
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

  const offered =
    standing !== null &&
    !found.some(endpoint => endpoint.url === standing) &&
    !linked.some(peer => peer.id === standing);
  const typed = draft.trim();

  const submit = () => {
    if (typed.length > 0) void onConnect(typed);
  };

  return (
    <Section>
      <SectionHeader title="by address" />
      {offered ? (
        <ListGroup>
          <ListRow
            icon={<Icon name="plug" />}
            title="this computer"
            subtitle={standing}
            trailing={busy === standing ? <Spinner /> : undefined}
            chevron
            disabled={busy !== null}
            onClick={() => void onConnect(standing)}
          />
        </ListGroup>
      ) : null}
      <div class={cx('flex items-end gap-2', offered && 'mt-3')}>
        <Field
          value={draft}
          onInput={setDraft}
          onSubmit={submit}
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
