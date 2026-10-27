import { Text, View } from 'react-native';

import { Press } from './Press';
import { TEXT } from '../lib/theme';

export function Segmented<T extends string>({
  options,
  value,
  onChange,
  size = 'md',
  disabled = false,
}: {
  options: ReadonlyArray<T> | ReadonlyArray<{ value: T; label: string }>;
  value: T | null;
  onChange: (next: T) => void;
  size?: 'sm' | 'md';
  disabled?: boolean;
}) {
  const items = options.map(option =>
    typeof option === 'string'
      ? { value: option as T, label: option as string }
      : option,
  );
  const pad = size === 'sm' ? 'px-2.5 py-1' : 'px-3 py-1.5';
  const label = size === 'sm' ? TEXT.eyebrow : TEXT.hint;

  return (
    <View
      className={`flex-row self-start border border-rule ${disabled ? 'opacity-40' : ''}`}
      accessibilityRole="radiogroup"
    >
      {items.map((item, index) => {
        const selected = item.value === value;
        return (
          <Press
            key={item.value}
            onPress={() => onChange(item.value)}
            disabled={disabled}
            accessibilityRole="radio"
            accessibilityState={{ selected, disabled }}
            accessibilityLabel={item.label}
            className={`${index > 0 ? 'border-l border-rule' : ''} ${
              selected ? 'bg-accent-soft' : ''
            }`}
          >
            <View className={pad}>
              <Text
                className={`font-mono uppercase ${selected ? 'text-accent' : 'text-soft'}`}
                style={label}
              >
                {item.label}
              </Text>
            </View>
          </Press>
        );
      })}
    </View>
  );
}
