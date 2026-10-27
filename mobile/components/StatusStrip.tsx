import { Text, View } from 'react-native';

import { Chevron } from './Chevron';
import { Press } from './Press';
import { TEXT, type Tone } from '../lib/theme';
import { TONE_BG, TONE_BORDER, TONE_DOT } from '../lib/tone';

export function StatusStrip({
  tone = 'neutral',
  title,
  subtitle,
  onPress,
}: {
  tone?: Tone;
  title: string;
  subtitle?: string;
  onPress?: () => void;
}) {
  const body = (
    <View
      className={`flex-row items-center gap-3 border px-4 py-3 ${TONE_BG[tone]} ${TONE_BORDER[tone]}`}
    >
      <View className={`h-2 w-2 ${TONE_DOT[tone]}`} />
      <View className="min-w-0 flex-1">
        <Text className="font-sans text-fg" style={TEXT.body} numberOfLines={1}>
          {title}
        </Text>
        {subtitle ? (
          <Text
            className="mt-0.5 font-sans text-muted"
            style={TEXT.hint}
            numberOfLines={1}
          >
            {subtitle}
          </Text>
        ) : null}
      </View>
      {onPress ? <Chevron /> : null}
    </View>
  );

  if (!onPress) return body;
  return <Press onPress={onPress}>{body}</Press>;
}
