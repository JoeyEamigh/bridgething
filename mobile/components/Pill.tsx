import { Text, View } from 'react-native';

import { TEXT, type Tone } from '../lib/theme';
import { TONE_BG, TONE_DOT, TONE_TEXT } from '../lib/tone';

export function Pill({
  tone = 'neutral',
  dot = false,
  children,
}: {
  tone?: Tone;
  dot?: boolean;
  children: string;
}) {
  return (
    <View className="items-start">
      <View
        className={`flex-row items-center gap-1.5 px-2 py-0.5 ${TONE_BG[tone]}`}
      >
        {dot ? (
          <View
            className={`h-1.5 w-1.5 ${TONE_DOT[tone]}`}
            accessibilityElementsHidden
            importantForAccessibility="no-hide-descendants"
          />
        ) : null}
        <Text
          className={`font-mono uppercase ${TONE_TEXT[tone]}`}
          style={TEXT.eyebrow}
        >
          {children}
        </Text>
      </View>
    </View>
  );
}
