import { Button, ListGroup, ListRow, Pill, SectionEmpty, SectionHeader, Spinner, describeError } from '@bridgething/ui';
import { check, type Update } from '@tauri-apps/plugin-updater';
import type { VNode } from 'preact';
import { useState } from 'preact/hooks';

import { bytes } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { restart } from '../lib/lifecycle.ts';
import { Progress } from './Progress.tsx';
import { ErrorNote, Hint, Section } from './Screen.tsx';

type State =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current' }
  | { kind: 'found'; update: Update }
  | { kind: 'downloading'; update: Update; received: number; total: number | null }
  | { kind: 'ready' }
  | { kind: 'unavailable'; reason: string };

export function SelfUpdate({ version }: { version: string | undefined }): VNode {
  const [state, setState] = useState<State>({ kind: 'idle' });

  const look = async () => {
    setState({ kind: 'checking' });
    try {
      const update = await check();
      setState(update?.available ? { kind: 'found', update } : { kind: 'current' });
    } catch (reason) {
      setState({ kind: 'unavailable', reason: describeError(reason) });
    }
  };

  const install = async (update: Update) => {
    setState({ kind: 'downloading', update, received: 0, total: null });
    try {
      let received = 0;
      let total: number | null = null;
      await update.downloadAndInstall(progress => {
        if (progress.event === 'Started') total = progress.data.contentLength ?? null;
        else if (progress.event === 'Progress') received += progress.data.chunkLength;
        setState({ kind: 'downloading', update, received, total });
      });
      setState({ kind: 'ready' });
    } catch (reason) {
      setState({ kind: 'unavailable', reason: describeError(reason) });
    }
  };

  return (
    <Section>
      <SectionHeader
        title="this app"
        hint={version ? `bridgething desktop v${version}` : undefined}
        action={state.kind === 'downloading' ? undefined : 'check now'}
        pending={state.kind === 'checking'}
        onAction={() => void look()}
      />

      {state.kind === 'found' || state.kind === 'downloading' ? (
        <div class="border border-accent/30 bg-accent-soft">
          <div class="flex items-start gap-3 px-4 py-3">
            <div class="flex min-w-0 flex-1 flex-col gap-1">
              <span class="text-row text-off-white">version {state.update.version} is available</span>
              {state.update.date ? <span class="font-mono text-hint text-muted">{state.update.date}</span> : null}
              {state.kind === 'downloading' ? (
                <span class="font-mono text-hint text-muted">
                  {bytes(state.received)}
                  {state.total === null ? '' : ` of ${bytes(state.total)}`}
                </span>
              ) : null}
            </div>
            {state.kind === 'found' ? (
              <Button
                size="sm"
                variant="primary"
                icon={<Icon name="download" />}
                onClick={() => void install(state.update)}>
                download and install
              </Button>
            ) : (
              <Spinner />
            )}
          </div>
          {state.update.body ? (
            <p class="m-0 border-t border-accent/20 px-4 py-3 text-hint leading-relaxed whitespace-pre-wrap text-muted">
              {state.update.body}
            </p>
          ) : null}
          {state.kind === 'downloading' ? (
            <Progress percent={state.total === null ? null : Math.round((state.received / state.total) * 100)} />
          ) : null}
        </div>
      ) : state.kind === 'ready' ? (
        <ListGroup>
          <ListRow
            icon={<Icon name="refresh" />}
            iconTint="ok"
            title="the update is installed"
            subtitle="restart to run it"
            trailing={
              <Button size="sm" variant="primary" onClick={() => void restart()}>
                restart now
              </Button>
            }
          />
        </ListGroup>
      ) : state.kind === 'current' ? (
        <ListGroup>
          <ListRow
            icon={<Icon name="check" />}
            iconTint="ok"
            title="up to date"
            subtitle="nothing newer is published"
            trailing={<Pill tone="ok">current</Pill>}
          />
        </ListGroup>
      ) : state.kind === 'unavailable' ? (
        <>
          <SectionEmpty>updates are unavailable</SectionEmpty>
          <ErrorNote>{state.reason}</ErrorNote>
          <Hint>the release feed is unsigned until a signing key is published, so this check cannot succeed yet.</Hint>
        </>
      ) : (
        <SectionEmpty>{state.kind === 'checking' ? <Spinner class="mx-auto" /> : 'not checked yet'}</SectionEmpty>
      )}
    </Section>
  );
}
