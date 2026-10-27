import {
  appCategories,
  changelogText,
  compareVersions,
  fillsSlot,
  installsLabel,
  isLauncher,
  isOverlay,
  listingKindLabel,
  moreFromAuthor,
  permissionLabel,
  releasedAtInstant,
  sortNewestFirst,
  sourceLine,
  versionCompatible,
  type AppVersion,
  type CatalogAppListing,
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
import { SlotAction } from '../components/SlotAction';
import { Spinner } from '../components/Spinner';
import { useStoreInstall } from '../components/store/install';
import { ListingTile } from '../components/store/listing';
import { HorizontalRail } from '../components/store/storefront';
import { SwitchWebappButton } from '../components/SwitchWebappButton';
import {
  describeVersionInstall,
  deviceLibVersion,
  useSourceListings,
  useStoreCatalog,
  useStoreSource,
} from '../lib/catalog';
import {
  describePairOutcome,
  type PairNotice,
  presentPairWithGuidance,
  useSession,
} from '../lib/session';
import { TEXT, TYPE } from '../lib/theme';
import { formatBytes, relativeTime } from '../lib/utils';
import { humanizePermission } from '../lib/webapp-permissions';
import { hasSettings, useWebapps } from '../lib/webapps';
import type { StoreScreenProps } from '../navigation';

type Props = StoreScreenProps<'StoreApp'>;

export function StoreAppScreen({ navigation, route }: Props) {
  const { deviceId, appId, sourceUrl } = route.params;
  const listing =
    useSourceListings(sourceUrl, deviceId).find(l => l.app.id === appId) ??
    null;

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

  return (
    <StoreAppPage
      listing={listing}
      deviceId={deviceId}
      navigation={navigation}
    />
  );
}

function StoreAppPage({
  listing,
  deviceId,
  navigation,
}: {
  listing: CatalogAppListing;
  deviceId: string | null;
  navigation: Props['navigation'];
}) {
  const { app, newestCompatible, installedVersion, updateAvailable } = listing;
  const libVersion = useSession(s => deviceLibVersion(s, deviceId));
  const installed =
    useWebapps(deviceId).list.find(
      w => w.id.toLowerCase() === app.id.toLowerCase(),
    ) ?? null;
  const store = useStoreCatalog(deviceId);
  const source = useStoreSource(listing.sourceUrl, store.sources);
  const more = moreFromAuthor(store.listings, listing);
  const { installing, waiting, queued, failed, install } = useStoreInstall(
    deviceId,
    listing,
  );

  const [asking, setAsking] = useState<AppVersion | null>(null);
  const [asked, setAsked] = useState<AppVersion | null>(null);
  const [pairing, setPairing] = useState(false);
  const [pairNotice, setPairNotice] = useState<PairNotice | null>(null);
  const [showAllVersions, setShowAllVersions] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const ordered = sortNewestFirst(app.versions);
  const shown = newestCompatible ?? ordered[0] ?? null;

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

  const openAuthor = () =>
    navigation.push('StoreBrowse', {
      deviceId,
      query: {
        filter: { kind: 'author', author: app.author },
        sort: 'popular',
      },
    });

  const openListing = (other: CatalogAppListing) =>
    navigation.push('StoreApp', {
      deviceId,
      appId: other.app.id,
      sourceUrl: other.sourceUrl,
    });

  const question = asked
    ? describeVersionInstall({
        version: asked,
        newest: newestCompatible,
        installedVersion,
      })
    : null;

  const released = shown ? releasedAtInstant(shown.released_at) : null;
  const notes = changelogText(shown?.changelog);
  const category =
    appCategories(app).length > 0 &&
    !isLauncher(listing) &&
    !isOverlay(listing);

  const installButton = (
    <Button
      onPress={() => void install(newestCompatible)}
      disabled={!deviceId || !newestCompatible || waiting != null}
      loading={waiting != null && !queued}
    >
      {!newestCompatible
        ? 'needs newer firmware'
        : queued
          ? 'queued'
          : updateAvailable
            ? `update to ${newestCompatible.version}`
            : `install ${newestCompatible.version}`}
    </Button>
  );

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

      <View className="mb-4 flex-row items-center gap-3.5">
        <CatalogIcon url={app.icon} name={app.name} size={72} />
        <View className="min-w-0 flex-1">
          <Text
            className="font-display text-fg"
            style={TEXT.hero}
            numberOfLines={2}
          >
            {app.name}
          </Text>
          <Press onPress={openAuthor} hitSlop={6} className="self-start">
            <Text className="mt-0.5 font-sans text-muted" style={TEXT.body}>
              {app.author}
            </Text>
          </Press>
          {installedVersion ? (
            <View className="mt-1.5">
              <Pill tone="ok">{`installed ${installedVersion}`}</Pill>
            </View>
          ) : null}
        </View>
      </View>

      <View className="mb-4 gap-2">
        {installing ? (
          <View className="border border-rule bg-screen p-4">
            <View className="flex-row items-baseline justify-between">
              <Text className="font-sans text-fg" style={TEXT.hint}>
                {installing.stepLabel ?? 'installing'}
              </Text>
              <Text className="font-mono text-soft" style={TEXT.hint}>
                {installing.percent}%
              </Text>
            </View>
            <Progress percent={installing.percent} className="mt-2" />
          </View>
        ) : deviceId && installed ? (
          <>
            {updateAvailable ? installButton : null}
            <SwitchWebappButton
              deviceId={deviceId}
              id={installed.id}
              onError={setActionError}
            />
            {fillsSlot(installed, 'launcher') ? (
              <SlotAction
                deviceId={deviceId}
                id={installed.id}
                slot="launcher"
              />
            ) : null}
            {fillsSlot(installed, 'overlay') ? (
              <SlotAction
                deviceId={deviceId}
                id={installed.id}
                slot="overlay"
              />
            ) : null}
            {hasSettings(installed) ? (
              <Button
                variant="secondary"
                icon="SlidersHorizontal"
                onPress={() =>
                  navigation.navigate('apps', {
                    screen: 'WebappSettings',
                    params: {
                      deviceId,
                      id: installed.id,
                      name: installed.name,
                    },
                    initial: false,
                  })
                }
              >
                open {installed.name} settings
              </Button>
            ) : null}
          </>
        ) : (
          installButton
        )}
        {deviceId == null ? (
          <Note tone="warn" action="pair" onAction={() => void pair()}>
            connect a car thing to install
          </Note>
        ) : null}
        <PairNote notice={pairNotice} />
        {failed ? <Note tone="err">{failed}</Note> : null}
        {actionError ? <Note tone="err">{actionError}</Note> : null}
      </View>

      <View className="mb-4 flex-row border border-rule bg-screen">
        <Stat value={String(listing.installs)} label="installs" />
        <Stat
          value={listingKindLabel(listing) ?? 'app'}
          label={category ? 'category' : 'kind'}
        />
        <Stat
          value={shown ? formatBytes(shown.download.size) : '-'}
          label="size"
          last
        />
      </View>

      <Screenshots urls={app.screenshots} name={app.name} />

      <Text className="mb-8 font-sans text-near" style={TEXT.body}>
        {app.description}
      </Text>

      {shown && notes ? (
        <View className="mb-8">
          <SectionHeader
            title={`new in ${shown.version}`}
            action={released === null ? undefined : relativeTime(released)}
          />
          <Text className="font-sans text-muted" style={TEXT.body}>
            {notes}
          </Text>
        </View>
      ) : null}

      <View className="mb-8">
        <SectionHeader title="permissions" />
        {!shown || shown.permissions.length === 0 ? (
          <SectionEmpty>no permissions</SectionEmpty>
        ) : (
          <ListGroup>
            {shown.permissions.map(p => (
              <ListRow
                key={p}
                icon={humanizePermission(p).icon}
                title={permissionLabel(p)}
              />
            ))}
          </ListGroup>
        )}
      </View>

      {more.length > 0 ? (
        <View className="mb-8">
          <SectionHeader
            title={`more from ${app.author}`}
            action="see all"
            onAction={openAuthor}
          />
          <HorizontalRail>
            {more.map(other => (
              <ListingTile
                key={other.app.id}
                listing={other}
                caption={
                  listingKindLabel(other) ?? installsLabel(other.installs)
                }
                onOpen={() => openListing(other)}
              />
            ))}
          </HorizontalRail>
        </View>
      ) : null}

      {ordered.length > 1 ? (
        <View className="mb-8">
          <SectionHeader
            title="versions"
            action={showAllVersions ? 'hide' : `show all ${ordered.length}`}
            onAction={() => setShowAllVersions(v => !v)}
          />
          {showAllVersions ? (
            <ListGroup>
              {ordered.map(v => (
                <VersionRow
                  key={v.version}
                  version={v}
                  installedVersion={installedVersion}
                  libVersion={libVersion}
                  busy={waiting?.version ?? null}
                  blocked={deviceId == null || installing != null}
                  onInstall={() => start(v)}
                />
              ))}
            </ListGroup>
          ) : null}
        </View>
      ) : null}

      {app.homepage || app.source ? (
        <View className="mb-3 flex-row gap-4">
          {app.homepage ? <Link label="homepage" url={app.homepage} /> : null}
          {app.source ? <Link label="source code" url={app.source} /> : null}
        </View>
      ) : null}

      <Press
        testID="store-app-source"
        onPress={() =>
          navigation.push('StoreSource', {
            deviceId,
            url: source.url,
            name: source.name,
          })
        }
        hitSlop={8}
        className="self-start"
      >
        <Text className="font-mono text-dim" style={{ fontSize: TYPE.eyebrow }}>
          {sourceLine(source)}
        </Text>
      </Press>
    </ScrollScreen>
  );
}

