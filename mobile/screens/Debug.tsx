import type {
  BridgethingCompanionDebug,
  BridgethingSessionSnapshot,
} from '@bridgething/session-react-native';
import { type ReactNode, useCallback, useState } from 'react';
import { Text, View } from 'react-native';

import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { Switch } from '../components/ui/switch';
import { usePoll } from '../lib/poll';
import { getSession } from '../lib/session';
import { getNativeTabs, setNativeTabs } from '../lib/storage';
import { TEXT } from '../lib/theme';
import { formatBytes } from '../lib/utils';
import type { SettingsScreenProps } from '../navigation';

const POLL_MS = 1500;

type Props = SettingsScreenProps<'Debug'>;

export function DebugScreen(_: Props) {
  const [companion, setCompanion] = useState<BridgethingCompanionDebug | null>(
    null,
  );
  const [snapshot, setSnapshot] = useState<BridgethingSessionSnapshot | null>(
    null,
  );

  const refresh = useCallback(async () => {
    const session = getSession();
    const [c, s] = await Promise.allSettled([
      session.companionDebug(),
      session.snapshot(),
    ]);
    setCompanion(c.status === 'fulfilled' ? c.value : null);
    setSnapshot(s.status === 'fulfilled' ? s.value : null);
  }, []);

  usePoll(refresh, POLL_MS);

  return (
    <ScrollScreen>
      {!companion && !snapshot ? (
        <SectionEmpty>no session state yet · sign in and connect</SectionEmpty>
      ) : (
        <StateDump companion={companion} snapshot={snapshot} />
      )}
      <ExperimentsSection />
    </ScrollScreen>
  );
}

function ExperimentsSection() {
  const [nativeTabs, setNative] = useState(getNativeTabs());

  return (
    <View className="mt-8">
      <SectionHeader title="experiments" />
      <ListGroup>
        <ListRow
          icon="PanelBottom"
          title="native tab bar"
          subtitle="takes effect after closing and reopening the app"
          trailing={
            <Switch
              value={nativeTabs}
              onValueChange={next => {
                setNative(next);
                setNativeTabs(next);
              }}
            />
          }
        />
      </ListGroup>
    </View>
  );
}

