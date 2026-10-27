import FRAME from '@bridgething/ui/car-thing-frame.webp';
import { DEVICE_FRAME } from '@bridgething/ui/tokens';
import { useState, type ReactNode } from 'react';
import { Image, View } from 'react-native';

import { isHttpUrl } from '../lib/utils';

const { screen } = DEVICE_FRAME;

export const SCREEN_ASPECT =
  (DEVICE_FRAME.width * screen.width) / (DEVICE_FRAME.height * screen.height);

export function DeviceFrame({
  uri,
  width,
  fallback,
  onBroken,
}: {
  uri: string | null | undefined;
  width: number;
  fallback?: ReactNode;
  onBroken?: () => void;
}) {
  const [broken, setBroken] = useState<string | null>(null);
  const height = Math.round((width * DEVICE_FRAME.height) / DEVICE_FRAME.width);
  const shown = isHttpUrl(uri) && broken !== uri ? uri : null;
  const window = {
    position: 'absolute' as const,
    left: width * screen.left,
    top: height * screen.top,
    width: width * screen.width,
    height: height * screen.height,
  };

  return (
    <View style={{ width, height }}>
      {shown ? (
        <Image
          source={{ uri: shown }}
          style={window}
          resizeMode="cover"
          className="bg-neutral-soft"
          onError={() => {
            setBroken(shown);
            onBroken?.();
          }}
        />
      ) : (
        <View style={window} className="items-center justify-center bg-screen">
          {fallback}
        </View>
      )}
      <Image
        source={FRAME}
        style={{ position: 'absolute', left: 0, top: 0, width, height }}
        resizeMode="stretch"
      />
    </View>
  );
}
