import type { BrightnessMode, DisplaySettings, LauncherGesture, ResumeTarget } from '@bridgething/companion-types';
import { describeError, ListGroup, ListRow, Segmented, Slider, Switch } from '@bridgething/ui';
import {
  BRIGHTNESS_MODE_OPTIONS,
  DIM_AFTER_OPTIONS,
  DIM_LEVEL_STEP_PCT,
  dimAfterPreset,
  dimAfterSeconds,
  dimLevelLabel,
  levelLabel,
  levelPct,
  pctLevel,
  type DimAfter,
} from '@bridgething/ui/display-settings';
import { LAUNCHER_GESTURE_OPTIONS } from '@bridgething/ui/launcher-gesture';
import type { VNode } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { useDesktop } from '../desktop.ts';
import { Icon } from '../lib/icons.tsx';
import { autoResume, followHost, resumeTarget, selectedMeta } from '../stores/session.ts';
import { ErrorNote } from './Screen.tsx';

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
      title="jump back to apps"
      subtitle="m button"
      trailing={
        <Segmented<LauncherGesture>
          size="sm"
          label="jump back to apps"
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

export function DisplaySection(): VNode | null {
  const session = useDesktop();
  const stored = selectedMeta.value?.display ?? null;
  const [pending, setPending] = useState<DisplaySettings | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  if (stored === null) return null;
  const shown = pending ?? stored;
  const following = followHost.data.value;
  const dims = following || shown.dim.afterS !== null;

  const save = (next: DisplaySettings) => {
    setPending(next);
    setFailure(null);
    const settled = () => setPending(current => (current === next ? null : current));
    session
      .setDisplaySettings(next)
      .then(settled)
      .catch((err: unknown) => {
        settled();
        setFailure(describeError(err));
      });
  };

  const follow = (enabled: boolean) => {
    setFailure(null);
    session
      .setFollowHost(enabled)
      .then(() => followHost.refresh())
      .catch((err: unknown) => setFailure(describeError(err)));
  };

  return (
    <>
      <ListGroup>
        <ListRow
          icon={<Icon name="sun" />}
          iconTint="default"
          title="brightness"
          trailing={
            <Segmented<BrightnessMode>
              size="sm"
              label="brightness"
              options={BRIGHTNESS_MODE_OPTIONS}
              value={shown.mode}
              onChange={mode => save({ ...shown, mode })}
            />
          }
        />
        {shown.mode === 'manual' ? (
          <SliderRow
            icon={<span />}
            title="level"
            pct={levelPct(shown.level)}
            step={1}
            label={levelLabel}
            onCommit={pct => save({ ...shown, level: pctLevel(pct) })}
          />
        ) : null}
      </ListGroup>
      <ListGroup class="mt-2">
        <ListRow
          icon={<Icon name="device" />}
          iconTint={following ? 'accent' : 'default'}
          title="dim when this computer is locked or asleep"
          trailing={<Switch checked={following} label="dim when this computer is locked or asleep" onChange={follow} />}
        />
        <ListRow
          icon={<Icon name="clock" />}
          iconTint="default"
          title="dim after"
          trailing={
            <Segmented<DimAfter>
              size="sm"
              label="dim after"
              options={DIM_AFTER_OPTIONS}
              value={dimAfterPreset(shown.dim.afterS)}
              onChange={preset => save({ ...shown, dim: { ...shown.dim, afterS: dimAfterSeconds(preset) } })}
            />
          }
        />
      </ListGroup>
      <ListGroup class="mt-2">
        <SliderRow
          icon={<Icon name="moon" />}
          title="dim level"
          pct={levelPct(shown.dim.level)}
          step={DIM_LEVEL_STEP_PCT}
          label={dimLevelLabel}
          disabled={!dims}
          onCommit={pct => save({ ...shown, dim: { ...shown.dim, level: pctLevel(pct) } })}
        />
      </ListGroup>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </>
  );
}

function SliderRow({
  icon,
  title,
  pct,
  step,
  label,
  disabled = false,
  onCommit,
}: {
  icon: VNode;
  title: string;
  pct: number;
  step: number;
  label: (value: number) => string;
  disabled?: boolean;
  onCommit: (value: number) => void;
}): VNode {
  const [draft, setDraft] = useState(pct);
  useEffect(() => setDraft(pct), [pct]);

  return (
    <ListRow
      icon={icon}
      iconTint="default"
      title={title}
      value={label(draft)}
      disabled={disabled}
      trailing={
        <Slider
          class="w-48 shrink-0"
          label={title}
          value={draft}
          min={0}
          max={100}
          step={step}
          disabled={disabled}
          onInput={setDraft}
          onChange={next => next !== pct && onCommit(next)}
        />
      }
    />
  );
}
