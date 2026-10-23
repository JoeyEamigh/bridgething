import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';
import { Linking, Platform, View } from 'react-native';

import { AccountsSection } from '../components/accounts/AccountsSection';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { Note } from '../components/Note';
import { AncsRows } from '../components/permissions/AncsRows';
import { BackgroundLocationRow } from '../components/permissions/BackgroundLocationRow';
import { BatteryExemptionRow } from '../components/permissions/BatteryExemptionRow';
import { CapabilityRow } from '../components/permissions/CapabilityRow';
import { DefaultDialerRow } from '../components/permissions/DefaultDialerRow';
import { LocationRow } from '../components/permissions/LocationRow';
import { NotificationAccessRow } from '../components/permissions/NotificationAccessRow';
import { ScreenHeader } from '../components/ScreenHeader';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionHeader } from '../components/SectionHeader';
import { UpdatesSection } from '../components/UpdatesSection';
import { VoiceSection } from '../components/VoiceSection';
import {
  CAPABILITIES,
  type CapabilityGroup,
  type CapabilityKey,
} from '../lib/capabilities';
import { updateCapabilityFlags, useSession } from '../lib/session';
import type { SettingsScreenProps } from '../navigation';

const REPO_URL = 'https://github.com/JoeyEamigh/bridgething';
const DISCORD_URL = 'https://tl.mt/d';

type Props = SettingsScreenProps<'Settings'>;
type Failure = { group: CapabilityGroup; text: string };

export function SettingsScreen({ navigation }: Props) {
  const flags = useSession(s => s.capabilityFlags);
  const host = useSession(s => s.hostInfo);
  const [failure, setFailure] = useState<Failure | null>(null);

  const android = Platform.OS === 'android';

  const setFlag = (key: CapabilityKey) => (value: boolean) => {
    setFailure(null);
    void updateCapabilityFlags({ ...flags, [key]: value }).catch(
      (err: unknown) =>
        setFailure({
          group: CAPABILITIES[key].group,
          text: describeError(err),
        }),
    );
  };

  return (
    <ScrollScreen>
      <ScreenHeader
        title="settings"
        subtitle="dials and knobs to turn and tweak"
      />

      <AccountsSection />

      <View className="mb-8">
        <SectionHeader
          title="connections"
          hint="what this phone has to allow for your car thing."
        />
        <ListGroup>
          {android ? (
            <NotificationAccessRow
              value={flags.notifications}
              onChange={setFlag('notifications')}
            />
          ) : (
            <CapabilityRow
              capability="notifications"
              value={flags.notifications}
              onChange={setFlag('notifications')}
            />
          )}
          {android ? null : <AncsRows />}
          {android ? (
            <BackgroundLocationRow locationShared={flags.geo} />
          ) : null}
          {android ? <DefaultDialerRow /> : null}
          {android ? <BatteryExemptionRow /> : null}
        </ListGroup>
        <GroupNote failure={failure} group="connections" />
      </View>

      <View className="mb-8">
        <SectionHeader
          title="app access"
          hint="what your phone lends the apps on your car thing."
        />
        <ListGroup>
          <LocationRow value={flags.geo} onChange={setFlag('geo')} />
          <CapabilityRow
            capability="netFetch"
            value={flags.netFetch}
            onChange={setFlag('netFetch')}
          />
          <CapabilityRow
            capability="netWs"
            value={flags.netWs}
            onChange={setFlag('netWs')}
          />
          <CapabilityRow
            capability="audioTts"
            value={flags.audioTts}
            onChange={setFlag('audioTts')}
          />
        </ListGroup>
        <GroupNote failure={failure} group="sharing" />
      </View>

      <VoiceSection />

      <UpdatesSection />

      <View className="mb-8">
        <SectionHeader title="diagnostics" />
        <ListGroup>
          <ListRow
            icon="Activity"
            iconTint="accent"
            title="debug inspector"
            subtitle="internal state, useful for filing a bug"
            chevron
            onPress={() => navigation.navigate('Debug')}
          />
          <ListRow
            icon="TerminalSquare"
            iconTint="accent"
            title="logs"
            subtitle="stream device or phone logs"
            chevron
            onPress={() => navigation.navigate('Logs')}
          />
        </ListGroup>
      </View>

      <View className="mb-2">
        <SectionHeader title="about" />
        <ListGroup>
          <ListRow
            icon="LifeBuoy"
            iconTint="accent"
            title={`${host?.appName ?? 'bridgething'} companion`}
            subtitle={host ? `${host.osName} ${host.osVersion}` : 'loading…'}
            value={host ? `v${host.appVersion}` : undefined}
          />
          {host ? (
            <ListRow
              icon="RadioTower"
              iconTint="accent"
              title="protocol"
              subtitle={`lib ${host.libVersion} · wire ${host.libbridgethingVersion} · diagnostic detail`}
              value={host.adapterVersion}
            />
          ) : null}
          <ListRow
            icon="Code"
            iconTint="accent"
            title="source"
            subtitle={REPO_URL.replace('https://', '')}
            chevron
            onPress={() => Linking.openURL(REPO_URL)}
          />
          <ListRow
            icon="MessageCircle"
            iconTint="accent"
            title="community discord"
            subtitle="tl.mt/d"
            chevron
            onPress={() => Linking.openURL(DISCORD_URL)}
          />
        </ListGroup>
      </View>
    </ScrollScreen>
  );
}

function GroupNote({
  failure,
  group,
}: {
  failure: Failure | null;
  group: CapabilityGroup;
}) {
  if (!failure || failure.group !== group) return null;
  return (
    <Note tone="err" className="mt-2">
      {failure.text}
    </Note>
  );
}
