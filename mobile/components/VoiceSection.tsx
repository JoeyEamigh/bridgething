import type { BridgethingVoiceModelState } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { View } from 'react-native';

import { ListGroup } from './ListGroup';
import { ListRow, type RowTint } from './ListRow';
import { Note } from './Note';
import { CapabilityRow } from './permissions/CapabilityRow';
import { SectionHeader } from './SectionHeader';
import {
  downloadVoiceModel,
  updateCapabilityFlags,
  useSession,
} from '../lib/session';

export function VoiceSection() {
  const flags = useSession(s => s.capabilityFlags);
  const model = useSession(s => s.voiceModel);
  const [failure, setFailure] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);

  const write = async (voiceModel: boolean) => {
    setFailure(null);
    try {
      await updateCapabilityFlags({ ...flags, voiceModel });
    } catch (err) {
      setFailure(describeError(err));
    }
  };

  const download = async () => {
    setStarting(true);
    setFailure(null);
    try {
      await downloadVoiceModel();
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setStarting(false);
    }
  };

  const action = flags.voiceModel ? downloadAction(model.status) : undefined;

  return (
    <View className="mb-8">
      <SectionHeader
        title="voice"
        action={action}
        onAction={() => void download()}
        pending={starting}
      />
      <ListGroup>
        <CapabilityRow
          capability="voiceModel"
          value={flags.voiceModel}
          onChange={next => void write(next)}
        />
        {flags.voiceModel ? (
          <ListRow
            icon="Download"
            iconTint={modelTint(model)}
            title="voice model"
            subtitle={modelLine(model)}
            value={modelValue(model)}
            loading={model.status === 'downloading'}
          />
        ) : null}
      </ListGroup>
      {failure ? (
        <Note tone="err" className="mt-2">
          {failure}
        </Note>
      ) : null}
    </View>
  );
}

function downloadAction(
  status: BridgethingVoiceModelState['status'],
): string | undefined {
  switch (status) {
    case 'absent':
      return 'download now';
    case 'failed':
      return 'retry download';
    case 'downloading':
    case 'ready':
      return undefined;
  }
}

function modelTint(model: BridgethingVoiceModelState): RowTint {
  switch (model.status) {
    case 'ready':
      return 'ok';
    case 'downloading':
      return 'accent';
    case 'failed':
      return 'warn';
    case 'absent':
      return 'default';
  }
}

function modelLine(model: BridgethingVoiceModelState): string {
  switch (model.status) {
    case 'downloading':
      return 'downloading over wi-fi';
    case 'ready':
      return 'installed and active';
    case 'failed':
      return `couldn't download · ${describeError(model.error)}`;
    case 'absent':
      return 'downloads on wi-fi, or start it now (~127 MB)';
  }
}

function modelValue(model: BridgethingVoiceModelState): string | undefined {
  if (model.status === 'ready') return model.version ?? undefined;
  if (model.status !== 'downloading') return undefined;
  const share =
    model.totalBytes > 0
      ? Math.min(99, Math.floor((model.receivedBytes / model.totalBytes) * 100))
      : 0;
  return `${share}%`;
}
