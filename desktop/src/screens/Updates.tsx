import type { OtaDiscoverManifest, OtaManifestRelease, OtaPollConfig } from '@bridgething/companion-types';
import {
  Button,
  Dialog,
  Field,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionEmpty,
  SectionHeader,
  Segmented,
  Spinner,
  Switch,
  describeError,
  useSession,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { OtaRunCard } from '../components/OtaRunCard.tsx';
import { ErrorNote, Hint, Screen, Section } from '../components/Screen.tsx';
import { useDesktop } from '../desktop.ts';
import { basename, since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { DEFAULT_OTA_POLL_CONFIG, POLL_INTERVALS, intervalLabel, rootUrlOf } from '../lib/ota.ts';
import { pickArtifact } from '../lib/picker.ts';
import {
  otaAvailable,
  otaManifestFor,
  otaPoll,
  otaPollConfig,
  otaRuns,
  selectedDevice,
  selectedMeta,
} from '../stores/session.ts';

export function UpdatesScreen(): VNode {
  const session = useSession();
  const runs = otaRuns.value;
  const available = otaAvailable.value;
  const poll = otaPoll.value;

  const config = otaPollConfig.value;
  const rootUrl = rootUrlOf(config);
  const manifest = otaManifestFor(rootUrl);

  const deviceId = selectedDevice.data.value;
  const held = selectedMeta.value;
  const [channel, setChannel] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  const channels = Object.keys(manifest.data.value?.channels ?? {}).sort();
  const chosen = channel ?? held?.channel ?? channels[0] ?? 'stable';
  const installed = held ? `${held.daemonVersion}+image.${held.imageVersion}` : null;

  const check = async () => {
    setChecking(true);
    try {
      await session.checkForOtaUpdate(rootUrl);
    } finally {
      setChecking(false);
    }
  };

  return (
    <Screen>
      <ScreenHeader
        eyebrow="ota"
        title="updates"
        subtitle="what the release manifest offers, and what the core is pushing right now."
        trailing={
          <Button size="sm" icon={<Icon name="refresh" />} loading={checking} onClick={() => void check()}>
            check now
          </Button>
        }
      />

      {runs.length > 0 ? (
        <Section>
          <SectionHeader title="in flight" />
          <div class="flex flex-col gap-2">
            {runs.map(run => (
              <OtaRunCard
                key={run.runId}
                run={run}
                onDismiss={() => {
                  void session.dismissOtaRun();
                }}
              />
            ))}
          </div>
        </Section>
      ) : null}

      <Section>
        <SectionHeader
          title="releases"
          hint={installed ? `installed: ${installed}` : 'nothing connected'}
          action="reload"
          pending={manifest.pending.value}
          onAction={manifest.refresh}
        />
        {channels.length > 1 ? (
          <div class="mb-3">
            <Segmented options={channels} value={chosen} label="channel" size="sm" onChange={setChannel} />
          </div>
        ) : null}
        <Releases
          manifest={manifest.data.value}
          channel={chosen}
          loading={manifest.pending.value}
          error={manifest.error.value}
          installed={installed}
          deviceChannel={held?.channel ?? null}
          rootUrl={rootUrl}
        />
        {available
          .filter(entry => entry.deviceId === deviceId)
          .map(entry =>
            entry.releaseVersion ? (
              <Hint key={entry.deviceId}>the core has flagged release {entry.releaseVersion} for this device.</Hint>
            ) : null,
          )}
      </Section>

      <Section>
        <SectionHeader title="polling" hint={`last checked ${since(poll?.lastPolledAt ?? null)}`} />
        <PollConfig config={config} />
        {poll?.error ? <ErrorNote>{poll.error}</ErrorNote> : null}
      </Section>

      <PushLocalArtifact />
    </Screen>
  );
}

function Releases({
  manifest,
  channel,
  loading,
  error,
  installed,
  deviceChannel,
  rootUrl,
}: {
  manifest: OtaDiscoverManifest | null;
  channel: string;
  loading: boolean;
  error: string | undefined;
  installed: string | null;
  deviceChannel: string | null;
  rootUrl: string;
}): VNode {
  const session = useSession();
  const [pending, setPending] = useState<OtaManifestRelease | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  if (error) return <ErrorNote>{error}</ErrorNote>;
  if (!manifest) {
    return (
      <SectionEmpty>{loading ? <Spinner class="mx-auto" /> : 'the release manifest could not be read'}</SectionEmpty>
    );
  }

  const held = manifest.channels[channel];
  if (!held) return <SectionEmpty>{`channel '${channel}' is not in the manifest`}</SectionEmpty>;

  const releases = held.releases.map(version => manifest.releases[version]).filter(entry => entry !== undefined);
  if (releases.length === 0) return <SectionEmpty>no releases on this channel</SectionEmpty>;

  const apply = async (release: OtaManifestRelease) => {
    setFailure(null);
    try {
      await session.applyOtaUpdate(channel, release.version, rootUrl);
      setPending(null);
    } catch (reason) {
      setFailure(describeError(reason));
    }
  };

  return (
    <>
      <ListGroup>
        {releases.map(release => {
          const current = installed === release.version;
          const yanked = release.yanked !== null;
          return (
            <ListRow
              key={release.version}
              title={release.version}
              subtitle={yanked ? (release.yanked ?? 'yanked') : `channel ${release.channel}`}
              disabled={yanked || current}
              trailing={
                yanked ? (
                  <Pill tone="err">yanked</Pill>
                ) : current ? (
                  <Pill tone="ok">installed</Pill>
                ) : held.latest === release.version ? (
                  <Pill tone="accent">latest</Pill>
                ) : release.deprecated ? (
                  <Pill tone="warn">old</Pill>
                ) : undefined
              }
              onClick={yanked || current ? undefined : () => setPending(release)}
            />
          );
        })}
      </ListGroup>
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}

      <Dialog
        open={pending !== null}
        onClose={() => setPending(null)}
        title={pending ? `install ${pending.version}?` : ''}
        subtitle="the device reboots into the new image once it is written"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPending(null)}>
              cancel
            </Button>
            <Button variant="primary" onClick={() => pending && void apply(pending)}>
              install
            </Button>
          </>
        }>
        <p class="m-0 text-body text-muted">
          {deviceChannel && deviceChannel !== channel
            ? `this moves the device from the ${deviceChannel} channel to ${channel}.`
            : 'the daemon and the system image are pushed together.'}
        </p>
      </Dialog>
    </>
  );
}

