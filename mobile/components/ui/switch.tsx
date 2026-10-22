import { useEffect } from 'react';
import { Pressable } from 'react-native';
import Animated, {
  interpolateColor,
  useAnimatedStyle,
  useSharedValue,
  withSpring,
  withTiming,
} from 'react-native-reanimated';

import { usePalette } from '../../lib/theme';

const TRACK_W = 44;
const TRACK_H = 26;
const BORDER = 1;
const INSET = 3;
const THUMB = TRACK_H - 2 * BORDER - 2 * INSET;
const TRAVEL = TRACK_W - 2 * BORDER - 2 * INSET - THUMB;

const SPRING = { damping: 20, stiffness: 320, mass: 0.6 };

function Switch({
  value,
  onValueChange,
  disabled,
  testID,
}: {
  value: boolean;
  onValueChange: (next: boolean) => void;
  disabled?: boolean;
  testID?: string;
}) {
  const palette = usePalette();
  const progress = useSharedValue(value ? 1 : 0);

  useEffect(() => {
    progress.value = withSpring(value ? 1 : 0, SPRING);
  }, [progress, value]);

  const trackStyle = useAnimatedStyle(() => ({
    backgroundColor: interpolateColor(
      progress.value,
      [0, 1],
      [palette.neutralSoft, palette.accentSoft],
    ),
    borderColor: interpolateColor(
      progress.value,
      [0, 1],
      [palette.edge, palette.accent],
    ),
    opacity: withTiming(disabled ? 0.4 : 1, { duration: 120 }),
  }));

  const thumbStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: progress.value * TRAVEL }],
    backgroundColor: interpolateColor(
      progress.value,
      [0, 1],
      [palette.dim, palette.accent],
    ),
  }));

  return (
    <Pressable
      accessibilityRole="switch"
      testID={testID}
      accessibilityState={{ checked: value, disabled: disabled ?? false }}
      disabled={disabled}
      hitSlop={8}
      onPress={() => onValueChange(!value)}
    >
      <Animated.View
        style={[
          {
            width: TRACK_W,
            height: TRACK_H,
            borderWidth: BORDER,
            padding: INSET,
            justifyContent: 'center',
          },
          trackStyle,
        ]}
      >
        <Animated.View style={[{ width: THUMB, height: THUMB }, thumbStyle]} />
      </Animated.View>
    </Pressable>
  );
}

export { Switch };
