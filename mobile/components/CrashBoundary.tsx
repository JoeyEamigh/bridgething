import { Component, type ErrorInfo, type ReactNode } from 'react';
import { ScrollView, Share, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

import { Button } from './Button';
import {
  type CrashRecord,
  formatCrash,
  recordCrash,
  useCrashStore,
} from '../lib/crash';
import { TEXT } from '../lib/theme';

type Props = { children: ReactNode };
type State = { failed: boolean };

export class CrashBoundary extends Component<Props, State> {
  state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    recordCrash(error, 'boundary', false, info.componentStack);
  }

  render(): ReactNode {
    if (!this.state.failed) return this.props.children;
    return <CrashScreen onRetry={() => this.setState({ failed: false })} />;
  }
}

function CrashScreen({ onRetry }: { onRetry: () => void }) {
  const record = useCrashStore(s => s.last);
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
            {record ? formatCrash(record) : 'nothing was recorded'}
          </Text>
        </ScrollView>

        <View className="gap-2">
          <Button
            onPress={() => void shareCrash(record)}
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

async function shareCrash(record: CrashRecord | null): Promise<void> {
  if (!record) return;
  try {
    await Share.share({ message: formatCrash(record) });
  } catch {
    // the sheet being dismissed is not worth surfacing on a crash screen
  }
}
