import type { AuthState, CapabilityFlags, ProviderInfo, VoiceModelState } from '@bridgething/companion-types';
import {
  Button,
  ListGroup,
  ListRow,
  Pill,
  ScreenHeader,
  SectionEmpty,
  SectionHeader,
  Spinner,
  Switch,
  useSession,
} from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { Progress } from '../components/Progress.tsx';
import { ErrorNote, Hint, Screen, Section } from '../components/Screen.tsx';
import { SelfUpdate } from '../components/SelfUpdate.tsx';
import { useDesktop } from '../desktop.ts';
import { bytes } from '../lib/format.ts';
import { Icon, type IconName } from '../lib/icons.tsx';
import { quit } from '../lib/lifecycle.ts';
import { autostart, setAutostart } from '../stores/autostart.ts';
import {
  autoResume,
  capabilities,
  capabilitySupport,
  debugLogging,
  deviceMeta,
  hostInfo,
  libraryProvider,
  providerPriority,
  providers,
  snapshot,
  voiceModel,
} from '../stores/session.ts';

const REPO_URL = 'https://github.com/JoeyEamigh/bridgething';

type Flag = {
  key: keyof CapabilityFlags;
  icon: IconName;
  title: string;
  subtitle: string;
  unsupported?: (os: string) => string;
};

const NOTIFICATION_LIMITS: Record<string, string> = {
  macos: 'macOS has no public way for an app to read the notifications going to other apps',
  windows: 'Windows hands its notification listener only to packaged store installs, which this build is not',
};

const FLAGS: Flag[] = [
  { key: 'netFetch', icon: 'globe', title: 'HTTP proxy', subtitle: 'webapps reach the internet through this computer' },
  { key: 'netWs', icon: 'wifi', title: 'WebSocket proxy', subtitle: 'relayed websockets for webapps' },
  {
    key: 'audioTts',
    icon: 'speaker',
    title: 'speaker',
    subtitle: 'let the Car Thing play sound through this computer',
  },
  { key: 'geo', icon: 'pin', title: 'location', subtitle: 'share this computer notion of where it is' },
  {
    key: 'notifications',
    icon: 'bell',
    title: 'notifications',
    subtitle: 'forward desktop notifications to the device',
    unsupported: os => NOTIFICATION_LIMITS[os] ?? 'this desktop exposes no notifications for bridgething to read',
  },
  {
    key: 'voiceModel',
    icon: 'mic',
    title: 'voice understanding',
    subtitle: 'handle free-form requests, not just the built-in phrases · downloads a model',
  },
];

export function SettingsScreen(): VNode {
  const list = providers.value;
  const preferred = providerPriority.value[0] ?? null;
  const browsing = libraryProvider.value;
  const flags = capabilities.value;
  const support = capabilitySupport.data.value;
  const host = hostInfo.value;
  const meta = deviceMeta.value;
  const failure = snapshot.error.value;
  const pending = snapshot.pending.value;

  return (
    <Screen>
      <ScreenHeader
        eyebrow="companion"
        title="settings"
        subtitle="accounts, what this computer offers, and what it is."
      />

      <Section>
        <SectionHeader title="accounts" hint="tap a signed-in account to prefer it for browsing" />
        {list.filter(provider => provider.available).length === 0 ? (
          <SectionEmpty>{pending ? <Spinner class="mx-auto" /> : 'no providers are compiled in'}</SectionEmpty>
        ) : (
          <ListGroup>
            {list
              .filter(provider => provider.available)
              .map(provider => (
                <ProviderRow
                  key={provider.id}
                  provider={provider}
                  preferred={preferred === provider.id}
                  browsing={browsing === provider.id}
                  order={list.map(entry => entry.id)}
                />
              ))}
          </ListGroup>
        )}
        {list
          .filter(provider => provider.authState.kind !== 'idle')
          .map(provider => (
            <AuthCard key={`auth-${provider.id}`} provider={provider} />
          ))}
        {failure ? <ErrorNote>{failure}</ErrorNote> : null}
      </Section>

      <Section>
        <SectionHeader
          title="what this computer offers"
          hint="capabilities webapps on the Car Thing are allowed to reach for"
        />
        <CapabilityRows flags={flags} support={support} />
        <Hint>a capability that is off here stays off no matter what a webapp asks for.</Hint>
      </Section>

      <Section>
        <SectionHeader title="device" />
        {meta.length > 0 ? (
          <ListGroup>
            {meta.map(entry => (
              <ListRow
                key={entry.deviceId}
                icon={<Icon name="device" />}
                title={entry.meta.nickname ?? entry.meta.modelName}
                subtitle={`${entry.meta.osName} ${entry.meta.osVersion} · image ${entry.meta.imageVersion} · ${entry.meta.channel}`}
                value={`v${entry.meta.daemonVersion}`}
              />
            ))}
            <AutoResumeRow />
          </ListGroup>
        ) : (
          <SectionEmpty>connect a Car Thing to see its details</SectionEmpty>
        )}
      </Section>

      <Section>
        <SectionHeader title="this app" />
        <ListGroup>
          <AutostartRow />
          <DebugLoggingRow />
          <ListRow
            icon={<Icon name="signOut" />}
            title="quit bridgething"
            subtitle="the Car Thing keeps running whatever is on its screen"
            destructive
            onClick={() => void quit()}
          />
        </ListGroup>
      </Section>

      <SelfUpdate version={host?.appVersion} />

      <Section>
        <SectionHeader title="about" />
        <ListGroup>
          <ListRow
            icon={<Icon name="info" />}
            title={host?.appName ?? 'bridgething desktop'}
            subtitle={host ? `${host.osName} ${host.osVersion}` : 'reading host info'}
            value={host ? `v${host.appVersion}` : undefined}
          />
          {host ? (
            <ListRow
              icon={<Icon name="link" />}
              title="protocol"
              subtitle={`lib ${host.libVersion} · wire ${host.libbridgethingVersion}`}
              value={host.adapterVersion}
            />
          ) : null}
          <ListRow icon={<Icon name="file" />} title="source" subtitle={<span class="select-all">{REPO_URL}</span>} />
        </ListGroup>
      </Section>
    </Screen>
  );
}

