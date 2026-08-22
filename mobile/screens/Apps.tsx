import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from '../components/Button';
import { Caret } from '../components/Caret';
import { LinkRecovery } from '../components/LinkRecovery';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { OtaCard } from '../components/OtaCard';
import { PairNote } from '../components/PairNote';
import { Pill } from '../components/Pill';
import { Press } from '../components/Press';
import { CompanionUpdateBanner } from '../components/CompanionUpdateBanner';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { SideloadSheet } from '../components/SideloadSheet';
import { WebappIcon } from '../components/WebappIcon';
import { useUpdates } from '../lib/catalog';
import { linkSummary } from '../lib/devices';
import { useOta, useOtaProgress } from '../lib/ota';
import {
  describePairOutcome,
  knownDevices,
  type PairNotice,
  presentPairWithGuidance,
  useSession,
} from '../lib/session';
import { TEXT } from '../lib/theme';
import { appTiles, useWebapps } from '../lib/webapps';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'Apps'>;

export function AppsScreen({ navigation }: Props) {
  const peers = useSession(s => s.peers);
  const ledger = useSession(s => s.ledger);
  const known = knownDevices(ledger, peers);
  const primary = known[0] ?? null;
  const deviceId = primary?.id ?? null;
  const connected = primary?.peer?.status === 'connected';

  const [sideloadOpen, setSideloadOpen] = useState(false);

  if (known.length === 0) {
    return (
      <ScrollScreen>
        <CompanionUpdateBanner className="mb-4" />
        <NoDeviceHero onBrowseStore={() => navigation.navigate('store')} />
      </ScrollScreen>
    );
  }

  const broken = known.find(d => d.peer?.status === 'linkFailed');

  return (
    <ScrollScreen>
      <SideloadSheet
        visible={sideloadOpen}
        deviceId={deviceId}
        onClose={() => setSideloadOpen(false)}
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
        </ListGroup>
        {broken?.peer ? (
          <View className="mt-3">
            <LinkRecovery peer={broken.peer} />
          </View>
        ) : null}
      </View>

      {deviceId ? <FirmwareAttention deviceId={deviceId} /> : null}

      <InstalledApps
        deviceId={deviceId}
        connected={connected}
        navigation={navigation}
      />

      <Press onPress={() => setSideloadOpen(true)} className="mt-4">
        <View className="flex-row items-center justify-between px-1 py-3">
          <Text className="font-mono uppercase text-dim" style={TEXT.eyebrow}>
            install from url or file
          </Text>
          <Text className="font-mono text-dim" style={TEXT.body}>
            ›
          </Text>
        </View>
      </Press>

      <View className="mt-4">
        <PairButton label="pair another car thing" variant="secondary" />
      </View>
    </ScrollScreen>
  );
}

function FirmwareAttention({ deviceId }: { deviceId: string }) {
  const available = useOta(s => s.available[deviceId]);
  const progress = useOtaProgress(deviceId);

  if (!available?.releaseVersion && !progress) return null;

  return (
    <View className="mb-8">
      <SectionHeader title="firmware" />
      <OtaCard deviceId={deviceId} />
    </View>
  );
}

function InstalledApps({
  deviceId,
  connected,
  navigation,
}: {
  deviceId: string | null;
  connected: boolean;
  navigation: Props['navigation'];
}) {
  const { list, active } = useWebapps(deviceId);
  const updates = useUpdates(deviceId);

  const tiles = appTiles(
    list,
    active?.id ?? null,
    updates.map(u => u.appId),
  );

  return (
    <View>
      <SectionHeader
        title="on your car thing"
        action={deviceId ? 'home screen' : undefined}
        onAction={() =>
          deviceId && navigation.navigate('WebappSlots', { deviceId })
        }
      />
      {tiles.length === 0 ? (
        <SectionEmpty>
          {connected
            ? 'waiting for your car thing to list its apps'
            : 'connect your car thing to see what is on it'}
        </SectionEmpty>
      ) : (
        <ListGroup>
          {tiles.map(tile => (
            <Press
              key={tile.id}
              onPress={() =>
                deviceId &&
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
                <Text className="font-mono text-dim" style={TEXT.body}>
                  ›
                </Text>
              </View>
            </Press>
          ))}
        </ListGroup>
      )}
    </View>
  );
}

function PairButton({
  label,
  variant,
}: {
  label: string;
  variant: 'primary' | 'secondary';
}) {
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

  return (
    <View className="gap-2">
      <Button onPress={pair} loading={pairing} icon="Cable" variant={variant}>
        {label}
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
      <Text className="mt-3 font-sans text-muted" style={TEXT.body}>
        pair one to get started, or look through the store
      </Text>
      <View className="mt-6 gap-2">
        <PairButton label="pair a car thing" variant="primary" />
        <Button onPress={onBrowseStore} icon="Store" variant="secondary">
          browse the store
        </Button>
      </View>
    </View>
  );
}
