import { Button, ScreenHeader, StatusStrip, Wordmark } from '@bridgething/ui';
import type { VNode } from 'preact';

import { ConnectFlow } from '../components/ConnectFlow.tsx';
import { Screen } from '../components/Screen.tsx';
import { endpoints, peers } from '../stores/session.ts';

export function OnboardingScreen({ onSkip }: { onSkip: () => void }): VNode {
  const found = endpoints.data.value;
  const failed = peers.value.filter(peer => peer.status === 'linkFailed');

  return (
    <div class="flex h-full min-w-0 flex-col">
      <header class="flex shrink-0 items-center justify-between gap-3 border-b border-rule bg-screen px-6 py-4">
        <Wordmark size="sm" />
        <Button variant="ghost" size="sm" onClick={onSkip}>
          skip
        </Button>
      </header>

      <Screen>
        <ScreenHeader title="connect a Car Thing" />

        {failed.length > 0 ? (
          <StatusStrip tone="err" title="link failed" subtitle={failed[0]?.linkError ?? undefined} class="mb-6" />
        ) : found.length === 0 ? (
          <StatusStrip tone="warn" title="searching" subtitle="plug a Car Thing in over usb" class="mb-6" />
        ) : null}

        <ConnectFlow />
      </Screen>
    </div>
  );
}
