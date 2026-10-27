import type { OtaDiscoverManifest, OtaManifestRelease, OtaPollConfig, OtaRun } from '@bridgething/companion-types';
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
import { Disclosure, ErrorNote, Hint, Screen, Section } from '../components/Screen.tsx';
import { useDesktop } from '../desktop.ts';
import { basename, since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { DEFAULT_OTA_POLL_CONFIG, POLL_INTERVALS, intervalLabel, rootUrlOf } from '../lib/ota.ts';
import { pickArtifact } from '../lib/picker.ts';
import { otaManifestFor, otaPoll, otaPollConfig, otaRuns, selectedMeta } from '../stores/session.ts';

const RELEASES_SHOWN = 5;

export function UpdatesScreen(): VNode {
  const session = useSession();
  const active = otaRuns.value.filter(run => run.outcome !== 'succeeded');
  const firmware = otaRuns.value.find(run => !run.outcome && run.lane === 'firmware') ?? null;
  const poll = otaPoll.value;

  const config = otaPollConfig.value;
  const rootUrl = rootUrlOf(config);
  const manifest = otaManifestFor(rootUrl);

  const held = selectedMeta.value;
  const [channel, setChannel] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  const channels = Object.keys(manifest.data.value?.channels ?? {}).sort();
  const chosen = channel ?? held?.channel ?? channels[0] ?? 'stable';
  const installed = held ? `${held.daemonVersion}+image.${held.imageVersion}` : null;

  const check = async () => {
    setChecking(true);
    try {
      await Promise.all([session.checkForOtaUpdate(rootUrl), manifest.refresh()]);
    } finally {
      setChecking(false);
    }
  };

  return (
    <Screen>
      <ScreenHeader
        title="updates"
        trailing={
          <Button size="sm" icon={<Icon name="refresh" />} loading={checking} onClick={() => void check()}>
            check
          </Button>
        }
      />

      {active.length > 0 ? (
        <Section>
          <SectionHeader title="in progress" />
          <div class="flex flex-col gap-2">
            {active.map(run => (
              <OtaRunCard
                key={run.runId}
                run={run}
                onDismiss={() => {
                  void session.dismissOtaRun(run.lane);
                }}
              />
            ))}
          </div>
        </Section>
      ) : null}

      <Section>
        <SectionHeader title="releases" hint={installed ? `installed ${installed}` : undefined} />
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
          firmware={firmware}
          deviceChannel={held?.channel ?? null}
          rootUrl={rootUrl}
        />
      </Section>

      <Section>
        <SectionHeader title="automatic updates" hint={`checked ${since(poll?.lastPolledAt ?? null)}`} />
        <PollConfig config={config} />
        {poll?.error ? <ErrorNote>{poll.error}</ErrorNote> : null}
      </Section>

      <Disclosure title="advanced">
        <ManifestRoot config={config} />
        <PushLocalArtifact />
      </Disclosure>
    </Screen>
  );
}

function Releases({
  manifest,
  channel,
  loading,
  error,
  installed,
  firmware,
  deviceChannel,
  rootUrl,
}: {
  manifest: OtaDiscoverManifest | null;
  channel: string;
  loading: boolean;
  error: string | undefined;
  installed: string | null;
  firmware: OtaRun | null;
  deviceChannel: string | null;
  rootUrl: string;
}): VNode {
  const session = useSession();
  const [pending, setPending] = useState<OtaManifestRelease | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [all, setAll] = useState(false);

  if (error) return <ErrorNote>{error}</ErrorNote>;
  if (!manifest) {
    return <SectionEmpty>{loading ? <Spinner class="mx-auto" /> : 'manifest unavailable'}</SectionEmpty>;
  }

  const held = manifest.channels[channel];
  if (!held) return <SectionEmpty>{`no ${channel} channel`}</SectionEmpty>;

  const releases = held.releases.map(version => manifest.releases[version]).filter(entry => entry !== undefined);
  if (releases.length === 0) return <SectionEmpty>no releases</SectionEmpty>;

  const apply = async (release: OtaManifestRelease) => {
    setPending(null);
    setFailure(null);
    try {
      await session.applyOtaUpdate(channel, release.version, rootUrl);
    } catch (reason) {
      setFailure(describeError(reason));
    }
  };

  return (
    <>
      <ListGroup>
        {(all ? releases : releases.slice(0, RELEASES_SHOWN)).map(release => {
          const current = installed === release.version;
          const yanked = release.yanked !== null;
          const locked = yanked || current || firmware !== null;
          return (
            <ListRow
              key={release.version}
              title={release.version}
              subtitle={yanked ? (release.yanked ?? undefined) : undefined}
              disabled={locked}
              trailing={
                firmware?.releaseVersion === release.version ? (
                  <Pill tone="accent">installing</Pill>
                ) : yanked ? (
                  <Pill tone="err">yanked</Pill>
                ) : current ? (
                  <Pill tone="ok">installed</Pill>
                ) : held.latest === release.version ? (
                  <Pill tone="accent">latest</Pill>
                ) : release.deprecated ? (
                  <Pill tone="warn">old</Pill>
                ) : undefined
              }
              onClick={locked ? undefined : () => setPending(release)}
            />
          );
        })}
      </ListGroup>
      {releases.length > RELEASES_SHOWN ? (
        <Button size="sm" variant="ghost" class="mt-2" onClick={() => setAll(!all)}>
          {all ? 'fewer' : `all ${releases.length}`}
        </Button>
      ) : null}
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}

      <Dialog
        open={pending !== null}
        onClose={() => setPending(null)}
        title={pending ? `install ${pending.version}?` : ''}
        subtitle="the device reboots when it finishes"
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
        {deviceChannel && deviceChannel !== channel ? (
          <p class="m-0 text-body text-muted">{`moves the device from ${deviceChannel} to ${channel}`}</p>
        ) : null}
      </Dialog>
    </>
  );
}

