import { Text, View } from 'react-native';

import { Button } from './Button';
import { Note } from './Note';
import { Sheet, SheetTitle } from './Sheet';
import { TEXT } from '../lib/theme';

type ConfirmProps = {
  title: string;
  body?: string;
  warning?: string | null;
  detail?: string;
  confirmLabel: string;
  cancelLabel?: string;
  destructive?: boolean;
  busy?: boolean;
  onConfirm: () => void;
};

export function ConfirmBody({
  title,
  body,
  warning,
  detail,
  confirmLabel,
  cancelLabel = 'cancel',
  destructive = false,
  busy = false,
  onConfirm,
  onCancel,
}: ConfirmProps & { onCancel: () => void }) {
  return (
    <>
      <View className="gap-2">
        <SheetTitle>{title}</SheetTitle>
        {body ? (
          <Text className="font-sans text-fg" style={TEXT.body}>
            {body}
          </Text>
        ) : null}
        {warning ? (
          <Note tone="warn" className="mt-1">
            {warning}
          </Note>
        ) : null}
        {detail ? (
          <Text className="font-mono text-dim" style={TEXT.hint}>
            {detail}
          </Text>
        ) : null}
      </View>
      <View className="flex-row justify-end gap-2">
        <Button variant="ghost" size="md" full={false} onPress={onCancel}>
          {cancelLabel}
        </Button>
        <Button
          variant={destructive ? 'destructive' : 'primary'}
          size="md"
          full={false}
          loading={busy}
          onPress={onConfirm}
        >
          {confirmLabel}
        </Button>
      </View>
    </>
  );
}

export function ConfirmSheet({
  visible,
  onClose,
  ...confirm
}: ConfirmProps & { visible: boolean; onClose: () => void }) {
  return (
    <Sheet visible={visible} onClose={onClose}>
      <ConfirmBody {...confirm} onCancel={onClose} />
    </Sheet>
  );
}
