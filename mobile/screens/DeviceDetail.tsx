import { type BridgethingResumeTarget } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useEffect, useState } from 'react';
import { View } from 'react-native';

import { ConfirmSheet } from '../components/ConfirmSheet';
import { Field } from '../components/Field';
import { LinkRecovery } from '../components/LinkRecovery';
import { ListGroup } from '../components/ListGroup';
import { ListRow } from '../components/ListRow';
import { Note } from '../components/Note';
import { OtaCard } from '../components/OtaCard';
import { RenameSheet } from '../components/RenameSheet';
import { RowNote, type RowNotice } from '../components/RowNote';
import { ScreenHeader } from '../components/ScreenHeader';
import { ScrollScreen } from '../components/ScrollScreen';
import { SectionHeader } from '../components/SectionHeader';
import { Segmented } from '../components/Segmented';
import { Switch } from '../components/ui/switch';
import { linkSummary } from '../lib/devices';
import { rootUrlOf } from '../lib/ota';
import {
  forgetKnownDevice,
  getSession,
  knownDevices,
  patchOtaPollConfig,
  setDeviceName,
  useSession,
} from '../lib/session';
import { DEFAULT_OTA_POLL_CONFIG, DEFAULT_OTA_ROOT_URL } from '../lib/storage';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'DeviceDetail'>;

export function DeviceDetailScreen({ route, navigation }: Props) {
  const { deviceId } = route.params;
  const ledger = useSession(s => s.ledger);
  const peers = useSession(s => s.peers);
  const meta = useSession(s => s.deviceMeta[deviceId]);
  const pollConfig = useSession(s => s.otaPollConfig);
  const device = knownDevices(ledger, peers).find(d => d.id === deviceId);

  const [renameOpen, setRenameOpen] = useState(false);
  const [forgetOpen, setForgetOpen] = useState(false);
  const [renameError, setRenameError] = useState<string | null>(null);
  const [hostDraft, setHostDraft] = useState(pollConfig?.rootUrl ?? '');

  const heldHost = pollConfig?.rootUrl ?? '';
  useEffect(() => setHostDraft(heldHost), [heldHost]);

  if (!device) {
    return (
      <ScrollScreen>
        <ScreenHeader
          title="not paired"
          subtitle="this phone no longer knows this car thing."
        />
      </ScrollScreen>
    );
  }

  const connected = device.peer?.status === 'connected';

  const commitHost = (raw: string) => {
    const next = raw.trim().replace(/\/+$/, '');
    if (next === rootUrlOf(pollConfig)) return;
    setHostDraft(next);
    void patchOtaPollConfig({ rootUrl: next || undefined });
  };

  const forget = () => {
    setForgetOpen(false);
    forgetKnownDevice(deviceId);
    navigation.goBack();
  };

  return (
    <ScrollScreen>
      <RenameSheet
        visible={renameOpen}
        title="rename your car thing"
        message="this renames the device and shows on its screen."
        initialValue={device.nickname ?? ''}
        placeholder={device.peer?.name ?? device.displayName}
        onSubmit={value => {
          setRenameError(null);
          void setDeviceName(deviceId, value).catch((err: unknown) =>
            setRenameError(describeError(err)),
          );
        }}
        onClose={() => setRenameOpen(false)}
      />
      <ConfirmSheet
        visible={forgetOpen}
        title="forget this car thing?"
        body={
          connected
            ? 'this will unlink it now. you can pair it again later.'
            : 'this phone will stop reconnecting to it. you can pair it again later.'
        }
        confirmLabel="forget"
        destructive
        onConfirm={forget}
        onClose={() => setForgetOpen(false)}
      />

      <ScreenHeader title={device.displayName} subtitle={linkSummary(device)} />

      {device.peer?.status === 'linkFailed' ? (
        <View className="mb-8">
          <SectionHeader title="link" />
          <LinkRecovery peer={device.peer} />
        </View>
      ) : null}

      <View className="mb-8">
        <ListGroup>
          <ListRow
            icon="Pencil"
            title="name"
            value={device.nickname ?? device.displayName}
            chevron
            onPress={() => setRenameOpen(true)}
          />
          <AutoResumeRow deviceId={deviceId} />
          <ResumeTargetRow deviceId={deviceId} />
        </ListGroup>
        {renameError ? (
          <Note className="mt-2" tone="err">
            {renameError}
          </Note>
        ) : null}
      </View>

      <View className="mb-8">
        <SectionHeader title="firmware" />
        <OtaCard
          deviceId={deviceId}
          onPickVersion={() =>
            navigation.navigate('OtaVersions', {
              deviceId,
              channel: meta?.channel || 'stable',
            })
          }
        />
        <ListGroup className="mt-3">
          <ListRow
            icon="ArrowDownToLine"
            title="install updates automatically"
            subtitle="firmware and store apps"
            trailing={
              <Switch
                value={pollConfig?.autoPush ?? DEFAULT_OTA_POLL_CONFIG.autoPush}
                onValueChange={autoPush => patchOtaPollConfig({ autoPush })}
              />
            }
          />
        </ListGroup>
      </View>

      <View className="mb-8">
        <SectionHeader title="details" />
        <ListGroup>
          <ListRow
            title="model"
            value={meta?.modelName ?? device.displayName}
          />
          {device.serialNumber ? (
            <ListRow title="id" value={device.serialNumber} />
          ) : null}
          {meta ? (
            <ListRow
              title="version"
              value={`${meta.daemonVersion}+image.${meta.imageVersion}`}
            />
          ) : null}
          {meta ? <ListRow title="release track" value={meta.channel} /> : null}
        </ListGroup>
      </View>

      <View className="mb-8">
        <SectionHeader title="advanced" />
        <Field
          label="update host"
          icon="Globe"
          testID="update-host"
          value={hostDraft}
          onChangeText={setHostDraft}
          onCommit={commitHost}
          placeholder={DEFAULT_OTA_ROOT_URL}
          autoCapitalize="none"
          autoCorrect={false}
          keyboardType="url"
          hint={`probably don't change this. leave empty for ${DEFAULT_OTA_ROOT_URL}.`}
          clearable
        />
      </View>

      <ListGroup>
        <ListRow
          icon="Trash2"
          iconTint="err"
          title="forget this car thing"
          destructive
          onPress={() => setForgetOpen(true)}
        />
      </ListGroup>
    </ScrollScreen>
  );
}

