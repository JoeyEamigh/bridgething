import { useEffect, useRef, useState } from 'react';
import { PanResponder, View, type GestureResponderEvent } from 'react-native';

import { usePalette } from '../lib/theme';

const HEIGHT = 32;
const THUMB = 14;

function clamp(value: number, low: number, high: number): number {
  return Math.max(low, Math.min(high, value));
}

export function Slider({
  value,
  min,
  max,
  step = 1,
  onInput,
  onChange,
  disabled = false,
  label,
  testID,
}: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onInput?: (next: number) => void;
  onChange: (next: number) => void;
  disabled?: boolean;
  label: string;
  testID?: string;
}) {
  const palette = usePalette();
  const [width, setWidth] = useState(0);
  const [draft, setDraft] = useState(value);
  const dragging = useRef(false);
  const grantX = useRef(0);
  const latest = useRef({
    width,
    min,
    max,
    step,
    draft,
    value,
    onInput,
    onChange,
    disabled,
  });
  latest.current = {
    width,
    min,
    max,
    step,
    draft,
    value,
    onInput,
    onChange,
    disabled,
  };

  useEffect(() => {
    if (!dragging.current) setDraft(value);
  }, [value]);

  const snap = (x: number): number => {
    const { width: w, min: lo, max: hi, step: by } = latest.current;
    const ratio = clamp((x - THUMB / 2) / Math.max(1, w - THUMB), 0, 1);
    return clamp(lo + Math.round((ratio * (hi - lo)) / by) * by, lo, hi);
  };

  const move = (x: number) => {
    const next = snap(x);
    if (next === latest.current.draft) return;
    latest.current.draft = next;
    setDraft(next);
    latest.current.onInput?.(next);
  };

  const commit = () => {
    dragging.current = false;
    const { draft: next, value: held } = latest.current;
    if (next !== held) latest.current.onChange(next);
  };

  const responder = useRef(
    PanResponder.create({
      onStartShouldSetPanResponder: () => !latest.current.disabled,
      onMoveShouldSetPanResponder: () => !latest.current.disabled,
      onPanResponderTerminationRequest: () => false,
      onPanResponderGrant: (event: GestureResponderEvent) => {
        dragging.current = true;
        grantX.current = event.nativeEvent.locationX;
        move(grantX.current);
      },
      onPanResponderMove: (_event, gesture) =>
        move(grantX.current + gesture.dx),
      onPanResponderRelease: commit,
      onPanResponderTerminate: commit,
    }),
  ).current;

  const nudge = (by: number) => {
    if (disabled) return;
    const next = clamp(draft + by, min, max);
    if (next === draft) return;
    setDraft(next);
    onInput?.(next);
    onChange(next);
  };

  const ratio = max > min ? (draft - min) / (max - min) : 0;
  const travel = Math.max(0, width - THUMB);

  return (
    <View
      testID={testID}
      accessible
      accessibilityRole="adjustable"
      accessibilityLabel={label}
      accessibilityState={{ disabled }}
      accessibilityValue={{ min, max, now: draft }}
      accessibilityActions={[{ name: 'increment' }, { name: 'decrement' }]}
      onAccessibilityAction={event =>
        nudge(event.nativeEvent.actionName === 'increment' ? step : -step)
      }
      onLayout={event => setWidth(event.nativeEvent.layout.width)}
      style={{ height: HEIGHT, justifyContent: 'center' }}
      {...responder.panHandlers}
    >
      <View
        pointerEvents="none"
        style={{ height: 1, backgroundColor: palette.rule }}
      >
        <View
          style={{
            width: ratio * travel + THUMB / 2,
            height: '100%',
            backgroundColor: palette.accent,
          }}
        />
      </View>
      <View
        pointerEvents="none"
        style={{
          position: 'absolute',
          left: ratio * travel,
          width: THUMB,
          height: THUMB,
          backgroundColor: palette.accent,
        }}
      />
    </View>
  );
}
