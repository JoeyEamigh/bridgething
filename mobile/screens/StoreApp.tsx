import {
  compareVersions,
  sortNewestFirst,
  versionCompatible,
  type AppVersion,
} from '@bridgething/catalog';
import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Linking, Text, View } from 'react-native';

import { Button } from '../components/Button';
import { CatalogIcon } from '../components/CatalogIcon';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { Note } from '../components/Note';
import { PairNote } from '../components/PairNote';
import { Pill } from '../components/Pill';
import { Press } from '../components/Press';
import { Progress } from '../components/Progress';
import { Screenshots } from '../components/Screenshots';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { Spinner } from '../components/Spinner';
import {
  describeVersionInstall,
  deviceLibVersion,
  installApp,
  useSourceListings,
} from '../lib/catalog';
import { useOtaProgress } from '../lib/ota';
import {
  describePairOutcome,
  type PairNotice,
  presentPairWithGuidance,
  useSession,
} from '../lib/session';
import { TEXT } from '../lib/theme';
import { formatBytes } from '../lib/utils';
import { humanizePermission } from '../lib/webapp-permissions';
import type { StoreScreenProps } from '../navigation';

type Props = StoreScreenProps<'StoreApp'>;

export function StoreAppScreen({ navigation, route }: Props) {
  const { deviceId, appId, sourceUrl } = route.params;
  const listings = useSourceListings(sourceUrl, deviceId);
  const listing = listings.find(l => l.app.id === appId) ?? null;
  const libVersion = useSession(s => deviceLibVersion(s, deviceId));

  const progress = useOtaProgress(deviceId);
  const installingThis =
    progress && !progress.run.outcome && progress.run.webappId === appId
      ? progress
      : null;

  const [failed, setFailed] = useState<string | null>(null);
  const [installing, setInstalling] = useState<string | null>(null);
  const [asking, setAsking] = useState<AppVersion | null>(null);
  const [asked, setAsked] = useState<AppVersion | null>(null);
  const [pairing, setPairing] = useState(false);
  const [pairNotice, setPairNotice] = useState<PairNotice | null>(null);
  const [showAllVersions, setShowAllVersions] = useState(false);

  if (!listing) {
    return (
      <ScrollScreen>
        <SectionEmpty>this app is no longer listed by that source</SectionEmpty>
        <View className="mt-4">
          <Button
            onPress={() => navigation.navigate('Store')}
            variant="secondary"
            icon="Store"
          >
            back to the store
          </Button>
        </View>
      </ScrollScreen>
    );
  }

  const { app, newestCompatible, installedVersion, updateAvailable } = listing;
  const incompatible = !newestCompatible;
  const canAct =
    deviceId != null && !incompatible && (!installedVersion || updateAvailable);

  const install = async (version: AppVersion) => {
    if (!deviceId) return;
    setFailed(null);
    setInstalling(version.version);
    try {
      await installApp(deviceId, listing, version);
    } catch (err) {
      setFailed(describeError(err));
    } finally {
      setInstalling(null);
    }
  };

  const start = (version: AppVersion) => {
    if (newestCompatible && version.version !== newestCompatible.version) {
      setAsked(version);
      setAsking(version);
      return;
    }
    void install(version);
  };

  const pair = async () => {
    if (pairing) return;
    setPairing(true);
    setPairNotice(null);
    try {
      setPairNotice((await presentPairWithGuidance()).notice);
    } catch (err) {
      setPairNotice(
        describePairOutcome({ kind: 'error', message: describeError(err) }),
      );
    } finally {
      setPairing(false);
    }
  };

  const ordered = sortNewestFirst(app.versions);
  const visibleVersions = showAllVersions ? ordered : ordered.slice(0, 1);
  const question = asked
    ? describeVersionInstall({
        version: asked,
        newest: newestCompatible,
        installedVersion,
      })
    : null;

  return (
    <ScrollScreen contentContainerStyle={{ paddingTop: 12 }}>
      <ConfirmSheet
        visible={asking != null}
        title={question?.title ?? ''}
        body={question?.body}
        warning={question?.warning}
        detail={question?.detail}
        confirmLabel="install it anyway"
        onConfirm={() => {
          const version = asking;
          setAsking(null);
          if (version) void install(version);
        }}
        onClose={() => setAsking(null)}
      />

      <View className="mb-6 flex-row items-center gap-4 border border-rule bg-screen p-4">
        <CatalogIcon url={app.icon} name={app.name} size={64} />
        <View className="min-w-0 flex-1">
          <Text
            className="font-display text-fg"
            style={TEXT.title}
            numberOfLines={2}
          >
            {app.name}
          </Text>
          <Text className="mt-0.5 font-sans text-muted" style={TEXT.hint}>
            {app.author}
          </Text>
          <View className="mt-2 flex-row flex-wrap gap-1.5">
            {installedVersion ? (
              <Pill tone="accent">{`installed v${installedVersion}`}</Pill>
            ) : null}
            {newestCompatible?.role === 'launcher' ? (
              <Pill tone="neutral">home screen</Pill>
            ) : null}
            {newestCompatible?.provides_overlay ? (
              <Pill tone="neutral">overlay</Pill>
            ) : null}
          </View>
        </View>
      </View>

      <Screenshots urls={app.screenshots} name={app.name} />

      {newestCompatible?.provides_overlay ? (
        <Text className="mb-6 px-1 font-sans text-muted" style={TEXT.hint}>
          an overlay app draws on top of whatever else is on the screen.
        </Text>
      ) : null}

      {installingThis ? (
        <View className="mb-6 border border-rule bg-screen p-4">
          <View className="flex-row items-baseline justify-between">
            <Text className="font-sans text-fg" style={TEXT.hint}>
              {installingThis.stepLabel ?? 'installing'}
            </Text>
            <Text className="font-mono text-soft" style={TEXT.hint}>
              {installingThis.percent}%
            </Text>
          </View>
          <Progress percent={installingThis.percent} className="mt-2" />
        </View>
      ) : (
        <View className="mb-6 gap-2">
          <Button
            onPress={() => {
              if (newestCompatible) void install(newestCompatible);
            }}
            disabled={!canAct || installing != null}
            loading={installing === newestCompatible?.version}
          >
            {incompatible
              ? 'needs a newer firmware'
              : updateAvailable
                ? `update to v${newestCompatible?.version}`
                : installedVersion
                  ? 'installed'
                  : `install v${newestCompatible?.version}`}
          </Button>
          {deviceId == null ? (
            <Note tone="warn" action="pair" onAction={() => void pair()}>
              connect a car thing to install
            </Note>
          ) : null}
          <PairNote notice={pairNotice} />
        </View>
      )}

      {failed ? (
        <View className="mb-6">
          <Note tone="err">{failed}</Note>
        </View>
      ) : null}

      <Text className="mb-6 px-1 font-sans text-fg" style={TEXT.body}>
        {app.description}
      </Text>

      {newestCompatible ? (
        <View className="mb-8">
          <SectionHeader title="what this app can do" />
          {newestCompatible.permissions.length === 0 ? (
            <SectionEmpty>nothing beyond drawing on the screen</SectionEmpty>
          ) : (
            <ListGroup>
              {newestCompatible.permissions.map(p => {
                const meta = humanizePermission(p);
                return (
                  <ListRow
                    key={p}
                    icon={meta.icon}
                    title={meta.title}
                    subtitle={meta.subtitle}
                  />
                );
              })}
            </ListGroup>
          )}
        </View>
      ) : null}

      <View className="mb-8">
        <SectionHeader title="versions" />
        <ListGroup>
          {visibleVersions.map(v => (
            <VersionRow
              key={v.version}
              version={v}
              installedVersion={installedVersion}
              libVersion={libVersion}
              busy={installing}
              blocked={deviceId == null || installingThis != null}
              onInstall={() => start(v)}
            />
          ))}
        </ListGroup>
        {app.versions.length > 1 ? (
          <Press
            onPress={() => setShowAllVersions(v => !v)}
            className="mt-2 self-start px-1 py-1"
          >
            <Text
              className="font-mono uppercase text-accent"
              style={TEXT.eyebrow}
            >
              {showAllVersions
                ? 'show fewer'
                : `show all ${app.versions.length} versions`}
            </Text>
          </Press>
        ) : null}
      </View>

      <View>
        <SectionHeader title="where this came from" />
        <ListGroup>
          <ListRow icon="LayoutGrid" title="source" subtitle={sourceUrl} />
          {app.homepage ? (
            <ListRow
              icon="Globe"
              title="homepage"
              subtitle={app.homepage}
              onPress={() => void Linking.openURL(app.homepage as string)}
            />
          ) : null}
          {app.source ? (
            <ListRow
              icon="Globe"
              title="source code"
              subtitle={app.source}
              onPress={() => void Linking.openURL(app.source as string)}
            />
          ) : null}
        </ListGroup>
        <Text className="mt-2 px-1 font-sans text-muted" style={TEXT.hint}>
          apps are not reviewed. buyer beware.
        </Text>
      </View>
    </ScrollScreen>
  );
}

