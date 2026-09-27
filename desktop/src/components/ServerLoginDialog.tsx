import type { ProviderInfo } from '@bridgething/companion-types';
import { Button, Dialog, Field, describeError } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { useDesktop } from '../desktop.ts';
import { ErrorNote } from './Screen.tsx';

export function ServerLoginDialog({
  provider,
  open,
  onClose,
}: {
  provider: ProviderInfo;
  open: boolean;
  onClose: () => void;
}): VNode {
  const session = useDesktop();
  const [serverUrl, setServerUrl] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setFailure(null);
    try {
      await session.completeProviderAuth(provider.id, {
        kind: 'serverLogin',
        serverUrl,
        username,
        password,
      });
      setPassword('');
      onClose();
    } catch (error: unknown) {
      setFailure(describeError(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={`sign in to ${provider.displayName}`}
      subtitle="the address of your server and the account it knows you by"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            cancel
          </Button>
          <Button variant="primary" loading={busy} onClick={() => void submit()}>
            sign in
          </Button>
        </>
      }>
      <div class="flex flex-col gap-3">
        <Field
          label="server"
          type="url"
          value={serverUrl}
          onInput={setServerUrl}
          placeholder="https://music.example.com"
          hint="https is assumed when the address has no scheme"
        />
        <Field label="username" value={username} onInput={setUsername} />
        <Field label="password" type="password" value={password} onInput={setPassword} onCommit={() => void submit()} />
        {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      </div>
    </Dialog>
  );
}
