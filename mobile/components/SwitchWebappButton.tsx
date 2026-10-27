import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from './Button';
import { Icon } from './Icon';
import { getSession } from '../lib/session';
import { TEXT } from '../lib/theme';
import { useWebapps } from '../lib/webapps';

export function SwitchWebappButton({
  deviceId,
  id,
  onError,
}: {
  deviceId: string;
  id: string;
  onError: (message: string | null) => void;
}) {
  const { active } = useWebapps(deviceId);
  const [switching, setSwitching] = useState(false);

  if (active?.id.toLowerCase() === id.toLowerCase()) {
    return (
      <View className="flex-row items-center justify-center gap-2 border border-ok bg-ok-soft px-5 py-2.5">
        <Icon name="Check" tone="ok" size={17} />
        <Text className="font-mono text-ok" style={TEXT.body}>
          active
        </Text>
      </View>
    );
  }

  const switchTo = async () => {
    setSwitching(true);
    onError(null);
    try {
      await getSession().switchWebapp(deviceId, id);
    } catch (err) {
      onError(describeError(err));
    } finally {
      setSwitching(false);
    }
  };

  return (
    <Button
      onPress={() => void switchTo()}
      loading={switching}
      variant="primary"
      icon="Play"
    >
      switch to this
    </Button>
  );
}
