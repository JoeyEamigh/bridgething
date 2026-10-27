import type {
  BridgethingOtaProgress,
  BridgethingOtaRun,
} from '@bridgething/session-react-native';
import { Text, View } from 'react-native';

import { Progress } from './Progress';
import { otaPhaseLabel, otaRunDetail } from '../lib/ota';
import { TEXT } from '../lib/theme';

export function OtaRunProgress({
  run,
  progress,
  className,
}: {
  run: BridgethingOtaRun;
  progress: BridgethingOtaProgress;
  className?: string;
}) {
  const detail = otaRunDetail(run, progress.etaSeconds);
  return (
    <View className={className}>
      <View className="flex-row items-baseline justify-between">
        <Text className="font-sans text-fg" style={TEXT.hint}>
          {otaPhaseLabel(run)}
        </Text>
        <Text className="font-mono text-soft" style={TEXT.hint}>
          {progress.percent}%
        </Text>
      </View>
      <Progress percent={progress.percent} className="mt-2" />
      {detail ? (
        <Text className="mt-1.5 font-mono text-dim" style={TEXT.eyebrow}>
          {detail}
        </Text>
      ) : null}
    </View>
  );
}

export function OtaStarting({ className }: { className?: string }) {
  return (
    <View className={className}>
      <Text className="font-sans text-fg" style={TEXT.hint}>
        starting
      </Text>
      <Progress percent={null} className="mt-2" />
    </View>
  );
}
