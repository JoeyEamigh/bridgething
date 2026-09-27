import type { BridgethingProviderInfo } from '@bridgething/session-react-native';
import { describeError } from '@bridgething/ui/errors';
import { useState } from 'react';

import { getSession, useSession } from '../../lib/session';

export type ServerLogin = {
  serverUrl: string;
  username: string;
  password: string;
};

export type Accounts = {
  providers: BridgethingProviderInfo[];
  offered: BridgethingProviderInfo[];
  connected: BridgethingProviderInfo[];
  awaitingAuth: BridgethingProviderInfo[];
  priority: string[];
  libraryProvider: string | null;
  signedIn: boolean;
  busyId: string | null;
  leaving: BridgethingProviderInfo | null;
  leaveBusy: boolean;
  login: BridgethingProviderInfo | null;
  loginBusy: boolean;
  loginFailure: string | null;
  failure: string | null;
  signIn: (id: string) => void;
  cancelAuth: (id: string) => void;
  askSignOut: (provider: BridgethingProviderInfo) => void;
  dismissSignOut: () => void;
  confirmSignOut: () => void;
  dismissLogin: () => void;
  submitLogin: (login: ServerLogin) => Promise<void>;
  promote: (id: string) => void;
};

export function useAccounts(): Accounts {
  const session = getSession();

  const providers = useSession(s => s.providers);
  const priority = useSession(s => s.providerPriority);
  const libraryProvider = useSession(s => s.libraryProvider);

  const [busyId, setBusyId] = useState<string | null>(null);
  const [leaving, setLeaving] = useState<BridgethingProviderInfo | null>(null);
  const [leaveBusy, setLeaveBusy] = useState(false);
  const [login, setLogin] = useState<BridgethingProviderInfo | null>(null);
  const [loginBusy, setLoginBusy] = useState(false);
  const [loginFailure, setLoginFailure] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const signIn = (id: string) => {
    if (busyId) return;
    setFailure(null);
    const provider = providers.find(p => p.id === id);
    if (provider?.signIn === 'serverLogin') {
      setLoginFailure(null);
      setLogin(provider);
      return;
    }
    setBusyId(id);
    void session
      .connectProvider(id)
      .catch(() => {})
      .finally(() => setBusyId(null));
  };

  const submitLogin = async (form: ServerLogin) => {
    const provider = login;
    if (!provider || loginBusy) return;
    setLoginBusy(true);
    setLoginFailure(null);
    try {
      await session.completeProviderAuth(provider.id, {
        kind: 'serverLogin',
        ...form,
      });
      setLogin(null);
    } catch (err: unknown) {
      setLoginFailure(describeError(err));
    } finally {
      setLoginBusy(false);
    }
  };

  const cancelAuth = (id: string) => {
    void session.cancelAuth(id);
    setBusyId(null);
  };

  const confirmSignOut = () => {
    const provider = leaving;
    if (!provider || leaveBusy) return;
    setLeaveBusy(true);
    setFailure(null);
    void session
      .disconnectProvider(provider.id)
      .then(() => setLeaving(null))
      .catch((err: unknown) => setFailure(describeError(err)))
      .finally(() => setLeaveBusy(false));
  };

  const promote = (id: string) => {
    const rest = providers.map(p => p.id).filter(x => x !== id);
    setFailure(null);
    void session
      .setProviderPriority([id, ...rest])
      .catch((err: unknown) => setFailure(describeError(err)));
  };

  return {
    providers,
    offered: providers.filter(p => p.available),
    connected: providers.filter(p => p.connected),
    awaitingAuth: providers.filter(
      p => p.authState.kind === 'pending' || p.authState.kind === 'failed',
    ),
    priority,
    libraryProvider,
    signedIn: providers.some(p => p.connected),
    busyId,
    leaving,
    leaveBusy,
    login,
    loginBusy,
    loginFailure,
    failure,
    signIn,
    cancelAuth,
    askSignOut: setLeaving,
    dismissSignOut: () => setLeaving(null),
    confirmSignOut,
    dismissLogin: () => setLogin(null),
    submitLogin,
    promote,
  };
}
