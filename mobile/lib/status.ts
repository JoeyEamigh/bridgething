import type { SourceFailure } from '@bridgething/catalog';
import type {
  BridgethingOtaRun,
  BridgethingProviderInfo,
  BridgethingSessionPeer,
  BridgethingVoiceModelState,
} from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useNavigation } from '@react-navigation/native';
import { useMemo, useState } from 'react';

import { useCatalog } from './catalog';
import { useOta } from './ota';
import { useReachable } from './reachability';
import {
  describePairOutcome,
  type PairNotice,
  runPairFlow,
  useSession,
} from './session';
import type { Tone } from './theme';
import type { RootNavigation } from '../navigation';

export type ConditionId =
  | 'notSignedIn'
  | 'noDevice'
  | 'linkFailed'
  | 'offline'
  | 'updateFailed'
  | 'voiceModelFailed'
  | 'storeUnavailable'
  | 'serviceDegraded';

export type ConditionAction = {
  kind: 'pair' | 'openApps' | 'openSettings' | 'openSources';
  label: string;
};

export type Condition = {
  id: ConditionId;
  tone: Tone;
  label: string;
  detail: string;
  action?: ConditionAction;
};

export type ConditionInput = {
  reachable: boolean;
  providers: BridgethingProviderInfo[];
  peers: BridgethingSessionPeer[];
  knownDeviceCount: number;
  voiceModel: BridgethingVoiceModelState;
  otaRuns: BridgethingOtaRun[];
  catalogSources: string[];
  catalogFailures: SourceFailure[];
};

export function conditions(input: ConditionInput): Condition[] {
  const found: Condition[] = [];

  if (!input.providers.some(p => p.connected)) {
    found.push({
      id: 'notSignedIn',
      tone: 'warn',
      label: 'not signed in',
      detail: 'connect a music account so your car thing has something to play',
      action: { kind: 'openSettings', label: 'sign in' },
    });
  }

  if (input.peers.length === 0 && input.knownDeviceCount === 0) {
    found.push({
      id: 'noDevice',
      tone: 'warn',
      label: 'no car thing',
      detail: 'pair a car thing to start using it',
      action: { kind: 'pair', label: 'pair' },
    });
  }

  const broken = input.peers.find(p => p.status === 'linkFailed');
  if (broken) {
    found.push({
      id: 'linkFailed',
      tone: 'err',
      label: 'link failed',
      detail: `${broken.name} is attached but the link did not open${
        broken.linkError ? ` · ${describeError(broken.linkError)}` : ''
      }`,
      action: { kind: 'openApps', label: 'reconnect' },
    });
  }

  if (!input.reachable) {
    found.push({
      id: 'offline',
      tone: 'warn',
      label: 'offline',
      detail:
        'the store and updates stay unavailable until this phone is back online',
    });
  }

  const failedRun = input.otaRuns.find(
    r => r.lane === 'firmware' && r.outcome === 'failed',
  );
  if (failedRun) {
    found.push({
      id: 'updateFailed',
      tone: 'err',
      label: 'update failed',
      detail: `an update did not finish${
        failedRun.error ? ` · ${describeError(failedRun.error)}` : ''
      }`,
      action: { kind: 'openApps', label: 'updates' },
    });
  }

  if (input.voiceModel.status === 'failed') {
    found.push({
      id: 'voiceModelFailed',
      tone: 'warn',
      label: 'voice model failed',
      detail: `the voice model did not download${
        input.voiceModel.error
          ? ` · ${describeError(input.voiceModel.error)}`
          : ''
      }`,
      action: { kind: 'openSettings', label: 'voice' },
    });
  }

  const badSources = input.reachable
    ? input.catalogFailures.filter(f => input.catalogSources.includes(f.url))
    : [];
  if (badSources.length > 0) {
    const all = badSources.length === input.catalogSources.length;
    found.push({
      id: 'storeUnavailable',
      tone: 'warn',
      label: all ? 'store unavailable' : 'a source is unavailable',
      detail: all
        ? `no app source could be read · ${badSources[0].reason}`
        : `${badSources.length} of ${input.catalogSources.length} app sources could not be read`,
      action: { kind: 'openSources', label: 'sources' },
    });
  }

  const degraded = input.reachable
    ? input.providers.find(p => p.connected && p.serviceHealth.kind !== 'ok')
    : undefined;
  if (degraded) {
    found.push({
      id: 'serviceDegraded',
      tone: 'warn',
      label:
        degraded.serviceHealth.kind === 'rateLimited'
          ? `${degraded.displayName} is rate limiting`
          : `${degraded.displayName} is unreachable`,
      detail:
        degraded.serviceHealth.kind === 'rateLimited'
          ? `${degraded.displayName} is throttling requests${
              degraded.serviceHealth.retryAfterSeconds
                ? ` · retrying in about ${Math.ceil(degraded.serviceHealth.retryAfterSeconds)}s`
                : ' · retrying shortly'
            }`
          : `${degraded.displayName} cannot be reached right now · retrying`,
    });
  }

  return found;
}

export function useConditions(): Condition[] {
  const providers = useSession(s => s.providers);
  const peers = useSession(s => s.peers);
  const ledger = useSession(s => s.ledger);
  const voiceModel = useSession(s => s.voiceModel);
  const runs = useOta(s => s.runs);
  const reachable = useReachable();
  const catalogSources = useCatalog(s => s.sources);
  const catalogFailures = useCatalog(s => s.failures);

  return useMemo(
    () =>
      conditions({
        reachable,
        providers,
        peers,
        knownDeviceCount: Object.keys(ledger).length,
        voiceModel,
        otaRuns: Object.values(runs),
        catalogSources,
        catalogFailures,
      }),
    [
      reachable,
      providers,
      peers,
      ledger,
      voiceModel,
      runs,
      catalogSources,
      catalogFailures,
    ],
  );
}

export function useConditionAction(): {
  run: (action: ConditionAction) => void;
  busy: boolean;
  notice: PairNotice | null;
} {
  const navigation = useNavigation<RootNavigation>();
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<PairNotice | null>(null);

  const run = (action: ConditionAction) => {
    switch (action.kind) {
      case 'openApps':
        navigation.navigate('Tabs', {
          screen: 'apps',
          params: { screen: 'Apps' },
        });
        return;
      case 'openSettings':
        navigation.navigate('Tabs', {
          screen: 'settings',
          params: { screen: 'Settings' },
        });
        return;
      case 'openSources':
        navigation.navigate('Tabs', {
          screen: 'store',
          params: { screen: 'StoreSources', params: { deviceId: null } },
        });
        return;
      case 'pair':
        if (busy) return;
        setBusy(true);
        setNotice(null);
        void runPairFlow()
          .then(outcome => setNotice(describePairOutcome(outcome)))
          .finally(() => setBusy(false));
        return;
    }
  };

  return { run, busy, notice };
}
