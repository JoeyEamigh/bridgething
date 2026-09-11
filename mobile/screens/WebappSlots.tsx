import { slotCandidates, type WebappSlotName } from '@bridgething/catalog';
import type { BridgethingWebappInfo } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useCallback, useEffect, useState } from 'react';
import { Text, View } from 'react-native';

import { Icon, type IconName } from '../components/Icon';
import { IconBadge } from '../components/IconBadge';
import { ListGroup } from '../components/ListGroup';
import { Note } from '../components/Note';
import { Pill } from '../components/Pill';
import { Press } from '../components/Press';
import { ScreenHeader } from '../components/ScreenHeader';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionHeader } from '../components/SectionHeader';
import { Spinner } from '../components/Spinner';
import { WebappIcon } from '../components/WebappIcon';
import { TEXT } from '../lib/theme';
import { assignSlot, refreshSlots, useSlots, useWebapps } from '../lib/webapps';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'WebappSlots'>;

type SlotFailure = { slot: WebappSlotName; message: string };

export function WebappSlotsScreen({ route }: Props) {
  const deviceId = route.params.deviceId;
  const { list } = useWebapps(deviceId);
  const { slots, error: loadError } = useSlots(deviceId);

  const [failure, setFailure] = useState<SlotFailure | null>(null);
  const [busy, setBusy] = useState<WebappSlotName | null>(null);

  const load = useCallback(() => void refreshSlots(deviceId), [deviceId]);

  useEffect(load, [load]);

  const assign = useCallback(
    async (slot: WebappSlotName, id?: string) => {
      if (busy) return;
      setBusy(slot);
      setFailure(null);
      try {
        await assignSlot(deviceId, slot, id);
      } catch (err) {
        setFailure({ slot, message: describeError(err) });
      } finally {
        setBusy(null);
      }
    },
    [busy, deviceId],
  );

  const launchers = slotCandidates(list, 'launcher');
  const overlays = slotCandidates(list, 'overlay');

  return (
    <ScrollScreen>
      <ScreenHeader
        title="home screen and overlay"
        subtitle="pick which installed app provides each"
      />

      {loadError ? (
        <Note tone="err" action="retry" onAction={load}>
          {loadError}
        </Note>
      ) : !slots ? (
        <View className="items-center py-12">
          <Spinner />
        </View>
      ) : (
        <>
          <SectionHeader
            title="home screen"
            hint={
              launchers.length === 0
                ? 'no installed app offers one yet'
                : undefined
            }
          />
          <ListGroup>
            <BuiltinRow
              label="built-in hub"
              detail="the launcher that ships with bridgething"
              icon="LayoutGrid"
              selected={slots.launcher == null}
              busy={busy === 'launcher'}
              onPress={() => assign('launcher')}
            />
            {launchers.map(w => (
              <CandidateRow
                key={w.id}
                webapp={w}
                deviceId={deviceId}
                selected={slots.launcher === w.id}
                busy={busy === 'launcher'}
                onPress={() => assign('launcher', w.id)}
              />
            ))}
          </ListGroup>
          {failure?.slot === 'launcher' ? (
            <Note className="mt-2" tone="err">
              {failure.message}
            </Note>
          ) : null}

          <View className="mt-10">
            <SectionHeader
              title="system overlay"
              hint={
                overlays.length === 0
                  ? 'no installed app offers one yet'
                  : undefined
              }
            />
            <ListGroup>
              <BuiltinRow
                label="built-in overlay"
                detail="notifications, calls, pairing, volume"
                icon="Layers"
                selected={slots.overlay == null}
                busy={busy === 'overlay'}
                onPress={() => assign('overlay')}
              />
              {overlays.map(w => (
                <CandidateRow
                  key={w.id}
                  webapp={w}
                  deviceId={deviceId}
                  selected={slots.overlay === w.id}
                  busy={busy === 'overlay'}
                  onPress={() => assign('overlay', w.id)}
                />
              ))}
            </ListGroup>
            {failure?.slot === 'overlay' ? (
              <Note className="mt-2" tone="err">
                {failure.message}
              </Note>
            ) : null}
          </View>
        </>
      )}
    </ScrollScreen>
  );
}

function SelectionMark({
  selected,
  busy,
}: {
  selected: boolean;
  busy: boolean;
}) {
  if (busy) return <Spinner />;
  if (!selected) return <View className="h-[18px] w-[18px]" />;
  return <Icon name="Check" tone="accent" size={18} />;
}

function BuiltinRow({
  label,
  detail,
  icon,
  selected,
  busy,
  onPress,
}: {
  label: string;
  detail: string;
  icon: IconName;
  selected: boolean;
  busy: boolean;
  onPress: () => void;
}) {
  return (
    <Press onPress={onPress}>
      <View className="flex-row items-center gap-3 px-4 py-3">
        <IconBadge name={icon} tone="neutral" size="md" />
        <View className="flex-1">
          <View className="flex-row items-center gap-2">
            <Text
              className="flex-shrink font-sans text-fg"
              style={TEXT.row}
              numberOfLines={1}
            >
              {label}
            </Text>
            <Pill tone="neutral">built-in</Pill>
          </View>
          <Text
            className="mt-0.5 font-sans text-muted"
            style={TEXT.hint}
            numberOfLines={1}
          >
            {detail}
          </Text>
        </View>
        <SelectionMark selected={selected} busy={busy} />
      </View>
    </Press>
  );
}

function CandidateRow({
  webapp,
  deviceId,
  selected,
  busy,
  onPress,
}: {
  webapp: BridgethingWebappInfo;
  deviceId: string;
  selected: boolean;
  busy: boolean;
  onPress: () => void;
}) {
  return (
    <Press onPress={onPress}>
      <View className="flex-row items-center gap-3 px-4 py-3">
        <WebappIcon
          deviceId={deviceId}
          id={webapp.id}
          iconHash={webapp.iconHash}
          name={webapp.name}
          size={44}
        />
        <View className="flex-1">
          <Text
            className="font-sans text-fg"
            style={TEXT.row}
            numberOfLines={1}
          >
            {webapp.name}
          </Text>
          <Text
            className="mt-0.5 font-sans text-muted"
            style={TEXT.hint}
            numberOfLines={1}
          >
            v{webapp.version}
            {webapp.description ? ` · ${webapp.description}` : ''}
          </Text>
        </View>
        <SelectionMark selected={selected} busy={busy} />
      </View>
    </Press>
  );
}