function PollConfig({ config }: { config: OtaPollConfig | null }): VNode {
  const session = useSession();
  const held = config ?? DEFAULT_OTA_POLL_CONFIG;
  const [rootDraft, setRootDraft] = useState(rootUrlOf(config));

  const write = (partial: Partial<OtaPollConfig>) => {
    void session.setOtaPollConfig({
      intervalSeconds: partial.intervalSeconds ?? held.intervalSeconds,
      autoPush: partial.autoPush ?? held.autoPush,
      rootUrl: partial.rootUrl ?? rootUrlOf(config),
    });
  };

  return (
    <div class="flex flex-col gap-2">
      <ListGroup>
        <ListRow
          icon={<Icon name="download" />}
          iconTint={held.autoPush ? 'accent' : 'default'}
          title="install updates automatically"
          subtitle="firmware and store apps. off means every release waits for you to pick it"
          trailing={
            <Switch
              checked={held.autoPush}
              label="install updates automatically"
              onChange={autoPush => write({ autoPush })}
            />
          }
        />
        <ListRow
          icon={<Icon name="clock" />}
          title="check every"
          subtitle="how often the manifest is polled"
          trailing={
            <Segmented
              options={POLL_INTERVALS.map(value => ({ value, label: intervalLabel(Number(value)) }))}
              value={String(held.intervalSeconds)}
              label="poll interval"
              size="sm"
              onChange={value => write({ intervalSeconds: Number(value) })}
            />
          }
        />
      </ListGroup>
      <Field
        label="manifest root"
        value={rootDraft}
        onInput={setRootDraft}
        onCommit={value => write({ rootUrl: value.trim() })}
        icon={<Icon name="globe" />}
        type="url"
        hint="where manifest.json is fetched from"
      />
    </div>
  );
}

function PushLocalArtifact(): VNode {
  const session = useDesktop();
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const push = async (artifact: string) => {
    setBusy(true);
    setOutcome(null);
    setFailure(null);
    try {
      const answer = await session.otaPushDaemon(artifact);
      if (answer.kind === 'completed') setOutcome(`${basename(artifact)} is on the device`);
      else if (answer.kind === 'failed') setFailure(answer.reason);
      else setFailure('the push stopped before it finished');
    } catch (reason) {
      setFailure(describeError(reason));
    } finally {
      setBusy(false);
    }
  };

  const browse = async () => {
    const picked = await pickArtifact('daemon');
    if (!picked) return;
    setPath(picked);
    await push(picked);
  };

  return (
    <Section>
      <SectionHeader title="push a local build" hint="a daemon binary or .swu from this computer" />
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          value={path}
          onInput={setPath}
          onCommit={value => value.trim() && void push(value.trim())}
          icon={<Icon name="file" />}
          placeholder="/path/to/bridgething"
          clearable
        />
        <Button icon={<Icon name="upload" />} loading={busy} onClick={() => void browse()}>
          pick a file
        </Button>
      </div>
      {outcome ? <Hint>{outcome}</Hint> : null}
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      <Hint>this bypasses the manifest entirely. the device takes whatever you hand it.</Hint>
    </Section>
  );
}