function Stat({
  value,
  label,
  last = false,
}: {
  value: string;
  label: string;
  last?: boolean;
}) {
  return (
    <View
      className={`flex-1 px-3 py-2.5 ${last ? '' : 'border-r border-rule'}`}
    >
      <Text className="font-sans text-fg" style={TEXT.row} numberOfLines={1}>
        {value}
      </Text>
      <Text className="font-mono uppercase text-dim" style={TEXT.eyebrow}>
        {label}
      </Text>
    </View>
  );
}

function Link({ label, url }: { label: string; url: string }) {
  return (
    <Press onPress={() => void Linking.openURL(url)} hitSlop={8}>
      <Text className="font-mono text-soft" style={TEXT.hint}>
        {label} ↗
      </Text>
    </Press>
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
  const released = releasedAtInstant(version.released_at);
  const notes = changelogText(version.changelog);

  return (
    <Press onPress={onInstall} disabled={!actionable || busy != null}>
      <View className="px-4 py-3">
        <View className="flex-row items-center gap-2">
          <Text className="font-mono text-fg" style={TEXT.hint}>
            {version.version}
          </Text>
          {installed ? <Pill tone="ok">installed</Pill> : null}
          {!installed && !compatible ? (
            <Pill tone="warn">needs newer firmware</Pill>
          ) : null}
          <Text className="ml-auto font-sans text-dim" style={TEXT.hint}>
            {released === null ? '' : relativeTime(released)}
          </Text>
        </View>
        {notes ? (
          <Text className="mt-1 font-sans text-muted" style={TEXT.hint}>
            {notes}
          </Text>
        ) : null}
        <View className="mt-1 flex-row items-center gap-2">
          <Text className="min-w-0 flex-1 font-sans text-dim" style={TEXT.hint}>
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
