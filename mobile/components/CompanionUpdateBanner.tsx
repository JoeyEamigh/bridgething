import { Text, View } from 'react-native';

import { Press } from './Press';
import {
  type CompanionUpdatePhase,
  dismissCompanionUpdate,
  manifestHost,
  startCompanionUpdate,
  useCompanionUpdateCheck,
  usePendingCompanionUpdate,
} from '../lib/companion-update';
import { rootUrlOf } from '../lib/ota';
import { useSession } from '../lib/session';
import { TEXT } from '../lib/theme';
import { TONE_BG, TONE_BORDER, TONE_TEXT } from '../lib/tone';
import { formatBytes } from '../lib/utils';

export function CompanionUpdateBanner({ className }: { className?: string }) {
  const root = rootUrlOf(useSession(s => s.otaPollConfig));
  useCompanionUpdateCheck(root);
  const pending = usePendingCompanionUpdate();
  if (!pending) return null;

  const { release, phase } = pending;
  const tone = phase.kind === 'failed' ? 'err' : 'accent';
  const busy = phase.kind === 'downloading';
  const host = manifestHost(root);

  return (
    <View
      className={`border px-3 py-2 ${TONE_BORDER[tone]} ${TONE_BG[tone]} ${className ?? ''}`}
    >
      <Text
        className={`mb-1 font-mono uppercase ${TONE_TEXT[tone]}`}
        style={TEXT.eyebrow}
        numberOfLines={1}
      >
        app update v{release.version}
      </Text>
      <Text className={`font-mono ${TONE_TEXT[tone]}`} style={TEXT.hint}>
        {describePhase(phase, release.size, host)}
      </Text>
      {busy ? null : (
        <View className="mt-2 flex-row gap-4">
          <Press
            onPress={() => void startCompanionUpdate(release)}
            className="px-1 py-0.5"
          >
            <Text
              className={`font-mono uppercase ${TONE_TEXT[tone]}`}
              style={TEXT.eyebrow}
            >
              {phase.kind === 'failed' ? 'retry' : 'download and install'}
            </Text>
          </Press>
          <Press
            onPress={() => dismissCompanionUpdate(release.version)}
            className="px-1 py-0.5"
          >
            <Text className="font-mono uppercase text-dim" style={TEXT.eyebrow}>
              skip this version
            </Text>
          </Press>
        </View>
      )}
    </View>
  );
}

function describePhase(
  phase: CompanionUpdatePhase,
  size: number,
  host: string,
): string {
  switch (phase.kind) {
    case 'downloading': {
      const total = phase.total > 0 ? phase.total : size;
      const pct =
        total > 0
          ? Math.min(100, Math.round((phase.received / total) * 100))
          : 0;
      return `downloading ${formatBytes(phase.received)} of ${formatBytes(total)} (${pct}%)`;
    }
    case 'failed':
      return phase.reason;
    case 'idle':
    default:
      return `${formatBytes(size)} apk from ${host}. android will ask you to confirm the install.`;
  }
}
