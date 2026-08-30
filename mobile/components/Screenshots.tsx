import { useState } from 'react';
import { Image, ScrollView } from 'react-native';

import { SPACE } from '../lib/theme';
import { isHttpUrl } from '../lib/utils';

const SHOT_HEIGHT = 190;
const SHOT_WIDTH = Math.round((SHOT_HEIGHT * 800) / 480);
const GAP = 10;

export const SHOT_ASPECT = SHOT_WIDTH / SHOT_HEIGHT;

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
  const shots = visibleShots(urls, broken);

  if (shots.length === 0) return null;

  return (
    <ScrollView
      horizontal
      showsHorizontalScrollIndicator={false}
      decelerationRate="fast"
      snapToInterval={SHOT_WIDTH + GAP}
      snapToAlignment="start"
      testID="app-screenshots"
      accessibilityLabel={`${name} screenshots`}
      style={{ marginHorizontal: -SPACE.gutter }}
      contentContainerStyle={{ gap: GAP, paddingHorizontal: SPACE.gutter }}
      className="mb-6"
    >
      {shots.map(url => (
        <Image
          key={url}
          source={{ uri: url }}
          style={{ width: SHOT_WIDTH, height: SHOT_HEIGHT }}
          resizeMode="cover"
          className="border border-rule bg-neutral-soft"
          onError={() =>
            setBroken(prev => (prev.includes(url) ? prev : [...prev, url]))
          }
        />
      ))}
    </ScrollView>
  );
}
