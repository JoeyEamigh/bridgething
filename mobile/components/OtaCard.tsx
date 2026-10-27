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
  const progress = useOtaProgress(deviceId, 'firmware');
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
  const status = running
    ? null
    : run?.outcome === 'succeeded'
      ? { text: 'update installed', tone: 'ok' }
      : run?.outcome === 'cancelled'
        ? { text: 'update cancelled', tone: 'muted' }
        : run?.outcome === 'failed'
          ? {
              text: run.resumable
                ? 'waiting to reconnect to finish'
                : 'the update did not finish',
              tone: 'muted',
            }
          : offer.version
            ? null
            : { text: offer.value, tone: 'muted' };

  return (
    <View className="gap-3 border border-rule bg-screen p-4">
      {running && progress ? (
        <OtaRunProgress run={run} progress={progress} />
      ) : installing ? (
        <OtaStarting />
      ) : status ? (
        <View className="flex-row items-center justify-between gap-3">
          <Text
            className={`flex-1 font-sans ${status.tone === 'ok' ? 'text-ok' : 'text-fg'}`}
            style={TEXT.body}
          >
            {status.text}
          </Text>
          {run?.outcome ? (
            <Press
              onPress={() => dismissOtaRun(deviceId, 'firmware')}
              hitSlop={10}
              accessibilityRole="button"
              accessibilityLabel="dismiss"
            >
              <Icon name="X" size={16} />
            </Press>
          ) : null}
        </View>
      ) : null}

      {!running && !installing && offer.detail ? (
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
            check for updates
          </Button>
        </View>
      ) : null}

      {onPickVersion ? (
        <Press
          onPress={onPickVersion}
          className="flex-row items-center gap-1.5 self-start py-1"
          accessibilityRole="button"
        >
          <Text className="font-sans text-accent" style={TEXT.hint}>
            choose a specific version
          </Text>
          <Icon name="ChevronRight" tone="accent" size={14} />
        </Press>
      ) : null}
    </View>
  );
}
