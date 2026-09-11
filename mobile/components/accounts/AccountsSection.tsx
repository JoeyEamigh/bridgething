import { Text, View } from 'react-native';

import { ListGroup } from '../ListGroup';
import { ListRow } from '../ListRow';
import { Note } from '../Note';
import { PendingAuth } from '../PendingAuth';
import { Pill } from '../Pill';
import { SectionHeader } from '../SectionHeader';
import { ServerLoginSheet } from './ServerLoginSheet';
import { SignOutSheet } from './SignOutSheet';
import { useAccounts } from './useAccounts';
import { TEXT } from '../../lib/theme';

export function AccountsSection() {
  const accounts = useAccounts();

  return (
    <View className="mb-8">
      <SectionHeader title="accounts" />
      <ListGroup>
        {accounts.offered.map(p =>
          p.connected ? (
            <ListRow
              key={p.id}
              icon="UserRound"
              iconTint="accent"
              title={p.displayName}
              subtitle={
                accounts.libraryProvider === p.id
                  ? 'signed in · browsing'
                  : 'signed in'
              }
              trailing={
                accounts.priority[0] === p.id ? (
                  <Pill tone="ok">preferred</Pill>
                ) : null
              }
              onPress={() => accounts.promote(p.id)}
            />
          ) : (
            <ListRow
              key={p.id}
              icon="LogIn"
              iconTint="accent"
              title={`sign in to ${p.displayName}`}
              chevron
              onPress={() => accounts.signIn(p.id)}
              loading={accounts.busyId === p.id}
            />
          ),
        )}
        {accounts.connected.map(p => (
          <ListRow
            key={`out-${p.id}`}
            icon="LogOut"
            iconTint="err"
            title={`sign out of ${p.displayName}`}
            destructive
            onPress={() => accounts.askSignOut(p)}
          />
        ))}
      </ListGroup>

      {accounts.providers.length > 1 ? (
        <Text className="mt-2 px-1 font-sans text-muted" style={TEXT.hint}>
          tap an account to prefer it for browsing and when nothing is playing.
        </Text>
      ) : null}

      {accounts.failure ? (
        <Note tone="err" className="mt-3">
          {accounts.failure}
        </Note>
      ) : null}

      {accounts.awaitingAuth.map(p => (
        <View className="mt-3" key={`auth-${p.id}`}>
          <PendingAuth
            state={p.authState}
            onCancel={
              p.authState.kind === 'pending'
                ? () => accounts.cancelAuth(p.id)
                : undefined
            }
            onRetry={
              p.authState.kind === 'failed' && accounts.busyId === null
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