function usePollWriter(config: OtaPollConfig | null): (partial: Partial<OtaPollConfig>) => void {
  const session = useSession();
  const held = config ?? DEFAULT_OTA_POLL_CONFIG;
  return partial => {
    void session.setOtaPollConfig({
      intervalSeconds: partial.intervalSeconds ?? held.intervalSeconds,
      autoPush: partial.autoPush ?? held.autoPush,
      rootUrl: partial.rootUrl ?? rootUrlOf(config),
    });
  };
}

function PollConfig({ config }: { config: OtaPollConfig | null }): VNode {
  const held = config ?? DEFAULT_OTA_POLL_CONFIG;
  const write = usePollWriter(config);

  return (
    <ListGroup>
      <ListRow
        icon={<Icon name="download" />}
        iconTint={held.autoPush ? 'accent' : 'default'}
        title="install automatically"
        subtitle="firmware and store apps"
        trailing={
          <Switch checked={held.autoPush} label="install automatically" onChange={autoPush => write({ autoPush })} />
        }
      />
      <ListRow
        icon={<Icon name="clock" />}
        title="check every"
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
  );
}

function ManifestRoot({ config }: { config: OtaPollConfig | null }): VNode {
  const write = usePollWriter(config);
  const [draft, setDraft] = useState(rootUrlOf(config));

  return (
    <Section>
      <Field
        label="manifest root"
        value={draft}
        onInput={setDraft}
        onCommit={value => write({ rootUrl: value.trim() })}
        icon={<Icon name="globe" />}
        type="url"
      />
    </Section>
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
      if (answer.kind === 'completed') setOutcome(`${basename(artifact)} pushed`);
      else if (answer.kind === 'failed') setFailure(answer.reason);
      else setFailure('push stopped');
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
      <div class="flex items-end gap-2">
        <Field
          class="flex-1"
          label="push a daemon binary"
          value={path}
          onInput={setPath}
          onSubmit={value => value.trim() && void push(value.trim())}
          icon={<Icon name="file" />}
          placeholder="/path/to/bridgething"
          clearable
        />
        <Button icon={<Icon name="upload" />} loading={busy} onClick={() => void browse()}>
          choose
        </Button>
      </div>
      {outcome ? <Hint>{outcome}</Hint> : null}
      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}
