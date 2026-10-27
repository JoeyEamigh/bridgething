import type {
  BridgethingOtaManifest,
  BridgethingOtaOutcome,
  BridgethingOtaRelease,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useCallback, useEffect, useState } from 'react';
import { Text, View } from 'react-native';

import { ConfirmSheet } from '../components/ConfirmSheet';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { Note } from '../components/Note';
import { OtaRunProgress, OtaStarting } from '../components/OtaRun';
import { Pill } from '../components/Pill';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { Segmented } from '../components/Segmented';
import { Spinner } from '../components/Spinner';
import {
  describeOtaInstall,
  isRunning,
  rootUrlOf,
  useOtaProgress,
} from '../lib/ota';
import { getSession, useSession } from '../lib/session';
import { TEXT } from '../lib/theme';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'OtaVersions'>;

function outcomeLabel(
  outcome: BridgethingOtaOutcome | undefined,
  error: string | undefined,
): string {
  if (outcome === 'succeeded') return 'installed';
  if (outcome === 'cancelled') return 'cancelled';
  return error ? `did not finish · ${describeError(error)}` : 'did not finish';
}

export function OtaVersionsScreen({ route }: Props) {
  const { deviceId, channel: initialChannel } = route.params;
  const meta = useSession(s => s.deviceMeta[deviceId]);
  const rootUrl = rootUrlOf(useSession(s => s.otaPollConfig));
  const progress = useOtaProgress(deviceId, 'firmware');
  const run = progress?.run;
  const outcome = run?.outcome;
  const runError = run?.error;

  const [manifest, setManifest] = useState<BridgethingOtaManifest | null>(null);
  const [channel, setChannel] = useState(initialChannel);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [asking, setAsking] = useState<BridgethingOtaRelease | null>(null);
  const [asked, setAsked] = useState<BridgethingOtaRelease | null>(null);
  const [target, setTarget] = useState<BridgethingOtaRelease | null>(null);
  const [starting, setStarting] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoadError(null);
    setManifest(null);
    try {
      setManifest(await getSession().fetchOtaManifest(rootUrl));
    } catch (err) {
      setLoadError(describeError(err));
      setManifest(null);
    }
  }, [rootUrl]);

  useEffect(() => {
    load();
  }, [load]);

  const channels = manifest?.channels.map(c => c.slug) ?? [];
  const selected = manifest?.channels.find(c => c.slug === channel);
  const releases = manifest ? (selected?.releases ?? []) : null;
  const latest = selected?.latest ?? null;

  const current = meta
    ? `${meta.daemonVersion}+image.${meta.imageVersion}`
    : null;

  const apply = async (release: BridgethingOtaRelease) => {
    setTarget(release);
    setFailure(null);
    setStarting(true);
    try {
      await getSession().applyOtaUpdate(
        deviceId,
        channel,
        release.version,
        rootUrl,
      );
    } catch (err) {
      setFailure(describeError(err));
    } finally {
      setStarting(false);
    }
  };

  const ask = (release: BridgethingOtaRelease) => {
    setAsked(release);
    setAsking(release);
  };

  const installing = isRunning(run) || starting;
  const watching =
    target?.version ?? (isRunning(run) ? (run.releaseVersion ?? null) : null);
  const question = asked
    ? describeOtaInstall(asked, channel, meta?.channel)
    : null;

  return (
    <ScrollScreen>
      <ConfirmSheet
        visible={asking != null}
        title={question?.title ?? ''}
        body={question?.body}
        warning={question?.warning}
        detail={question?.detail}
        confirmLabel="install"
        onConfirm={() => {
          const release = asking;
          setAsking(null);
          if (release) void apply(release);
        }}
        onClose={() => setAsking(null)}
      />

      <SectionHeader
        title="releases"
        hint={current ? `installed ${current}` : undefined}
      />
      {channels.length > 1 ? (
        <View className="mb-3">
          <Segmented options={channels} value={channel} onChange={setChannel} />
        </View>
      ) : null}

      {releases == null && !loadError ? (
        <View className="items-center py-10">
          <Spinner />
        </View>
      ) : loadError ? (
        <Note tone="err" action="retry" onAction={() => void load()}>
          {loadError}
        </Note>
      ) : !selected ? (
        <SectionEmpty>{`there is no ${channel} track on this update host`}</SectionEmpty>
      ) : releases == null || releases.length === 0 ? (
        <SectionEmpty>nothing has been released here yet</SectionEmpty>
      ) : (
        <ListGroup>
          {releases.map(r => {
            const isCurrent = current === r.version;
            const watched = watching === r.version;

            if (watched && (installing || failure || outcome)) {
              return (
                <View key={r.version} className="gap-3 px-4 py-3">
                  <Text className="font-mono text-fg" style={TEXT.row}>
                    {r.version}
                  </Text>
                  {run && progress && !outcome ? (
                    <OtaRunProgress run={run} progress={progress} />
                  ) : starting ? (
                    <OtaStarting />
                  ) : failure ? (
                    <Note
                      tone="err"
                      action="retry"
                      onAction={() => void apply(r)}
                    >
                      {failure}
                    </Note>
                  ) : (
                    <Text
                      className={`font-sans ${
                        outcome === 'succeeded' ? 'text-ok' : 'text-muted'
                      }`}
                      style={TEXT.hint}
                    >
                      {outcomeLabel(outcome, runError)}
                    </Text>
                  )}
                </View>
              );
            }

            const tappable = !r.yanked && !isCurrent && !installing;
            return (
              <ListRow
                key={r.version}
                title={r.version}
                subtitle={`daemon ${r.daemonVersion} · image ${r.imageVersion}`}
                disabled={!tappable}
                onPress={tappable ? () => ask(r) : undefined}
                trailing={
                  r.yanked ? (
                    <Pill tone="err" dot={false}>
                      yanked
                    </Pill>
                  ) : isCurrent ? (
                    <Pill tone="ok" dot={false}>
                      installed
                    </Pill>
                  ) : latest === r.version ? (
                    <Pill tone="accent" dot={false}>
                      latest
                    </Pill>
                  ) : r.deprecated ? (
                    <Pill tone="warn" dot={false}>
                      old
                    </Pill>
                  ) : undefined
                }
              />
            );
          })}
        </ListGroup>
      )}
    </ScrollScreen>
  );
}
