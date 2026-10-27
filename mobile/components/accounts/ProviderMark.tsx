import { PROVIDER_MARKS } from '@bridgething/ui/provider-marks';
import { SvgXml } from 'react-native-svg';

import { Icon } from '../Icon';
import { usePalette } from '../../lib/theme';

export function ProviderMark({ id, size }: { id: string; size: number }) {
  const palette = usePalette();
  const mark = PROVIDER_MARKS[id];
  if (!mark) return <Icon name="Music" size={size} />;
  return <SvgXml xml={mark} width={size} height={size} color={palette.fg} />;
}
