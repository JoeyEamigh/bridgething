import type { WebappSlotName } from '@bridgething/catalog';
import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from './Button';
import { Icon } from './Icon';
import { Note } from './Note';
import { TEXT } from '../lib/theme';
import { assignSlot, heldBy, useSlots } from '../lib/webapps';

const LABEL: Record<WebappSlotName, string> = {
  launcher: 'home screen',
  overlay: 'system overlay',
};

export function SlotAction({
  deviceId,
  id,
  slot,
}: {
  deviceId: string;
  id: string;
  slot: WebappSlotName;
}) {
  const { slots } = useSlots(deviceId);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const held = heldBy(slots, slot);
  const mine = held?.toLowerCase() === id.toLowerCase();

  const assign = async (next?: string) => {
    setBusy(true);
    setFailure(null);
    try {
      await assignSlot(deviceId, slot, next);
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <View className="gap-2">
      {mine ? (
        <>
          <View className="flex-row items-center justify-center gap-2 border border-ok bg-ok-soft px-5 py-2.5">
            <Icon name="Check" tone="ok" size={17} />
            <Text className="font-mono text-ok" style={TEXT.body}>
              your {LABEL[slot]}
            </Text>
          </View>
          <Button
            onPress={() => void assign()}
            loading={busy}
            variant="ghost"
            icon="RotateCcw"
          >
            back to the built-in {LABEL[slot]}
          </Button>
        </>
      ) : (
        <Button
          onPress={() => void assign(id)}
          loading={busy}
          variant="secondary"
          icon={slot === 'launcher' ? 'LayoutGrid' : 'Layers'}
        >
          use as {LABEL[slot]}
        </Button>
      )}
      {failure ? <Note tone="err">{failure}</Note> : null}
    </View>
  );
}