function AutoResumeRow({ deviceId }: { deviceId: string }) {
  const session = getSession();
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [failure, setFailure] = useState<RowNotice | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const value = await session.isDeviceAutoResumeEnabled(deviceId);
        if (!cancelled) setEnabled(value);
      } catch {
        if (!cancelled) setEnabled(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [session, deviceId]);

  const toggle = (next: boolean) => {
    setEnabled(next);
    setFailure(null);
    void session.setDeviceAutoResume(deviceId, next).catch((err: unknown) => {
      setEnabled(!next);
      setFailure({ text: describeError(err) });
    });
  };

  return (
    <View>
      <ListRow
        icon="Play"
        iconTint={enabled ? 'accent' : 'default'}
        title="resume playback on connect"
        subtitle="wake your music app and pick up where you left off"
        trailing={
          <Switch
            value={enabled ?? true}
            onValueChange={toggle}
            disabled={enabled == null}
          />
        }
      />
      <RowNote notice={failure} />
    </View>
  );
}

function ResumeTargetRow({ deviceId }: { deviceId: string }) {
  const session = getSession();
  const [target, setTarget] = useState<BridgethingResumeTarget | null>(null);
  const [failure, setFailure] = useState<RowNotice | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const value = await session.deviceResumeTarget(deviceId);
        if (!cancelled) setTarget(value);
      } catch {
        if (!cancelled) setTarget('phoneOnly');
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [session, deviceId]);

  const pick = (next: BridgethingResumeTarget) => {
    const prior = target;
    setTarget(next);
    setFailure(null);
    void session.setDeviceResumeTarget(deviceId, next).catch((err: unknown) => {
      setTarget(prior);
      setFailure({ text: describeError(err) });
    });
  };

  return (
    <View>
      <ListRow
        icon="Smartphone"
        title="resume on"
        subtitle="any speaker lets playback start on whatever spotify last used"
        trailing={
          <Segmented
            size="sm"
            options={RESUME_TARGET_OPTIONS}
            value={target ?? 'phoneOnly'}
            onChange={pick}
          />
        }
      />
      <RowNote notice={failure} />
    </View>
  );
}

const RESUME_TARGET_OPTIONS: ReadonlyArray<{
  value: BridgethingResumeTarget;
  label: string;
}> = [
  { value: 'phoneOnly', label: 'phone only' },
  { value: 'anySpeaker', label: 'any speaker' },
];
