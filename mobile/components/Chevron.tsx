import { Text } from 'react-native';

import { TEXT } from '../lib/theme';

export function Chevron() {
  return (
    <Text
      className="font-mono text-dim"
      style={TEXT.body}
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants"
    >
      ›
    </Text>
  );
}
