import type { BridgethingWebappInfo } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from '../components/Button';
import { Caret } from '../components/Caret';
import { Chevron } from '../components/Chevron';
import { CompanionUpdateBanner } from '../components/CompanionUpdateBanner';
import { LinkRecovery } from '../components/LinkRecovery';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { OtaCard } from '../components/OtaCard';
import { PairNote } from '../components/PairNote';
import { Pill } from '../components/Pill';
import { Press } from '../components/Press';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { SideloadSheet } from '../components/SideloadSheet';
import { Spinner } from '../components/Spinner';
import { WebappIcon } from '../components/WebappIcon';
import { useUpdates } from '../lib/catalog';
import { linkSummary } from '../lib/devices';
import { useOta, useOtaRun } from '../lib/ota';
import {
  describePairOutcome,
  knownDevices,
  type PairNotice,
  presentPairWithGuidance,
  useSession,
} from '../lib/session';
import { TEXT } from '../lib/theme';
import { appTiles, heldBy, useSlots, useWebapps } from '../lib/webapps';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'Apps'>;

export function AppsScreen({ navigation }: Props) {
  const peers = useSession(s => s.peers);
  const ledger = useSession(s => s.ledger);
  const known = knownDevices(ledger, peers);

  const [sideloadFor, setSideloadFor] = useState<string | null>(null);

  if (known.length === 0) {
    return (
      <ScrollScreen>
        <CompanionUpdateBanner className="mb-4" />
        <NoDeviceHero onBrowseStore={() => navigation.navigate('store')} />
      </ScrollScreen>
    );
  }

  const broken = known.find(d => d.peer?.status === 'linkFailed');
  const many = known.length > 1;

  return (
    <ScrollScreen>
      <SideloadSheet
        visible={sideloadFor != null}
        deviceId={sideloadFor}
        onClose={() => setSideloadFor(null)}
      />

      <CompanionUpdateBanner className="mb-4" />

      <View className="mb-8">
        <ListGroup>
          {known.map(device => (
            <ListRow
              key={device.id}
              icon="Cable"
              iconTint={
                device.peer?.status === 'connected' ? 'accent' : 'default'
              }
              title={device.displayName}
              subtitle={linkSummary(device)}
              chevron
              onPress={() =>
                navigation.navigate('DeviceDetail', { deviceId: device.id })
              }
            />
          ))}
          <PairRow />
        </ListGroup>
        {broken?.peer ? (
          <View className="mt-3">
            <LinkRecovery peer={broken.peer} />
          </View>
        ) : null}
      </View>

      {known.map(device => (
        <View key={device.id} className="mb-8">
          <FirmwareAttention
            deviceId={device.id}
            title={many ? `firmware · ${device.displayName}` : 'firmware'}
          />
          <InstalledApps
            deviceId={device.id}
            title={many ? `on ${device.displayName}` : 'on your car thing'}
            connected={device.peer?.status === 'connected'}
            navigation={navigation}
            onSideload={() => setSideloadFor(device.id)}
          />
        </View>
      ))}
    </ScrollScreen>
  );
}

function FirmwareAttention({
  deviceId,
  title,
}: {
  deviceId: string;
  title: string;
}) {
  const available = useOta(s => s.available[deviceId]);
  const run = useOtaRun(deviceId, 'firmware');

  if (!available?.releaseVersion && !run) return null;

  return (
    <View className="mb-8">
      <SectionHeader title={title} />
      <OtaCard deviceId={deviceId} />
    </View>
  );
}

