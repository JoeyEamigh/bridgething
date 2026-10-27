import type { LauncherGesture, ResumeTarget } from '@bridgething/companion-types';
import { ListGroup, ListRow, Segmented, Switch } from '@bridgething/ui';
import { LAUNCHER_GESTURE_OPTIONS } from '@bridgething/ui/launcher-gesture';
import type { VNode } from 'preact';

import { useDesktop } from '../desktop.ts';
import { Icon } from '../lib/icons.tsx';
import { autoResume, resumeTarget, selectedMeta } from '../stores/session.ts';

export function DeviceSettings(): VNode {
  return (
    <ListGroup>
      <AutoResumeRow />
      <ResumeTargetRow />
      <LauncherGestureRow />
    </ListGroup>
  );
}

function ResumeTargetRow(): VNode {
  const session = useDesktop();
  const target = resumeTarget.data.value;

  return (
    <ListRow
      icon={<Icon name="speaker" />}
      iconTint="default"
      title="resume on"
      trailing={
        <Segmented<ResumeTarget>
          size="sm"
          label="resume on"
          options={[
            { value: 'phoneOnly', label: 'phone only' },
            { value: 'anySpeaker', label: 'any speaker' },
          ]}
          value={target}
          onChange={next => {
            void session.setDeviceResumeTarget(next);
          }}
        />
      }
    />
  );
}

function LauncherGestureRow(): VNode {
  const session = useDesktop();
  const gesture = selectedMeta.value?.launcherGesture ?? 'fivePress';

  return (
    <ListRow
      icon={<Icon name="grid" />}
      iconTint="default"
      title="back to launcher"
      trailing={
        <Segmented<LauncherGesture>
          size="sm"
          label="back to launcher"
          options={LAUNCHER_GESTURE_OPTIONS}
          value={gesture}
          onChange={next => {
            void session.setLauncherGesture(next);
          }}
        />
      }
    />
  );
}

function AutoResumeRow(): VNode {
  const session = useDesktop();
  const enabled = autoResume.data.value;

  return (
    <ListRow
      icon={<Icon name="play" />}
      iconTint={enabled ? 'accent' : 'default'}
      title="resume playback on connect"
      trailing={
        <Switch
          checked={enabled}
          label="resume playback on connect"
          onChange={next => {
            void session.setDeviceAutoResume(next);
          }}
        />
      }
    />
  );
}
