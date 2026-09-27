import type { BridgethingProviderInfo } from '@bridgething/session-react-native';
import { Text, View } from 'react-native';

import { Icon } from '../Icon';
import { Note } from '../Note';
import { PendingAuth } from '../PendingAuth';
import { Press } from '../Press';
import { ServerLoginSheet } from './ServerLoginSheet';
import { SignOutSheet } from './SignOutSheet';
import { useAccounts } from './useAccounts';
import { BOX, TEXT } from '../../lib/theme';

export function ProviderTiles() {
  const accounts = useAccounts();

  const pick = (provider: BridgethingProviderInfo) => {
    if (accounts.busyId || !provider.available) return;
    if (provider.connected) {
      accounts.askSignOut(provider);
      return;
    }
    accounts.signIn(provider.id);
  };

  return (
    <View>
      <View className="gap-2.5">
        {accounts.providers.length === 0 ? (
          <View className="border border-rule bg-screen px-4 py-6">
            <Text
              className="text-center font-mono text-muted"
              style={TEXT.hint}
            >
              starting up…
            </Text>
          </View>
        ) : (
          accounts.providers.map(p => (
            <ProviderTile
              key={p.id}
              name={p.displayName}
              selected={p.connected}
              busy={accounts.busyId === p.id}
              authStatus={p.authState.kind}
              handshake={p.signIn === 'handshake'}
              disabled={
                !p.available ||
                (accounts.busyId !== null && accounts.busyId !== p.id)
              }
              comingSoon={!p.available}
              onPress={() => pick(p)}
            />
          ))
        )}
      </View>

      {accounts.failure ? (
        <Note tone="err" className="mt-4">
          {accounts.failure}
        </Note>
      ) : null}

      {accounts.awaitingAuth.map(p => (
        <View className="mt-4" key={p.id}>
          <PendingAuth
            state={p.authState}
            onCancel={
              p.authState.kind === 'pending'
                ? () => accounts.cancelAuth(p.id)
                : undefined
            }
            onRetry={
              p.authState.kind === 'failed'
                ? () => accounts.signIn(p.id)
                : undefined
            }
          />
        </View>
      ))}

      <SignOutSheet accounts={accounts} />
      <ServerLoginSheet accounts={accounts} />
    </View>
  );
}

function ProviderTile({
  name,
  selected,
  busy,
  authStatus,
  handshake,
  disabled,
  comingSoon,
  onPress,
}: {
  name: string;
  selected: boolean;
  busy: boolean;
  authStatus: 'idle' | 'pending' | 'authenticated' | 'failed';
  handshake: boolean;
  disabled: boolean;
  comingSoon: boolean;
  onPress: () => void;
}) {
  const initial = name.slice(0, 1).toUpperCase();
  const subtitle = comingSoon
    ? 'coming soon'
    : busy
      ? 'opening…'
      : !selected
        ? 'tap to sign in'
        : authStatus === 'failed'
          ? 'sign-in failed'
          : authStatus === 'pending'
            ? handshake
              ? 'finish in your browser'
              : 'signing in…'
            : 'signed in';
  const showCheck = selected && authStatus === 'authenticated';

  return (
    <Press onPress={onPress} disabled={disabled}>
      <View
        className={`flex-row items-center gap-3 border bg-screen px-4 py-3 ${
          selected ? 'border-accent' : 'border-rule'
        } ${disabled ? 'opacity-60' : ''}`}
      >
        <View
          className={`items-center justify-center ${
            selected ? 'bg-accent' : 'bg-neutral-soft'
          }`}
          style={{ width: BOX.md, height: BOX.md }}
        >
          <Text
            className={`font-mono ${selected ? 'text-screen' : 'text-soft'}`}
            style={TEXT.rowLg}
          >
            {initial}
          </Text>
        </View>
        <View className="flex-1">
          <Text className="font-sans text-fg" style={TEXT.row}>
            {name}
          </Text>
          <Text className="mt-0.5 font-mono text-muted" style={TEXT.hint}>
            {subtitle}
          </Text>
        </View>
        {showCheck ? <Icon name="Check" tone="ok" size={16} /> : null}
      </View>
    </Press>
  );
}