function ProviderRow({
  provider,
  preferred,
  browsing,
  order,
}: {
  provider: ProviderInfo;
  preferred: boolean;
  browsing: boolean;
  order: string[];
}): VNode {
  const session = useDesktop();
  const [busy, setBusy] = useState(false);

  const promote = async () => {
    setBusy(true);
    try {
      await session.setProviderPriority([provider.id, ...order.filter(id => id !== provider.id)]);
    } finally {
      setBusy(false);
    }
  };

  const signIn = async () => {
    setBusy(true);
    try {
      await session.connectProvider(provider.id);
    } catch {
      // the failure lands on the provider's authState
    } finally {
      setBusy(false);
    }
  };

  if (!provider.connected) {
    return (
      <ListRow
        icon={<Icon name="signIn" />}
        iconTint="accent"
        title={`sign in to ${provider.displayName}`}
        subtitle={healthLine(provider)}
        trailing={busy ? <Spinner /> : undefined}
        chevron
        disabled={busy}
        onClick={() => void signIn()}
      />
    );
  }

  return (
    <ListRow
      icon={<Icon name="user" />}
      iconTint="accent"
      title={provider.displayName}
      subtitle={browsing ? 'signed in · browsing' : (healthLine(provider) ?? 'signed in')}
      trailing={
        <span class="flex shrink-0 items-center gap-2">
          {preferred ? <Pill tone="ok">preferred</Pill> : null}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              void session.disconnectProvider(provider.id);
            }}>
            sign out
          </Button>
        </span>
      }
      disabled={busy}
      onClick={preferred ? undefined : () => void promote()}
    />
  );
}

function healthLine(provider: ProviderInfo): string | undefined {
  const { kind, retryAfterSeconds } = provider.serviceHealth;
  if (kind === 'ok') return undefined;
  if (kind === 'unreachable') return 'the service is unreachable';
  return retryAfterSeconds === null ? 'rate limited' : `rate limited, retry in ${retryAfterSeconds}s`;
}

function AuthCard({ provider }: { provider: ProviderInfo }): VNode | null {
  const session = useDesktop();
  const state: AuthState = provider.authState;

  if (state.kind === 'pending') {
    const url = state.verificationUrlComplete ?? state.verificationUrl;
    return (
      <div class="mt-3 border border-accent/30 bg-accent-soft px-4 py-3">
        <span class="flex items-center gap-2 font-mono text-eyebrow tracking-[0.18em] text-accent uppercase">
          <Spinner />
          waiting on {provider.displayName}
        </span>
        {state.userCode ? (
          <p class="m-0 mt-3 font-mono text-title tracking-[0.2em] text-off-white select-all">{state.userCode}</p>
        ) : null}
        {url ? <p class="m-0 mt-2 break-all text-hint text-accent select-all">{url}</p> : null}
        <div class="mt-3">
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              void session.cancelProviderAuth(provider.id);
            }}>
            cancel
          </Button>
        </div>
      </div>
    );
  }

  if (state.kind === 'failed') {
    return (
      <div class="mt-3 border border-err/30 bg-err-soft px-4 py-3">
        <span class="font-mono text-eyebrow tracking-[0.18em] text-err uppercase">sign-in failed</span>
        <p class="m-0 mt-1 text-body text-err">{state.message ?? 'unknown error'}</p>
        <div class="mt-3">
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              void session.connectProvider(provider.id);
            }}>
            try again
          </Button>
        </div>
      </div>
    );
  }

  return null;
}

