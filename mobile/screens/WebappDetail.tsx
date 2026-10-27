import { fillsSlot } from '@bridgething/catalog';
import { describeError } from '@bridgething/ui/errors';
import { useFocusEffect } from '@react-navigation/native';
import { useCallback, useState } from 'react';
import { Text, View } from 'react-native';

import { Button } from '../components/Button';
import { Chevron } from '../components/Chevron';
import { ConfirmSheet } from '../components/ConfirmSheet';
import { Icon } from '../components/Icon';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { Note } from '../components/Note';
import { Pill } from '../components/Pill';
import { Press } from '../components/Press';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionEmpty, SectionHeader } from '../components/SectionHeader';
import { SlotAction } from '../components/SlotAction';
import { SwitchWebappButton } from '../components/SwitchWebappButton';
import { Spinner } from '../components/Spinner';
import { WebappIcon } from '../components/WebappIcon';
import { useUpdates } from '../lib/catalog';
import {
  getSession,
  peerDisplayName,
  usePeer,
  useSession,
} from '../lib/session';
import { selectSettings } from '../lib/settings-host';
import { TEXT } from '../lib/theme';
import { humanizePermission } from '../lib/webapp-permissions';
import { hasSettings, useWebapps } from '../lib/webapps';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'WebappDetail'>;

export function WebappDetailScreen({ navigation, route }: Props) {
  const session = getSession();
  const { deviceId, id } = route.params;

  const peer = usePeer(deviceId);
  const ledger = useSession(s => s.ledger);

  const { list } = useWebapps(deviceId);
  const info = list.find(w => w.id === id) ?? null;
  const update =
    useUpdates(deviceId).find(
      u => u.appId.toLowerCase() === id.toLowerCase(),
    ) ?? null;

  const settable = info !== null && hasSettings(info);

  useFocusEffect(
    useCallback(() => {
      if (settable) selectSettings({ deviceId, id });
    }, [deviceId, id, settable]),
  );

  const [uninstalling, setUninstalling] = useState(false);
  const [askUninstall, setAskUninstall] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const uninstall = async () => {
    setAskUninstall(false);
    setUninstalling(true);
    setActionError(null);
    try {
      await session.uninstallWebapp(deviceId, id);
      navigation.goBack();
    } catch (err) {
      setActionError(describeError(err));
    } finally {
      setUninstalling(false);
    }
  };

  if (!info) {
    return (
      <ScrollScreen contentContainerStyle={{ paddingTop: 12 }}>
        <View className="mb-6 flex-row items-center gap-4 border border-rule bg-screen p-4">
          <View className="h-16 w-16 items-center justify-center border border-rule bg-neutral-soft">
            <Spinner />
          </View>
          <Text className="font-mono text-dim" style={TEXT.hint}>
            {id}
          </Text>
        </View>
      </ScrollScreen>
    );
  }

  const builtin = info.source === 'builtin';

  return (
    <ScrollScreen contentContainerStyle={{ paddingTop: 12 }}>
      <ConfirmSheet
        visible={askUninstall}
        title={`uninstall ${info.name}?`}
        body="it leaves your car thing now."
        detail={`detail: ${info.id} ${info.version}`}
        confirmLabel="uninstall"
        destructive
        busy={uninstalling}
        onConfirm={() => void uninstall()}
        onClose={() => setAskUninstall(false)}
      />

      <View className="mb-6 flex-row items-center gap-4 border border-rule bg-screen p-4">
        <WebappIcon
          deviceId={deviceId}
          id={info.id}
          iconHash={info.iconHash}
          name={info.name}
          size={64}
        />
        <View className="min-w-0 flex-1">
          <Text
            className="font-display text-fg"
            style={TEXT.title}
            numberOfLines={2}
          >
            {info.name}
          </Text>
          <Text className="mt-0.5 font-mono text-muted" style={TEXT.hint}>
            v{info.version}
          </Text>
          <View className="mt-2 flex-row flex-wrap gap-1.5">
            {builtin ? (
              <Pill tone="neutral">built-in</Pill>
            ) : (
              <Pill tone="accent">installed</Pill>
            )}
            {peer ? (
              <Pill tone="neutral">{peerDisplayName(peer, ledger)}</Pill>
            ) : null}
          </View>
        </View>
      </View>

      {info.description ? (
        <Text className="mb-6 px-1 font-sans text-muted" style={TEXT.body}>
          {info.description}
        </Text>
      ) : null}

      {update ? (
        <Press
          onPress={() =>
            navigation.navigate('store', {
              screen: 'StoreApp',
              params: {
                deviceId,
                appId: update.appId,
                sourceUrl: update.sourceUrl,
              },
            })
          }
          className="mb-6"
        >
          <View className="flex-row items-center gap-3 border border-accent bg-accent-soft px-4 py-3">
            <Icon name="ArrowUpCircle" tone="accent" size={18} />
            <View className="flex-1">
              <Text className="font-sans text-fg" style={TEXT.hint}>
                update available
              </Text>
              <Text className="mt-0.5 font-mono text-muted" style={TEXT.hint}>
                v{update.installedVersion} → v{update.target.version}
              </Text>
            </View>
            <Chevron />
          </View>
        </Press>
      ) : null}

      <View className="mb-8 gap-2">
        <SwitchWebappButton
          deviceId={deviceId}
          id={info.id}
          onError={setActionError}
        />
        {actionError ? <Note tone="err">{actionError}</Note> : null}
      </View>

      {fillsSlot(info, 'launcher') || fillsSlot(info, 'overlay') ? (
        <View className="mb-8 gap-2">
          {fillsSlot(info, 'launcher') ? (
            <SlotAction deviceId={deviceId} id={info.id} slot="launcher" />
          ) : null}
          {fillsSlot(info, 'overlay') ? (
            <SlotAction deviceId={deviceId} id={info.id} slot="overlay" />
          ) : null}
        </View>
      ) : null}

      <View className="mb-8">
        {settable ? (
          <Button
            onPress={() =>
              navigation.navigate('WebappSettings', {
                deviceId,
                id,
                name: info.name,
              })
            }
            variant="secondary"
            icon="SlidersHorizontal"
          >
            open {info.name} settings
          </Button>
        ) : (
          <SectionEmpty>no settings</SectionEmpty>
        )}
      </View>

      {info.permissions.length > 0 ? (
        <View className="mb-8">
          <SectionHeader title="permissions" />
          <ListGroup>
            {info.permissions.map(p => {
              const meta = humanizePermission(p);
              return <ListRow key={p} icon={meta.icon} title={meta.title} />;
            })}
          </ListGroup>
        </View>
      ) : null}

      {!builtin ? (
        <ListGroup>
          <ListRow
            icon="Trash2"
            iconTint="err"
            title="uninstall"
            destructive
            loading={uninstalling}
            onPress={() => setAskUninstall(true)}
          />
        </ListGroup>
      ) : null}
    </ScrollScreen>
  );
}
