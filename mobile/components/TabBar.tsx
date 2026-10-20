import type { BottomTabBarProps } from '@react-navigation/bottom-tabs';
import { useEffect, useState } from 'react';
import { Text, View } from 'react-native';
import Animated, {
  useAnimatedStyle,
  useSharedValue,
  withSpring,
} from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { Icon, type IconName } from './Icon';
import { Press } from './Press';
import { StatusLine } from './StatusLine';
import { TEXT, usePalette } from '../lib/theme';
import type { TabName } from '../navigation';

const TAB_ICON: Record<TabName, IconName> = {
  apps: 'LayoutGrid',
  store: 'Store',
  settings: 'Settings2',
};

const INDICATOR_INSET = 24;

const SPRING = { damping: 22, stiffness: 320, mass: 0.7 };

export function TabBar({ state, navigation }: BottomTabBarProps) {
  const insets = useSafeAreaInsets();
  const palette = usePalette();

  const [barWidth, setBarWidth] = useState(0);
  const index = useSharedValue(state.index);

  useEffect(() => {
    index.value = withSpring(state.index, SPRING);
  }, [index, state.index]);

  const cellWidth = barWidth / state.routes.length;

  const indicatorStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: index.value * cellWidth + INDICATOR_INSET }],
  }));

  return (
    <View className="bg-bg" style={{ paddingBottom: insets.bottom }}>
      <StatusLine />
      <View
        className="flex-row border-t border-rule"
        onLayout={e => setBarWidth(e.nativeEvent.layout.width)}
      >
        {barWidth > 0 ? (
          <Animated.View
            style={[
              {
                position: 'absolute',
                top: -1,
                height: 2,
                width: Math.max(cellWidth - 2 * INDICATOR_INSET, 0),
                backgroundColor: palette.accent,
              },
              indicatorStyle,
            ]}
          />
        ) : null}
        {state.routes.map((route, routeIndex) => {
          const focused = state.index === routeIndex;
          const name = route.name as TabName;
          return (
            <Press
              key={route.key}
              testID={`tab-${name}`}
              accessibilityRole="button"
              accessibilityState={focused ? { selected: true } : {}}
              className="flex-1 items-center gap-1.5 pb-2 pt-2.5"
              onPress={() => {
                const event = navigation.emit({
                  type: 'tabPress',
                  target: route.key,
                  canPreventDefault: true,
                });
                if (!focused && !event.defaultPrevented)
                  navigation.navigate(route.name, route.params);
              }}
            >
              <Icon
                name={TAB_ICON[name]}
                size={22}
                tone={focused ? 'accent' : 'neutral'}
              />
              <Text
                className={`font-mono ${focused ? 'text-accent' : 'text-soft'}`}
                style={TEXT.hint}
              >
                {name}
              </Text>
            </Press>
          );
        })}
      </View>
    </View>
  );
}
