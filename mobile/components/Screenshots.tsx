import { useState } from 'react';
import { ScrollView, useWindowDimensions } from 'react-native';

import { DeviceFrame, SCREEN_ASPECT } from './DeviceFrame';
import { SPACE } from '../lib/theme';
import { isHttpUrl } from '../lib/utils';

const GAP = 10;
const PEEK = 0.82;

export const SHOT_ASPECT = SCREEN_ASPECT;

export function visibleShots(
  urls: string[] | undefined,
  broken: readonly string[] = [],
): string[] {
  return (urls ?? []).filter(url => isHttpUrl(url) && !broken.includes(url));
}

export function Screenshots({
  urls,
  name,
}: {
  urls: string[] | undefined;
  name: string;
}) {
  const [broken, setBroken] = useState<string[]>([]);
  const { width: window } = useWindowDimensions();
  const shots = visibleShots(urls, broken);

  if (shots.length === 0) return null;

  const content = window - SPACE.gutter * 2;
  const width = shots.length === 1 ? content : Math.round(content * PEEK);

  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      decelerationRate="fast"
      snapToInterval={width + GAP}
      snapToAlignment="start"
      scrollEnabled={shots.length > 1}
      testID="app-screenshots"
      accessibilityLabel={`${name} screenshots`}
      style={{ marginHorizontal: -SPACE.gutter }}
      contentContainerStyle={{ gap: GAP, paddingHorizontal: SPACE.gutter }}
      className="mb-6"
    >
      {shots.map(url => (
        <DeviceFrame
          key={url}
          uri={url}
          width={width}
          onBroken={() =>
            setBroken(prev => (prev.includes(url) ? prev : [...prev, url]))
          }
        />
      ))}
    </ScrollView>
  );
}
