import { useEffect, useState } from 'react';
import { Image, Text, View } from 'react-native';
import { SvgXml } from 'react-native-svg';

import { Icon } from './Icon';
import { boundedCache } from '../lib/bounded-cache';
import { getSession } from '../lib/session';
import { TEXT } from '../lib/theme';

type IconData = { svg?: string; fileUri?: string };

const ICON_CACHE_LIMIT = 96;
const cache = boundedCache<IconData>(ICON_CACHE_LIMIT);

export function WebappIcon({
  deviceId,
  id,
  iconHash,
  name,
  size,
}: {
  deviceId: string | null;
  id: string;
  iconHash?: string;
  name: string;
  size: number;
}) {
  const session = getSession();
  const [icon, setIcon] = useState<IconData | null>(null);

  useEffect(() => {
    if (!iconHash || !deviceId) {
      setIcon(null);
      return;
    }
    const key = `${deviceId}:${id}:${iconHash}`;
    const cached = cache.get(key);
    if (cached) {
      setIcon(cached);
      return;
    }
    let cancelled = false;
    (async () => {
      try {
        const result = await session.webappIcon(deviceId, id);
        if (!result) return;
        const next = { svg: result.svg, fileUri: result.fileUri };
        cache.set(key, next);
        if (!cancelled) setIcon(next);
      } catch {
        // icon load failure is non-fatal; the fallback renders.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [deviceId, iconHash, id, session]);

  const dims = { width: size, height: size };

  return (
    <View
      className="items-center justify-center overflow-hidden border border-rule bg-neutral-soft"
      style={dims}
    >
      {icon?.svg ? (
        <SvgXml xml={icon.svg} width={size} height={size} />
      ) : icon?.fileUri ? (
        <Image source={{ uri: icon.fileUri }} style={dims} resizeMode="cover" />
      ) : name ? (
        <Text className="font-mono uppercase text-soft" style={TEXT.rowLg}>
          {name.slice(0, 1)}
        </Text>
      ) : (
        <Icon name="AppWindow" size={Math.round(size * 0.42)} />
      )}
    </View>
  );
}
