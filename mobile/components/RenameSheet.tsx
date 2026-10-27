import { useEffect, useState } from 'react';
import { View } from 'react-native';

import { Button } from './Button';
import { Field } from './Field';
import { Sheet, SheetTitle } from './Sheet';

export function RenameSheet({
  visible,
  title,
  initialValue,
  placeholder,
  onSubmit,
  onClose,
}: {
  visible: boolean;
  title: string;
  initialValue?: string;
  placeholder?: string;
  onSubmit: (value: string | null) => void;
  onClose: () => void;
}) {
  const [draft, setDraft] = useState(initialValue ?? '');

  useEffect(() => {
    if (visible) setDraft(initialValue ?? '');
  }, [visible, initialValue]);

  const submit = () => {
    const trimmed = draft.trim();
    onSubmit(trimmed === '' ? null : trimmed);
    onClose();
  };

  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetTitle>{title}</SheetTitle>
      <Field
        value={draft}
        onChangeText={setDraft}
        placeholder={placeholder}
        autoFocus
        returnKeyType="done"
        onSubmitEditing={submit}
        autoCapitalize="words"
        autoCorrect={false}
      />
      <View className="flex-row justify-end gap-2">
        <Button variant="ghost" size="md" full={false} onPress={onClose}>
          cancel
        </Button>
        <Button variant="primary" size="md" full={false} onPress={submit}>
          save
        </Button>
      </View>
    </Sheet>
  );
}
