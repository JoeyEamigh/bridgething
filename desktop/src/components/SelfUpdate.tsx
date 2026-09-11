import { Button, ListGroup, ListRow, Pill, SectionEmpty, SectionHeader, Spinner } from '@bridgething/ui';
import type { VNode } from 'preact';

import { bytes } from '../lib/format.ts';
import { Icon } from '../lib/icons.tsx';
import { applySelfUpdate, checkSelfUpdate, selfUpdate } from '../stores/self-update.ts';
import { Progress } from './Progress.tsx';
import { ErrorNote, Section } from './Screen.tsx';

export function SelfUpdate({ version }: { version: string | undefined }): VNode {
  const state = selfUpdate.value;

  return (
    <Section>
      <SectionHeader
        title="this app"
        hint={version ? `bridgething desktop v${version}` : undefined}
        action={state.kind === 'downloading' ? undefined : 'check now'}
        pending={state.kind === 'checking'}
        onAction={() => void checkSelfUpdate()}
      />

      {state.kind === 'downloading' ? (
        <div class="border border-accent/30 bg-accent-soft">
          <div class="flex items-start gap-3 px-4 py-3">
            <div class="flex min-w-0 flex-1 flex-col gap-1">
              <span class="text-row text-off-white">fetching version {state.update.version}</span>
              <span class="font-mono text-hint text-muted">
                {bytes(state.received)}
                {state.total === null ? '' : ` of ${bytes(state.total)}`}
              </span>
            </div>
            <Spinner />
          </div>
          <Progress percent={state.total === null ? null : Math.round((state.received / state.total) * 100)} />
        </div>
      ) : state.kind === 'ready' ? (
        <div class="border border-accent/30 bg-accent-soft">
          <div class="flex items-start gap-3 px-4 py-3">
            <div class="flex min-w-0 flex-1 flex-col gap-1">
              <span class="text-row text-off-white">version {state.update.version} is ready</span>
              {state.update.date ? <span class="font-mono text-hint text-muted">{state.update.date}</span> : null}
            </div>
            <Button size="sm" variant="primary" icon={<Icon name="refresh" />} onClick={() => void applySelfUpdate()}>
              restart now
            </Button>
          </div>
          {state.update.body ? (
            <p class="m-0 border-t border-accent/20 px-4 py-3 text-hint leading-relaxed whitespace-pre-wrap text-muted">
              {state.update.body}
            </p>
          ) : null}
        </div>
      ) : state.kind === 'current' ? (
        <ListGroup>
          <ListRow
            icon={<Icon name="check" />}
            iconTint="ok"
            title="up to date"
            subtitle="checked automatically"
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
