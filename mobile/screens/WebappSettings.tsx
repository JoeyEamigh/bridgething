import { useCallback, useEffect, useRef } from 'react';
import { Text, View } from 'react-native';

import { Note } from '../components/Note';
import { Spinner } from '../components/Spinner';
import {
  hideSettings,
  onSettingsDone,
  retrySettings,
  revealSettings,
  selectSettings,
  settingsAbsent,
  settingsReady,
  useSettingsHostStore,
} from '../lib/settings-host';
import { TEXT } from '../lib/theme';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'WebappSettings'>;

export function WebappSettingsScreen({ navigation, route }: Props) {
  const { deviceId, id, name } = route.params;
  const placeholder = useRef<View>(null);
  const appeared = useRef(false);

  const failure = useSettingsHostStore(s => s.failure);
  const ready = useSettingsHostStore(settingsReady);
  const absent = useSettingsHostStore(settingsAbsent);

  const finish = useCallback(() => navigation.goBack(), [navigation]);

  useEffect(() => {
    navigation.setOptions({ title: `${name} settings` });
  }, [name, navigation]);

  const measure = useCallback(() => {
    placeholder.current?.measureInWindow((x, y, width, height) =>
      revealSettings({ x, y, width, height }),
    );
  }, []);

  useEffect(() => {
    selectSettings({ deviceId, id });
    const unsubscribe = [
      onSettingsDone(finish),
      navigation.addListener('focus', () => {
        selectSettings({ deviceId, id });
        if (appeared.current) measure();
      }),
      navigation.addListener('blur', hideSettings),
      navigation.addListener('transitionStart', event => {
        if (event.data.closing) hideSettings();
      }),
      navigation.addListener('transitionEnd', event => {
        if (event.data.closing) return;
        appeared.current = true;
        measure();
      }),
    ];
    return () => {
      for (const off of unsubscribe) off();
      hideSettings();
    };
  }, [deviceId, finish, id, measure, navigation]);

  return (
    <View
      ref={placeholder}
      className="flex-1 bg-bg"
      onLayout={() => {
        if (useSettingsHostStore.getState().frame) measure();
      }}
    >
      {failure ? (
        <View className="p-4">
          <Note tone="err" action="retry" onAction={retrySettings}>
            {failure}
          </Note>
        </View>
      ) : ready ? null : absent ? (
        <View className="p-4">
          <Note tone="warn">{`${name} is not on a connected car thing`}</Note>
        </View>
      ) : (
        <View className="flex-1 items-center justify-center gap-3">
          <Spinner />
          <Text className="font-mono text-dim" style={TEXT.hint}>
            fetching settings from your car thing
          </Text>
        </View>
      )}
    </View>
  );
}
