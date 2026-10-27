import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from './Button';
import { Icon } from './Icon';
import { Note } from './Note';
import { OtaRunProgress, OtaStarting } from './OtaRun';
import { Press } from './Press';
import { getSession } from '../lib/bridge';
import {
  describeOtaOffer,
  dismissOtaRun,
  installLatestOta,
  lastCheckedAt,
  rootUrlOf,
  useOta,
  useOtaProgress,
} from '../lib/ota';
import { useSession } from '../lib/session';
import { TEXT } from '../lib/theme';

export function OtaCard({
  deviceId,
  onPickVersion,
}: {
  deviceId: string;
  onPickVersion?: () => void;
}) {
  const meta = useSession(s => s.deviceMeta[deviceId]);
  const rootUrl = rootUrlOf(useSession(s => s.otaPollConfig));
  const poll = useOta(s => s.poll);
  const available = useOta(s => s.available[deviceId]);
  const progress = useOtaProgress(deviceId);
  const run = progress?.run;

  const [checking, setChecking] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const offer = describeOtaOffer({
    available,
    lastCheckedAt: lastCheckedAt(poll),
    error: poll.error,
  });

  const check = async () => {
    setChecking(true);
    setFailure(null);
    try {
      await getSession().checkForOtaUpdate(rootUrl);
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setChecking(false);
    }
  };

  const install = async () => {
    setInstalling(true);
    setFailure(null);
    try {
      await installLatestOta(deviceId, meta?.channel || 'stable', rootUrl);
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setInstalling(false);
    }
  };

  const running = run != null && run.outcome === undefined;

  return (
    <View className="gap-3 border border-rule bg-screen p-4">
      <View className="flex-row items-baseline justify-between gap-3">
        <Text className="font-sans text-fg" style={TEXT.body}>
          {run?.webappName ?? 'firmware'}
        </Text>
        {run?.outcome ? (
          <Press onPress={() => dismissOtaRun(deviceId)} hitSlop={10}>
            <Icon name="X" size={16} />
          </Press>
        ) : (
          <Text
            className="font-mono text-soft"
            style={TEXT.hint}
            numberOfLines={1}
          >
            {offer.value}
          </Text>
        )}
      </View>

      {running && progress ? (
        <OtaRunProgress run={run} progress={progress} />
      ) : installing ? (
        <OtaStarting />
      ) : run?.outcome === 'succeeded' ? (
        <Text className="font-sans text-ok" style={TEXT.hint}>
          update installed
        </Text>
      ) : run?.outcome === 'cancelled' ? (
        <Text className="font-sans text-muted" style={TEXT.hint}>
          update cancelled
        </Text>
      ) : run?.outcome === 'failed' ? (
        <Text className="font-sans text-muted" style={TEXT.hint}>
          {run.resumable
            ? 'waiting to reconnect to finish'
            : 'the update did not finish'}
        </Text>
      ) : offer.detail ? (
        <Text className="font-mono text-dim" style={TEXT.hint}>
          {offer.detail}
        </Text>
      ) : null}

      {run?.error ? (
        <Note tone="err">{describeError(run.error)}</Note>
      ) : failure ? (
        <Note tone="err">{failure}</Note>
      ) : null}

      {!running && !installing ? (
        <View className="gap-2">
          {offer.version || run?.outcome === 'failed' ? (
            <Button onPress={install} size="md">
              {offer.version ? `install ${offer.version}` : 'try again'}
            </Button>
          ) : null}
          <Button
            onPress={check}
            loading={checking}
            variant="secondary"
            size="md"
            icon="RefreshCw"
          >
            check for updates now
          </Button>
        </View>
      ) : null}

      {onPickVersion ? (
        <Press
          onPress={onPickVersion}
          className="flex-row items-center gap-1.5 self-start py-1"
        >
          <Text
            className="font-mono uppercase text-accent"
            style={TEXT.eyebrow}
          >
            choose a specific version
          </Text>
          <Icon name="ChevronRight" tone="accent" size={14} />
        </Press>
      ) : null}
    </View>
  );
}
