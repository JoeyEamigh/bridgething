import type { BridgethingAuthState } from '@bridgething/session-react-native';
import { useEffect, useRef } from 'react';
import { Text, View } from 'react-native';

import { Button } from './Button';
import { Spinner } from './Spinner';
import { closeSignIn, openSignIn } from '../lib/auth-browser';
import { TEXT, TYPE } from '../lib/theme';

const CODE_TRACKING = TYPE.title * 0.2;

export function PendingAuth({
  providerId,
  state,
  onCancel,
  onRetry,
}: {
  providerId: string;
  state: BridgethingAuthState;
  onCancel?: () => void;
  onRetry?: () => void;
}) {
  const pendingUrl =
    state.kind === 'pending'
      ? (state.verificationUrlComplete ?? state.verificationUrl ?? null)
      : null;

  const openedFor = useRef<string | null>(null);
  useEffect(() => {
    if (!pendingUrl) {
      openedFor.current = null;
      return;
    }
    const key =
      state.kind === 'pending' ? (state.userCode ?? pendingUrl) : pendingUrl;
    if (openedFor.current === key) return;
    openedFor.current = key;
    void openSignIn(providerId, pendingUrl);
  }, [providerId, pendingUrl, state]);

  useEffect(() => {
    if (state.kind !== 'pending') closeSignIn(providerId);
  }, [providerId, state.kind]);

  useEffect(() => () => closeSignIn(providerId), [providerId]);

  if (state.kind === 'pending') {
    return (
      <View className="border border-accent bg-accent-soft p-4">
        <View className="flex-row items-center gap-2">
          <Spinner tone="accent" />
          <Text
            className="font-mono uppercase text-accent"
            style={TEXT.eyebrow}
          >
            waiting on provider
          </Text>
        </View>
        {state.userCode ? (
          <View className="mt-3">
            <Text className="font-sans text-muted" style={TEXT.hint}>
              enter this code at spotify.com/pair
            </Text>
            <Text
              className="mt-1 font-mono text-fg"
              style={{ fontSize: TYPE.title, letterSpacing: CODE_TRACKING }}
              selectable
            >
              {state.userCode}
            </Text>
            <Text
              className="mt-1 font-mono uppercase text-dim"
              style={TEXT.eyebrow}
            >
              long press to copy
            </Text>
          </View>
        ) : null}
        {pendingUrl ? (
          <View className="mt-3 self-start">
            <Button
              variant="secondary"
              size="sm"
              full={false}
              onPress={() => void openSignIn(providerId, pendingUrl)}
            >
              open authorization
            </Button>
          </View>
        ) : null}
        {state.verificationUrl ? (
          <Text
            className="mt-2 font-mono text-dim"
            style={TEXT.hint}
            selectable
          >
            {state.verificationUrl}
          </Text>
        ) : null}
        {onCancel ? (
          <View className="mt-3 self-start">
            <Button variant="ghost" size="sm" full={false} onPress={onCancel}>
              cancel
            </Button>
          </View>
        ) : null}
      </View>
    );
  }
  if (state.kind === 'failed') {
    return (
      <View className="border border-err bg-err-soft p-4">
        <Text className="font-mono uppercase text-err" style={TEXT.eyebrow}>
          sign-in failed
        </Text>
        <Text className="mt-1 font-sans text-err" style={TEXT.body}>
          {state.message ?? 'unknown error'}
        </Text>
        {onRetry ? (
          <View className="mt-3 self-start">
            <Button
              variant="secondary"
              size="sm"
              full={false}
              onPress={onRetry}
            >
              try again
            </Button>
          </View>
        ) : null}
      </View>
    );
  }
  return null;
}
