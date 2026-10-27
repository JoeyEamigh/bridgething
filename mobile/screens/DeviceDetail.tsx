import {
  type BridgethingLauncherGesture,
  type BridgethingResumeTarget,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { LAUNCHER_GESTURE_OPTIONS } from '@bridgething/ui/launcher-gesture';
import { useEffect, useState } from 'react';
import { Platform, View } from 'react-native';

import { ConfirmSheet } from '../components/ConfirmSheet';
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
import {
  forgetKnownDevice,
  getSession,
  knownDevices,
  setCompanionLaunch,
  setDeviceName,
  setLauncherGesture,
  useSession,
} from '../lib/session';
import type { AppsScreenProps } from '../navigation';

type Props = AppsScreenProps<'DeviceDetail'>;

export function DeviceDetailScreen({ route, navigation }: Props) {
  const { deviceId } = route.params;
  const ledger = useSession(s => s.ledger);
  const peers = useSession(s => s.peers);
  const meta = useSession(s => s.deviceMeta[deviceId]);
  const device = knownDevices(ledger, peers).find(d => d.id === deviceId);

  const [renameOpen, setRenameOpen] = useState(false);
  const [forgetOpen, setForgetOpen] = useState(false);
  const [renameError, setRenameError] = useState<string | null>(null);

  if (!device) {
    return (
      <ScrollScreen>
        <ScreenHeader title="not paired" />
      </ScrollScreen>
    );
  }

  const connected = device.peer?.status === 'connected';

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
          <LauncherGestureRow
            deviceId={deviceId}
            gesture={meta?.launcherGesture ?? null}
          />
          {Platform.OS === 'ios' ? (
            <CompanionLaunchRow
              deviceId={deviceId}
              enabled={meta?.companionLaunch ?? null}
            />
          ) : null}
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
      </View>

      <View className="mb-8">
        <SectionHeader title="details" />
        <ListGroup>
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
        if (!cancelled) setEnabled(null);
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
        trailing={
          enabled == null ? undefined : (
            <Switch value={enabled} onValueChange={toggle} />
          )
        }
        disabled={enabled == null}
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
        if (!cancelled) setTarget(null);
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
        below={
          <Segmented
            size="sm"
            options={RESUME_TARGET_OPTIONS}
            value={target}
            onChange={pick}
            disabled={target == null}
          />
        }
      />
      <RowNote notice={failure} />
    </View>
  );
}

function LauncherGestureRow({
  deviceId,
  gesture,
}: {
  deviceId: string;
  gesture: BridgethingLauncherGesture | null;
}) {
  const [pending, setPending] = useState<BridgethingLauncherGesture | null>(
    null,
  );
  const [failure, setFailure] = useState<RowNotice | null>(null);

  useEffect(() => setPending(null), [gesture]);

  const pick = (next: BridgethingLauncherGesture) => {
    setPending(next);
    setFailure(null);
    void setLauncherGesture(deviceId, next).catch((err: unknown) => {
      setPending(null);
      setFailure({ text: describeError(err) });
    });
  };

  return (
    <View>
      <ListRow
        icon="LayoutGrid"
        title="jump back to apps"
        subtitle="m button"
        value={gesture == null ? 'not connected' : undefined}
        disabled={gesture == null}
        below={
          gesture == null ? undefined : (
            <Segmented
              size="sm"
              options={LAUNCHER_GESTURE_OPTIONS}
              value={pending ?? gesture}
              onChange={pick}
            />
          )
        }
      />
      <RowNote notice={failure} />
    </View>
  );
}

function CompanionLaunchRow({
  deviceId,
  enabled,
}: {
  deviceId: string;
  enabled: boolean | null;
}) {
  const [pending, setPending] = useState<boolean | null>(null);
  const [failure, setFailure] = useState<RowNotice | null>(null);

  useEffect(() => setPending(null), [enabled]);

  const toggle = (next: boolean) => {
    setPending(next);
    setFailure(null);
    void setCompanionLaunch(deviceId, next).catch((err: unknown) => {
      setPending(null);
      setFailure({ text: describeError(err) });
    });
  };

  return (
    <View>
      <ListRow
        icon="AppWindow"
        iconTint={(pending ?? enabled) ? 'accent' : 'default'}
        title="open bridgething on reconnect"
        value={enabled == null ? 'not connected' : undefined}
        disabled={enabled == null}
        trailing={
          enabled == null ? undefined : (
            <Switch
              testID="companion-launch"
              value={pending ?? enabled}
              onValueChange={toggle}
            />
          )
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
