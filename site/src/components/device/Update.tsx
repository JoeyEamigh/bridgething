import type { OtaRun } from '@bridgething/companion-types';
import { Button, Pill, SectionHeader, StatusStrip, type Tone } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { message } from '../../lib/browser-session';
import { useBrowser, useBrowserQuery } from '../../lib/browser-tier';
import { OTA_ROOT } from '../../lib/wired';
import { ErrorNote, Hint, Progress, Section, bytes } from './Screen';

const TONE: Record<string, Tone> = { failed: 'err', completed: 'ok' };

export function Update({ channel }: { channel: string | null }): VNode {
  const session = useBrowser();
  const available = useBrowserQuery(['ota-available'], s => s.otaAvailable());
  const poll = useBrowserQuery(['ota-poll'], s => s.otaPoll());
  const runs = useBrowserQuery(['ota-runs'], s => s.otaRuns());

  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  const run = (runs.data ?? []).find(entry => entry.kind === 'image' || entry.kind === 'daemon');
  const offered = (available.data ?? [])[0] ?? null;
  const version = offered?.releaseVersion ?? null;

  const apply = async () => {
    if (!version || !channel) return;
    setBusy(true);
    setFailure(null);
    try {
      await session.applyOtaUpdate(channel, version, OTA_ROOT);
    } catch (reason) {
      setFailure(message(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section>
      <SectionHeader
        title="firmware"
        hint={poll.data?.lastPolledAt ? `checked ${new Date(poll.data.lastPolledAt).toLocaleTimeString()}` : undefined}
        action="check again"
        pending={available.loading}
        onAction={() => void session.checkForOtaUpdate(OTA_ROOT)}
      />

      {run ? (
        <RunCard
          run={run}
          onDismiss={() => {
            void session.dismissOtaRun(run.lane);
          }}
        />
      ) : version ? (
        <StatusStrip
          tone="accent"
          title={`release ${version} is ready for this thing`}
          subtitle={`daemon ${offered?.daemonVersion ?? '?'} · image ${offered?.imageVersion ?? '?'} · ${channel ?? '?'}`}
        />
      ) : (
        <StatusStrip
          tone={poll.data?.error ? 'warn' : 'neutral'}
          title={poll.data?.error ? 'could not read the release manifest' : 'running the newest release on its channel'}
          subtitle={poll.data?.error ?? undefined}
        />
      )}

      {version && !run ? (
        <div class="mt-3 flex items-center gap-3">
          <Button variant="primary" loading={busy} disabled={!channel} onClick={() => void apply()}>
            install update
          </Button>
          <Hint>leave it plugged in until it finishes.</Hint>
        </div>
      ) : null}

      {failure ? <ErrorNote>{failure}</ErrorNote> : null}
    </Section>
  );
}

export function RunCard({ run, onDismiss }: { run: OtaRun; onDismiss?: () => void }): VNode {
  const live = run.outcome === null;
  const percent = runPercent(run);

  return (
    <div class="border-rule bg-screen border">
      <div class="flex items-start gap-3 px-4 py-3">
        <div class="flex min-w-0 flex-1 flex-col gap-1">
          <div class="flex items-center gap-2">
            <span class="text-row text-off-white truncate">{runTitle(run)}</span>
            <Pill tone={TONE[run.phase] ?? 'accent'} dot={live}>
              {run.phase}
            </Pill>
          </div>
          {run.stageTotal !== null && run.stageTotal > 0 ? (
            <span class="text-hint text-dim font-mono">
              {bytes(run.stageReceived ?? 0)} of {bytes(run.stageTotal)}
            </span>
          ) : null}
        </div>
        <div class="flex shrink-0 items-center gap-2">
          {percent !== null ? <span class="text-body text-soft font-mono">{percent}%</span> : null}
          {!live && onDismiss ? (
            <Button size="sm" variant="ghost" onClick={onDismiss}>
              dismiss
            </Button>
          ) : null}
        </div>
      </div>
      {live && percent !== null ? <Progress percent={percent} /> : null}
      {run.error ? <p class="border-rule text-hint text-err m-0 border-t px-4 py-2">{run.error}</p> : null}
    </div>
  );
}

export function runTitle(run: OtaRun): string {
  switch (run.kind) {
    case 'image':
      return `image ${run.imageVersion ?? ''}`.trim();
    case 'daemon':
      return `daemon ${run.daemonVersion ?? ''}`.trim();
    default:
      return run.webappName ?? 'webapp bundle';
  }
}

export function runPercent(run: OtaRun): number | null {
  if (run.outcome === 'succeeded') return 100;
  if (run.writePercent !== null) return Math.round(run.writePercent);
  if (run.stageTotal === null || run.stageTotal <= 0) return null;
  return Math.round(((run.stageReceived ?? 0) * 100) / run.stageTotal);
}