function InstalledApps({
  deviceId,
  title,
  connected,
  navigation,
  onSideload,
}: {
  deviceId: string;
  title: string;
  connected: boolean;
  navigation: Props['navigation'];
  onSideload: () => void;
}) {
  const { list, active, listed } = useWebapps(deviceId);
  const { slots } = useSlots(deviceId);
  const updates = useUpdates(deviceId);

  const launcherId = heldBy(slots, 'launcher');
  const tiles = appTiles(
    list,
    active?.id ?? null,
    updates.map(u => u.appId),
    launcherId,
  );

  return (
    <View>
      <SectionHeader title={title} />
      {tiles.length > 0 ? (
        <ListGroup>
          {tiles.map(tile => (
            <Press
              key={tile.id}
              testID={`webapp-row-${tile.name}`}
              accessibilityRole="button"
              accessibilityLabel={
                tile.state ? `${tile.name}, ${tile.state.label}` : tile.name
              }
              onPress={() =>
                navigation.navigate('WebappDetail', { deviceId, id: tile.id })
              }
            >
              <View className="flex-row items-center gap-3 px-4 py-2.5">
                <WebappIcon
                  deviceId={deviceId}
                  id={tile.id}
                  iconHash={tile.iconHash}
                  name={tile.name}
                  size={36}
                />
                <Text
                  className="min-w-0 flex-1 font-sans text-fg"
                  style={TEXT.row}
                  numberOfLines={1}
                >
                  {tile.name}
                </Text>
                {tile.state ? (
                  <Pill tone={tile.state.tone}>{tile.state.label}</Pill>
                ) : null}
                <Chevron />
              </View>
            </Press>
          ))}
        </ListGroup>
      ) : connected && !listed ? (
        <View className="items-center border border-rule bg-screen py-6">
          <Spinner />
        </View>
      ) : (
        <SectionEmpty>{connected ? 'no apps' : 'not connected'}</SectionEmpty>
      )}

      <View className="mt-4">
        <ListGroup>
          <ListRow
            icon="LayoutGrid"
            title="home screen and overlay"
            subtitle={launcherName(list, launcherId)}
            chevron
            onPress={() => navigation.navigate('WebappSlots', { deviceId })}
          />
          <ListRow
            icon="Link"
            title="install from url or file"
            chevron
            onPress={onSideload}
          />
        </ListGroup>
      </View>
    </View>
  );
}

function launcherName(
  list: BridgethingWebappInfo[],
  launcherId: string | null,
): string {
  if (!launcherId) return 'built-in hub';
  return list.find(w => w.id === launcherId)?.name ?? 'an installed app';
}

function usePair() {
  const [pairing, setPairing] = useState(false);
  const [notice, setNotice] = useState<PairNotice | null>(null);

  const pair = async () => {
    if (pairing) return;
    setPairing(true);
    setNotice(null);
    try {
      setNotice((await presentPairWithGuidance()).notice);
    } catch (err) {
      setNotice(
        describePairOutcome({ kind: 'error', message: describeError(err) }),
      );
    } finally {
      setPairing(false);
    }
  };

  return { pairing, notice, pair };
}

function PairRow() {
  const { pairing, notice, pair } = usePair();
  return (
    <View>
      <ListRow
        icon="Plus"
        iconTint="accent"
        title="pair another car thing"
        onPress={pair}
        loading={pairing}
      />
      {notice ? (
        <View className="px-4 pb-3">
          <PairNote notice={notice} />
        </View>
      ) : null}
    </View>
  );
}

function PairButton() {
  const { pairing, notice, pair } = usePair();
  return (
    <View className="gap-2">
      <Button onPress={pair} loading={pairing} icon="Cable">
        pair a car thing
      </Button>
      <PairNote notice={notice} />
    </View>
  );
}

function NoDeviceHero({ onBrowseStore }: { onBrowseStore: () => void }) {
  return (
    <View className="px-1 py-8">
      <View className="flex-row items-end gap-2">
        <Text className="font-display text-fg" style={TEXT.hero}>
          no car thing yet
        </Text>
        <Caret />
      </View>
      <View className="mt-6 gap-2">
        <PairButton />
        <Button onPress={onBrowseStore} icon="Store" variant="secondary">
          browse the store
        </Button>
      </View>
    </View>
  );
}
