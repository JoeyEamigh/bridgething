import { Button, Dialog, ListGroup, ListRow, SectionEmpty, SectionHeader, Spinner } from '@bridgething/ui';
import type { VNode } from 'preact';

import { useDesktop } from '../desktop.ts';
import { bytes, day, since } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { postponedUpdate, promptedUpdate, selfUpdate } from '../stores/session.ts';
import { Progress } from './Progress.tsx';
import { ErrorNote, Section } from './Screen.tsx';

export function SelfUpdate(): VNode {
  const session = useDesktop();
  const state = selfUpdate.data.value;

  return (
    <Section>
      <SectionHeader
        title="app update"
        action={state.kind === 'downloading' || state.kind === 'installing' ? undefined : 'check'}
        pending={state.kind === 'checking'}
        onAction={() => void session.checkSelfUpdate()}
      />

      {state.kind === 'downloading' ? (
        <div class="border border-accent/30 bg-accent-soft">
          <div class="flex items-start gap-3 px-4 py-3">
            <div class="flex min-w-0 flex-1 flex-col gap-1">
              <span class="text-row text-off-white">downloading {state.version}</span>
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
              <span class="text-row text-off-white">{state.version} ready</span>
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
          />
        </ListGroup>
      ) : state.kind === 'unavailable' ? (
        <>
          <SectionEmpty>unavailable</SectionEmpty>
          <ErrorNote>{state.reason}</ErrorNote>
        </>
      ) : (
        <SectionEmpty>{state.kind === 'checking' ? <Spinner class="mx-auto" /> : 'not checked'}</SectionEmpty>
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
      title={ready ? `${ready.version} ready` : ''}
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
      {ready?.notes ? <p class="m-0 text-hint leading-relaxed whitespace-pre-wrap text-muted">{ready.notes}</p> : null}
    </Dialog>
  );
}