function VersionRow({
  version,
  installedVersion,
  libVersion,
  busy,
  blocked,
  onInstall,
}: {
  version: AppVersion;
  installedVersion: string | null;
  libVersion: string | null;
  busy: string | null;
  blocked: boolean;
  onInstall: () => void;
}) {
  const installed = version.version === installedVersion;
  const compatible = versionCompatible(version, libVersion);
  const older =
    installedVersion != null &&
    compareVersions(version.version, installedVersion) < 0;
  const actionable = !installed && compatible && !blocked;
  const running = busy === version.version;

  return (
    <Press onPress={onInstall} disabled={!actionable || busy != null}>
      <View className="px-4 py-3">
        <View className="flex-row items-center gap-2">
          <Text className="font-mono text-fg" style={TEXT.hint}>
            v{version.version}
          </Text>
          {installed ? <Pill tone="ok">installed</Pill> : null}
          {!installed && !compatible ? (
            <Pill tone="warn">needs newer firmware</Pill>
          ) : null}
          <Text className="ml-auto font-mono text-dim" style={TEXT.eyebrow}>
            {new Date(version.released_at).toLocaleDateString()}
          </Text>
        </View>
        {version.changelog ? (
          <Text className="mt-1 font-sans text-muted" style={TEXT.hint}>
            {version.changelog}
          </Text>
        ) : null}
        <View className="mt-1 flex-row items-center gap-2">
          <Text
            className="min-w-0 flex-1 font-mono text-dim"
            style={TEXT.eyebrow}
          >
            needs firmware {version.min_libbridgething_version} ·{' '}
            {formatBytes(version.download.size)}
          </Text>
          {running ? (
            <Spinner />
          ) : actionable ? (
            <Text
              className="font-mono uppercase text-accent"
              style={TEXT.eyebrow}
            >
              {older ? 'roll back' : 'install'}
            </Text>
          ) : null}
        </View>
      </View>
    </Press>
  );
}
