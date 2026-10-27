import { PROVIDER_MARKS } from '@bridgething/ui/provider-marks';
import type { VNode } from 'preact';

import { Icon } from '../lib/icons.tsx';

export function ProviderMark({ id }: { id: string }): VNode {
  const mark = PROVIDER_MARKS[id];
  if (!mark) return <Icon name="music" />;
  return (
    <span aria-hidden="true" class="flex size-5 text-fg [&>svg]:size-full" dangerouslySetInnerHTML={{ __html: mark }} />
  );
}
