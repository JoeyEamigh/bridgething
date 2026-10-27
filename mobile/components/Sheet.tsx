import type { ReactNode } from 'react';
import { useEffect, useState } from 'react';
import { Modal, Pressable, StyleSheet, Text } from 'react-native';
import Animated, {
  runOnJS,
  useAnimatedKeyboard,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
} from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { TEXT, usePalette } from '../lib/theme';

export function SheetTitle({ children }: { children: string }) {
  return (
    <Text
      className="font-sans text-fg"
      style={TEXT.title}
      accessibilityRole="header"
    >
      {children}
    </Text>
  );
}

export function Sheet({
  visible,
  onClose,
  children,
}: {
  visible: boolean;
  onClose: () => void;
  children: ReactNode;
}) {
  const insets = useSafeAreaInsets();
  const palette = usePalette();
  const [render, setRender] = useState(visible);
  const progress = useSharedValue(visible ? 1 : 0);
  const keyboard = useAnimatedKeyboard();

  useEffect(() => {
    if (visible) {
      setRender(true);
      progress.value = withTiming(1, { duration: 200 });
    } else {
      progress.value = withTiming(0, { duration: 150 }, finished => {
        if (finished) runOnJS(setRender)(false);
      });
    }
  }, [visible, progress]);

  const backdropStyle = useAnimatedStyle(() => ({ opacity: progress.value }));
  const cardStyle = useAnimatedStyle(() => ({
    opacity: progress.value,
    transform: [{ translateY: (1 - progress.value) * 48 }],
  }));
  const frameStyle = useAnimatedStyle(() => ({
    paddingTop: insets.top + 16,
    paddingBottom: keyboard.height.value,
  }));
  const bodyStyle = useAnimatedStyle(() => ({
    paddingBottom: 24 + Math.max(insets.bottom - keyboard.height.value, 0),
  }));

  if (!render) return null;

  return (
    <Modal
      transparent
      visible
      animationType="none"
      statusBarTranslucent
      onRequestClose={onClose}
    >
      <Animated.View
        className="flex-1 items-center justify-end"
        style={frameStyle}
      >
        <Pressable style={StyleSheet.absoluteFill} onPress={onClose}>
          <Animated.View
            style={[
              StyleSheet.absoluteFill,
              { backgroundColor: palette.scrim },
              backdropStyle,
            ]}
          />
        </Pressable>
        <Animated.View className="w-full max-w-[560px]" style={cardStyle}>
          <Animated.View
            className="gap-4 border-t border-rule-strong bg-bg px-6 pt-6"
            style={bodyStyle}
          >
            {children}
          </Animated.View>
        </Animated.View>
      </Animated.View>
    </Modal>
  );
}