function CapabilityRows({ flags, support }: { flags: CapabilityFlags | null; support: CapabilityFlags | null }): VNode {
  const model = voiceModel.value;

  if (!flags || !support) {
    return (
      <SectionEmpty>
        <Spinner class="mx-auto" />
      </SectionEmpty>
    );
  }

  return (
    <>
      <ListGroup>
        {FLAGS.map(flag => (
          <FlagRow
            key={flag.key}
            flag={flag}
            flags={flags}
            support={support}
            model={flag.key === 'voiceModel' ? model : undefined}
          />
        ))}
      </ListGroup>
      {flags.voiceModel && support.voiceModel ? <VoiceModelNote state={model} /> : null}
    </>
  );
}

function FlagRow({
  flag,
  flags,
  support,
  model,
}: {
  flag: Flag;
  flags: CapabilityFlags;
  support: CapabilityFlags;
  model?: VoiceModelState | null;
}): VNode {
  const session = useSession();

  if (!support[flag.key]) {
    return (
      <ListRow
        icon={<Icon name={flag.icon} />}
        title={flag.title}
        subtitle={flag.unsupported?.(hostInfo.value?.osName ?? '') ?? 'this computer has no way to provide this'}
        trailing={<Pill>unavailable</Pill>}
        disabled
      />
    );
  }

  const live = flags[flag.key] ? (model ?? undefined) : undefined;

  return (
    <ListRow
      icon={<Icon name={flag.icon} />}
      iconTint={flags[flag.key] ? 'accent' : 'default'}
      title={flag.title}
      subtitle={live ? modelLine(live) : flag.subtitle}
      value={live?.status === 'ready' ? (live.version ?? undefined) : undefined}
      trailing={
        <Switch
          checked={flags[flag.key]}
          label={flag.title}
          onChange={next => {
            void session.setCapabilityFlags({ ...flags, [flag.key]: next });
          }}
        />
      }
    />
  );
}

function modelLine(state: VoiceModelState): string {
  switch (state.status) {
    case 'downloading':
      return `downloading the model · ${bytes(state.receivedBytes)} of ${bytes(state.totalBytes)}`;
    case 'ready':
      return 'the model is installed and understanding runs on this computer';
    case 'failed':
      return 'the model could not be installed';
    default:
      return 'waiting for the model to download';
  }
}

function VoiceModelNote({ state }: { state: VoiceModelState | null }): VNode | null {
  if (!state) return null;

  if (state.status === 'downloading') {
    return (
      <Progress
        class="mt-3"
        percent={state.totalBytes > 0 ? Math.round((state.receivedBytes / state.totalBytes) * 100) : null}
      />
    );
  }

  if (state.status === 'failed') {
    return <ErrorNote>{state.error ?? 'the voice model could not be installed'}</ErrorNote>;
  }

  return null;
}

function AutostartRow(): VNode {
  const enabled = autostart.data.value;

  return (
    <ListRow
      icon={<Icon name="power" />}
      iconTint={enabled ? 'accent' : 'default'}
      title="start with this computer"
      subtitle="the tray app comes back on its own after a reboot"
      trailing={
        <Switch
          checked={enabled}
          label="start with this computer"
          disabled={autostart.pending.value}
          onChange={next => {
            void setAutostart(next);
          }}
        />
      }
    />
  );
}

function DebugLoggingRow(): VNode {
  const session = useDesktop();
  const enabled = debugLogging.data.value;

  return (
    <ListRow
      icon={<Icon name="terminal" />}
      iconTint={enabled ? 'accent' : 'default'}
      title="verbose logging"
      subtitle="keep debug detail in this app's log, for when something needs chasing"
      trailing={
        <Switch
          checked={enabled}
          label="verbose logging"
          onChange={next => {
            void (async () => {
              await session.setDebugLogging(next);
              await debugLogging.refresh();
            })();
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
      subtitle="wake the music app and pick up where it left off"
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
