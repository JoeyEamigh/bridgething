import { describeError } from '@bridgething/ui/errors';
import { Component, type ErrorInfo, type ReactNode } from 'react';
import { ScrollView, Share, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

import { Button } from './Button';
import { TEXT } from '../lib/theme';

type Props = { children: ReactNode };
type State = { details: string | null };

export class CrashBoundary extends Component<Props, State> {
  state: State = { details: null };

  static getDerivedStateFromError(error: unknown): State {
    return { details: describeCrash(error, null) };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    const details = describeCrash(error, info.componentStack ?? null);
    console.error(`[bridgething] render crash\n${details}`);
    this.setState({ details });
  }

  render(): ReactNode {
    if (this.state.details == null) return this.props.children;
    return (
      <CrashScreen
        details={this.state.details}
        onRetry={() => this.setState({ details: null })}
      />
    );
  }
}

function describeCrash(error: unknown, componentStack: string | null): string {
  const lines = [describeError(error)];
  if (error instanceof Error && error.stack) lines.push('', error.stack);
  if (componentStack) lines.push('', componentStack.trim());
  return lines.join('\n');
}

function CrashScreen({
  details,
  onRetry,
}: {
  details: string;
  onRetry: () => void;
}) {
  return (
    <SafeAreaView className="flex-1 bg-bg">
      <View className="flex-1 gap-4 p-5">
        <Text className="font-mono uppercase text-err" style={TEXT.eyebrow}>
          bridgething hit an error
        </Text>
        <Text className="font-sans text-fg" style={TEXT.body}>
          the details below are saved on this phone. sharing them is what makes
          the bug fixable.
        </Text>

        <ScrollView className="flex-1 border border-rule bg-screen p-3">
          <Text className="font-mono text-muted" style={TEXT.hint} selectable>
            {details}
          </Text>
        </ScrollView>

        <View className="gap-2">
          <Button
            onPress={() => void Share.share({ message: details }).catch(() => {})}
            size="md"
            icon="Share2"
          >
            share the details
          </Button>
          <Button onPress={onRetry} variant="secondary" size="md">
            try again
          </Button>
        </View>
      </View>
    </SafeAreaView>
  );
}