function StateDump({
  companion,
  snapshot,
}: {
  companion: BridgethingCompanionDebug | null;
  snapshot: BridgethingSessionSnapshot | null;
}) {
  const np = snapshot?.nowPlaying;
  const repeat = np?.playback.repeatMode ?? 'off';

  return (
    <>
      {np ? (
        <Section title="now playing">
          <Row label="track id" value={np.track?.id ?? '-'} wrap />
          <Row label="title" value={np.track?.title ?? '-'} />
          <Row label="artist" value={np.track?.artist ?? '-'} />
          <Row label="album" value={np.track?.album ?? '-'} />
          <Row label="duration" value={ms(np.track?.durationMs)} />
          <Row label="position" value={ms(np.playback.positionMs)} />
          <Row
            label="state"
            value={np.playback.playing ? 'playing' : 'paused'}
          />
          <Row label="shuffle" value={yesno(np.playback.shuffle)} />
          <Row label="repeat" value={repeat} />
          <Row label="app" value={np.appName ?? '-'} />
        </Section>
      ) : null}

      {snapshot ? (
        <Section title="connection">
          {snapshot.peers.length === 0 ? (
            <Row label="peers" value="none" />
          ) : (
            snapshot.peers.map(p => (
              <Row
                key={p.id}
                label={p.name || p.id}
                value={
                  p.status === 'connected'
                    ? 'connected'
                    : `link failed${p.linkError ? `: ${p.linkError}` : ''}`
                }
                wrap
              />
            ))
          )}
          {snapshot.providers.map(p => (
            <Row
              key={p.id}
              label={p.displayName}
              value={`${p.connected ? 'connected' : p.authState.kind}${
                p.serviceHealth.kind === 'ok'
                  ? ''
                  : ` / ${p.serviceHealth.kind}`
              }`}
            />
          ))}
          <Row label="priority" value={list(snapshot.providerPriority)} wrap />
          {snapshot.ancsAuthStatuses.map(a => (
            <Row
              key={a.deviceId}
              label={`ANCS · ${a.deviceId}`}
              value={a.status}
            />
          ))}
          {snapshot.libraryProvider ? (
            <Row label="library" value={snapshot.libraryProvider} />
          ) : null}
        </Section>
      ) : null}

      {companion ? (
        <Section title="companion">
          <Row
            label="authority · playback"
            value={held(companion.authorityPlaybackHeld)}
          />
          <Row
            label="authority · metadata"
            value={held(companion.authorityMetadataHeld)}
          />
          <Row
            label="authority · volume"
            value={held(companion.authorityVolumeHeld)}
          />
          <Row
            label="claimed bundle"
            value={companion.authorityAppBundle ?? '-'}
            wrap
          />
          <Row label="audible" value={companion.arbitratedSource ?? 'none'} />
          <Row label="library source" value={companion.librarySource ?? '-'} />
          <Row
            label="last played from"
            value={companion.lastPlayedFrom ?? '-'}
          />
          <Row
            label="attached"
            value={list(companion.attachedProviders)}
            wrap
          />
          <Row
            label="uri schemes"
            value={list(companion.attachedSchemes)}
            wrap
          />
          <Row label="linked" value={list(companion.linkedDevices)} wrap />
          {companion.autoResume.map(entry => (
            <Row
              key={entry.deviceId}
              label={`auto-resume · ${entry.deviceId}`}
              value={yesno(entry.enabled)}
            />
          ))}
        </Section>
      ) : null}

      {snapshot ? (
        <Section title="voice">
          <Row label="model" value={snapshot.voiceModel.status} />
          <Row
            label="downloaded"
            value={progress(
              snapshot.voiceModel.receivedBytes,
              snapshot.voiceModel.totalBytes,
            )}
          />
          <Row label="version" value={snapshot.voiceModel.version ?? '-'} />
          {snapshot.voiceModel.error ? (
            <Row label="error" value={snapshot.voiceModel.error} wrap />
          ) : null}
          {companion ? (
            <>
              <Row label="armed" value={yesno(companion.voice.hasModel)} />
              <Row
                label="nlu bundle"
                value={companion.voice.nluBundleDir ?? '-'}
                wrap
              />
              <Row
                label="asr weights"
                value={companion.voice.asrWeights ?? '-'}
                wrap
              />
            </>
          ) : null}
        </Section>
      ) : null}

      {snapshot?.deviceMeta.map(d => (
        <Section key={d.deviceId} title={`device · ${d.deviceId}`}>
          <Row label="serial" value={d.meta.serialNumber || '-'} wrap />
          <Row label="channel" value={d.meta.channel || '-'} />
          <Row label="daemon" value={d.meta.daemonVersion || '-'} />
          <Row label="image" value={d.meta.imageVersion || '-'} />
          <Row
            label="os"
            value={`${d.meta.osName} ${d.meta.osVersion}`.trim() || '-'}
          />
        </Section>
      ))}

      {snapshot?.webapps.map(entry => (
        <Section key={entry.deviceId} title={`apps · ${entry.deviceId}`}>
          <Row label="active" value={entry.active?.id ?? 'none'} wrap />
          {entry.webapps.length === 0 ? (
            <Row label="installed" value="none" />
          ) : (
            entry.webapps.map(app => (
              <Row
                key={app.id}
                label={app.name}
                value={`${app.version} · ${app.source}${app.role === 'launcher' ? ' · launcher' : ''}`}
                wrap
              />
            ))
          )}
        </Section>
      ))}

      {snapshot ? (
        <Section title="capabilities">
          <Row label="geo" value={yesno(snapshot.capabilityFlags.geo)} />
          <Row
            label="notifications"
            value={yesno(snapshot.capabilityFlags.notifications)}
          />
          <Row
            label="net · fetch"
            value={yesno(snapshot.capabilityFlags.netFetch)}
          />
          <Row label="net · ws" value={yesno(snapshot.capabilityFlags.netWs)} />
          <Row
            label="audio · tts"
            value={yesno(snapshot.capabilityFlags.audioTts)}
          />
          <Row
            label="voice model"
            value={yesno(snapshot.capabilityFlags.voiceModel)}
          />
        </Section>
      ) : null}

      {snapshot ? (
        <Section title="ota">
          {snapshot.otaAvailable.length === 0 ? (
            <Row label="available" value="none" />
          ) : (
            snapshot.otaAvailable.map(a => (
              <Row
                key={a.deviceId}
                label={`available · ${a.deviceId}`}
                value={
                  a.releaseVersion ?? a.daemonVersion ?? a.imageVersion ?? '-'
                }
                wrap
              />
            ))
          )}
          {snapshot.otaRuns.map(run => (
            <Row
              key={run.runId}
              label={`${run.otaKind} · ${run.deviceId}`}
              value={`${run.phase}${run.outcome ? ` / ${run.outcome}` : ''}${run.error ? `: ${run.error}` : ''}`}
              wrap
            />
          ))}
          <Row
            label="last poll"
            value={snapshot.otaPoll.lastPolledAt ?? '-'}
            wrap
          />
          {snapshot.otaPoll.error ? (
            <Row label="poll error" value={snapshot.otaPoll.error} wrap />
          ) : null}
          {snapshot.otaPollConfig ? (
            <>
              <Row
                label="interval"
                value={`${snapshot.otaPollConfig.intervalSeconds}s`}
              />
              <Row
                label="auto-push"
                value={yesno(snapshot.otaPollConfig.autoPush)}
              />
              {snapshot.otaPollConfig.rootUrl ? (
                <Row label="root" value={snapshot.otaPollConfig.rootUrl} wrap />
              ) : null}
            </>
          ) : null}
        </Section>
      ) : null}

      {snapshot ? (
        <Section title="host">
          <Row
            label="app"
            value={`${snapshot.hostInfo.appName} ${snapshot.hostInfo.appVersion}`}
          />
          <Row
            label="os"
            value={`${snapshot.hostInfo.osName} ${snapshot.hostInfo.osVersion}`.trim()}
          />
          <Row label="lib" value={snapshot.hostInfo.libVersion} />
          <Row
            label="libbridgething"
            value={snapshot.hostInfo.libbridgethingVersion}
          />
          <Row label="adapter" value={snapshot.hostInfo.adapterVersion} />
          <Row label="host id" value={snapshot.hostInfo.hostIdentifier} wrap />
        </Section>
      ) : null}
    </>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <View className="mb-8">
      <SectionHeader title={title} />
      <View className="gap-1.5 border border-rule bg-screen px-4 py-3">
        {children}
      </View>
    </View>
  );
}

function Row({
  label,
  value,
  wrap,
}: {
  label: string;
  value: string;
  wrap?: boolean;
}) {
  return (
    <View className="flex-row items-baseline justify-between gap-4">
      <Text
        className="shrink font-mono text-dim"
        style={TEXT.hint}
        numberOfLines={1}
      >
        {label}
      </Text>
      <Text
        className="flex-1 text-right font-mono text-near"
        style={TEXT.hint}
        numberOfLines={wrap ? undefined : 1}
      >
        {value}
      </Text>
    </View>
  );
}

function ms(value: number | undefined): string {
  if (value == null) return '-';
  const total = Math.round(value / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

function yesno(value: boolean): string {
  return value ? 'yes' : 'no';
}

function held(value: boolean): string {
  return value ? 'held' : 'no';
}

function list(values: string[]): string {
  return values.length === 0 ? 'none' : values.join(', ');
}

function progress(received: number, total: number): string {
  if (total === 0) return received === 0 ? '-' : formatBytes(received);
  return `${formatBytes(received)} / ${formatBytes(total)}`;
}
