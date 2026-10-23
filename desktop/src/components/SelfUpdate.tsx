import { Button, Dialog, ListGroup, ListRow, Pill, SectionEmpty, SectionHeader, Spinner } from '@bridgething/ui';
import type { VNode } from 'preact';

import { useDesktop } from '../desktop.ts';
import { bytes, day, since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { postponedUpdate, promptedUpdate, selfUpdate } from '../stores/session.ts';
import { Progress } from './Progress.tsx';
import { ErrorNote, Section } from './Screen.tsx';

export function SelfUpdate({ version }: { version: string | undefined }): VNode {
  const session = useDesktop();
  const state = selfUpdate.data.value;

  return (
    <Section>
      <SectionHeader
        title="this app"
        hint={version ? `bridgething desktop v${version}` : undefined}
        action={state.kind === 'downloading' || state.kind === 'installing' ? undefined : 'check now'}
        pending={state.kind === 'checking'}
        onAction={() => void session.checkSelfUpdate()}
      />

      {state.kind === 'downloading' ? (
        <div class="border border-accent/30 bg-accent-soft">
          <div class="flex items-start gap-3 px-4 py-3">
            <div class="flex min-w-0 flex-1 flex-col gap-1">
              <span class="text-row text-off-white">fetching version {state.version}</span>
              <span class="font-mono text-hint text-muted">
                {bytes(state.received)}
                {state.total === null ? '' : ` of ${bytes(state.total)}`}
              </span>
            </div>
            <Spinner />
          </div>
          <Progress percent={state.total === null ? null : Math.round((state.received / state.total) * 100)} />
        </div>
      ) : state.kind === 'ready' || state.kind === 'installing' ? (
        <div class="border border-accent/30 bg-accent-soft">
          <div class="flex items-start gap-3 px-4 py-3">
            <div class="flex min-w-0 flex-1 flex-col gap-1">
              <span class="text-row text-off-white">version {state.version} is ready</span>
              {state.kind === 'ready' && state.date ? (
                <span class="font-mono text-hint text-muted">{day(state.date)}</span>
              ) : null}
            </div>
            {state.kind === 'installing' ? (
              <Spinner />
            ) : (
              <Button
                size="sm"
                variant="primary"
                icon={<Icon name="refresh" />}
                onClick={() => void session.applySelfUpdate()}>
                restart now
              </Button>
            )}
          </div>
          {state.kind === 'ready' && state.notes ? (
            <p class="m-0 border-t border-accent/20 px-4 py-3 text-hint leading-relaxed whitespace-pre-wrap text-muted">
              {state.notes}
            </p>
          ) : null}
        </div>
      ) : state.kind === 'current' ? (
        <ListGroup>
          <ListRow
            icon={<Icon name="check" />}
            iconTint="ok"
            title="up to date"
            subtitle={`checked ${since(state.checkedAt)}`}
            trailing={<Pill tone="ok">current</Pill>}
          />
        </ListGroup>
      ) : state.kind === 'unavailable' ? (
        <>
          <SectionEmpty>updates are unavailable</SectionEmpty>
          <ErrorNote>{state.reason}</ErrorNote>
        </>
      ) : (
        <SectionEmpty>{state.kind === 'checking' ? <Spinner class="mx-auto" /> : 'not checked yet'}</SectionEmpty>
      )}
    </Section>
  );
}

export function SelfUpdatePrompt(): VNode {
  const session = useDesktop();
  const ready = promptedUpdate.value;
  const later = (): void => {
    if (ready) postponedUpdate.value = ready.version;
  };

  return (
    <Dialog
      open={ready !== null}
      onClose={later}
      title={ready ? `version ${ready.version} is ready` : ''}
      subtitle="bridgething closes, installs it and opens again"
      footer={
        <>
          <Button variant="ghost" onClick={later}>
            later
          </Button>
          <Button variant="primary" icon={<Icon name="refresh" />} onClick={() => void session.applySelfUpdate()}>
            restart now
          </Button>
        </>
      }>
      {ready?.notes ? (
        <p class="m-0 text-hint leading-relaxed whitespace-pre-wrap text-muted">{ready.notes}</p>
      ) : (
        <p class="m-0 text-body text-muted">a linked Car Thing drops off for a moment while the app restarts.</p>
      )}
    </Dialog>
  );
}
