import { useEffect, useState } from 'react';
import { View } from 'react-native';

import { Button } from '../Button';
import { Field } from '../Field';
import { Note } from '../Note';
import { Sheet, SheetTitle } from '../Sheet';
import type { Accounts } from './useAccounts';

export function ServerLoginSheet({ accounts }: { accounts: Accounts }) {
  const provider = accounts.login;
  const visible = provider != null;
  const [serverUrl, setServerUrl] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');

  useEffect(() => {
    if (visible) setPassword('');
  }, [visible]);

  const ready =
    serverUrl.trim().length > 0 &&
    username.trim().length > 0 &&
    password.length > 0;

  const submit = () => {
    if (!ready || accounts.loginBusy) return;
    void accounts.submitLogin({ serverUrl, username, password });
  };

  return (
    <Sheet visible={visible} onClose={accounts.dismissLogin}>
      <SheetTitle>{`sign in to ${provider?.displayName ?? ''}`}</SheetTitle>
      <Field
        label="server"
        icon="Link"
        value={serverUrl}
        onChangeText={setServerUrl}
        placeholder="music.example.com"
        hint="https is assumed when the address has no scheme"
        autoCapitalize="none"
        autoCorrect={false}
        keyboardType="url"
        returnKeyType="next"
      />
      <Field
        label="username"
        icon="UserRound"
        value={username}
        onChangeText={setUsername}
        autoCapitalize="none"
        autoCorrect={false}
        returnKeyType="next"
      />
      <Field
        label="password"
        icon="Lock"
        value={password}
        onChangeText={setPassword}
        secureTextEntry
        autoCapitalize="none"
        autoCorrect={false}
        returnKeyType="done"
        onSubmitEditing={submit}
      />
      {accounts.loginFailure ? (
        <Note tone="err">{accounts.loginFailure}</Note>
      ) : null}
      <View className="flex-row justify-end gap-2">
        <Button
          variant="ghost"
          size="md"
          full={false}
          onPress={accounts.dismissLogin}
        >
          cancel
        </Button>
        <Button
          variant="primary"
          size="md"
          full={false}
          onPress={submit}
          disabled={!ready}
          loading={accounts.loginBusy}
        >
          sign in
        </Button>
      </View>
    </Sheet>
  );
}
