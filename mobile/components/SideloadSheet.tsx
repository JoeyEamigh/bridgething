import type { BridgethingWebappInfo } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { View } from 'react-native';

import { Button } from './Button';
import { Field } from './Field';
import { Note } from './Note';
import { Sheet, SheetTitle } from './Sheet';
import { getSession } from '../lib/session';
import { installPickedWebapp } from '../lib/webapps';

type Busy = 'url' | 'file' | null;

export function SideloadSheet({
  visible,
  deviceId,
  onClose,
}: {
  visible: boolean;
  deviceId: string | null;
  onClose: () => void;
}) {
  const [url, setUrl] = useState('');
  const [busy, setBusy] = useState<Busy>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [installed, setInstalled] = useState<string | null>(null);

  const run = async (
    kind: Exclude<Busy, null>,
    work: (deviceId: string) => Promise<BridgethingWebappInfo | null>,
  ) => {
    if (!deviceId || busy) return;
    setBusy(kind);
    setFailure(null);
    setInstalled(null);
    try {
      const info = await work(deviceId);
      if (info) {
        setInstalled(`${info.name} ${info.version}`);
        setUrl('');
      }
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setBusy(null);
    }
  };

  const close = () => {
    setFailure(null);
    setInstalled(null);
    onClose();
  };

  const trimmed = url.trim();

  return (
    <Sheet visible={visible} onClose={close}>
      <SheetTitle>install from url or file</SheetTitle>

      <Field
        label="url"
        icon="Link"
        value={url}
        onChangeText={setUrl}
        clearable
        editable={deviceId != null}
        placeholder="https://example.com/my-app.zip"
        autoCapitalize="none"
        autoCorrect={false}
        keyboardType="url"
      />

      {deviceId == null ? (
        <Note tone="warn">connect a car thing to install onto it</Note>
      ) : null}
      {installed ? <Note tone="ok">{`installed ${installed}`}</Note> : null}
      {failure ? <Note tone="err">{failure}</Note> : null}

      <View className="gap-2">
        <Button
          onPress={() =>
            void run('url', id =>
              getSession().installWebappFromUri(id, trimmed),
            )
          }
          loading={busy === 'url'}
          disabled={deviceId == null || trimmed.length === 0 || busy != null}
          icon="Download"
        >
          install
        </Button>
        <Button
          onPress={() => void run('file', id => installPickedWebapp(id))}
          loading={busy === 'file'}
          disabled={deviceId == null || busy != null}
          variant="secondary"
          icon="FolderOpen"
        >
          choose a zip
        </Button>
        <Button variant="ghost" onPress={close}>
          close
        </Button>
      </View>
    </Sheet>
  );
}
