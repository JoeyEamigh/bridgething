import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { ScrollView, Text, useWindowDimensions, View } from 'react-native';
import Animated, {
  Easing,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
} from 'react-native-reanimated';
import { SafeAreaView } from 'react-native-safe-area-context';

import { ProviderTiles } from '../components/accounts/ProviderTiles';
import { Button } from '../components/Button';
import { Caret } from '../components/Caret';
import { ConditionList } from '../components/ConditionList';
import { Icon } from '../components/Icon';
import { IconBadge } from '../components/IconBadge';
import { Note } from '../components/Note';
import { PagerDots } from '../components/PagerDots';
import { PairNote } from '../components/PairNote';
import { Press } from '../components/Press';
import { ScreenHeader } from '../components/ScreenHeader';
import {
  locationStatus,
  openAppSettings,
  type PermissionState,
  requestLocation,
} from '../lib/permissions';
import {
  describePairOutcome,
  type PairNotice,
  runPairFlow,
  useSession,
  type VoiceIntroState,
  voiceIntroState,
} from '../lib/session';
import {
  pendingSummary,
  type SetupStep,
  setupSteps,
  stepForCondition,
} from '../lib/setup';
import { useConditions } from '../lib/status';
import {
  setSetupCompleted,
  setVoiceIntroOutcome,
  type VoiceIntroOutcome,
} from '../lib/storage';
import { TEXT } from '../lib/theme';
import type { RootScreenProps } from '../navigation';

type Props = RootScreenProps<'Setup'>;

const AUTO_ADVANCE_MS = 350;

export function SetupScreen({ navigation, route }: Props) {
  const { width } = useWindowDimensions();

  const initialStep = route.params?.step ?? 0;
  const [step, setStep] = useState(initialStep);

  useEffect(() => {
    navigation.setParams({ step });
  }, [step, navigation]);

  const [pairBusy, setPairBusy] = useState(false);
  const [pairNotice, setPairNotice] = useState<PairNotice | null>(null);

  const providers = useSession(s => s.providers);
  const peers = useSession(s => s.peers);
  const lastVoiceTurn = useSession(s => s.lastVoiceTurn);

  const paired = peers.some(p => p.status === 'connected');
  const signedIn = providers.some(p => p.connected);
  const advancing = signedIn && step === 0;

  useEffect(() => {
    if (!advancing) return undefined;
    const t = setTimeout(() => setStep(1), AUTO_ADVANCE_MS);
    return () => clearTimeout(t);
  }, [advancing]);

  const offset = useSharedValue(0);

  const pagesStyle = useAnimatedStyle(() => ({
    transform: [{ translateX: offset.value }],
  }));

  const pair = async () => {
    if (pairBusy) return;
    setPairBusy(true);
    setPairNotice(null);
    try {
      setPairNotice(describePairOutcome(await runPairFlow()));
    } finally {
      setPairBusy(false);
    }
  };

  const finish = () => {
    setSetupCompleted(true);
    navigation.replace('Tabs');
  };

  const advance = () => setStep(s => s + 1);

  const steps = useMemo(() => setupSteps(paired), [paired]);
  const pages = steps.length + 1;

  const page = (step: SetupStep) => {
    switch (step) {
      case 'signIn':
        return (
          <SignInPage
            onContinue={advance}
            signedIn={signedIn}
            advancing={advancing}
          />
        );
      case 'pair':
        return (
          <PairPage
            paired={paired}
            busy={pairBusy}
            notice={pairNotice}
            onPair={pair}
            onContinue={advance}
          />
        );
      case 'voice':
        return (
          <VoicePage
            state={voiceIntroState(lastVoiceTurn)}
            transcript={lastVoiceTurn?.transcript ?? null}
            onSettled={outcome => {
              setVoiceIntroOutcome(outcome);
              advance();
            }}
          />
        );
      case 'permissions':
        return <PermissionsPage onContinue={advance} />;
    }
  };

  useEffect(() => {
    if (step > pages - 1) setStep(pages - 1);
  }, [step, pages]);

  const at = Math.min(step, pages - 1);

  useEffect(() => {
    offset.value = withTiming(-at * width, {
      duration: 360,
      easing: Easing.out(Easing.cubic),
    });
  }, [at, width, offset]);

  return (
    <SafeAreaView edges={['top', 'bottom']} className="flex-1 bg-bg">
      <View className="flex-row items-center justify-between px-4 py-3">
        {at > 0 ? (
          <Press
            onPress={() => setStep(s => Math.max(0, s - 1))}
            className="-ml-1 h-9 w-9 items-center justify-center"
          >
            <Icon name="ChevronLeft" size={22} />
          </Press>
        ) : (
          <View className="h-9 w-9" />
        )}
        <PagerDots count={pages} index={at} />
        <View className="h-9 w-9" />
      </View>

      <View className="flex-1 overflow-hidden">
        <Animated.View
          className="flex-1 flex-row"
          style={[{ width: width * pages }, pagesStyle]}
        >
          {steps.map(step => (
            <Page width={width} key={step}>
              {page(step)}
            </Page>
          ))}
          <Page width={width} key="finish">
            <FinishPage steps={steps} onGoBack={setStep} onFinish={finish} />
          </Page>
        </Animated.View>
      </View>
    </SafeAreaView>
  );
}

function Page({ children, width }: { children: ReactNode; width: number }) {
  return <View style={{ width }}>{children}</View>;
}

function PageBody({ children }: { children: ReactNode }) {
  return (
    <ScrollView
      contentContainerClassName="grow justify-between px-4 pb-8 pt-4"
      showsVerticalScrollIndicator={false}
    >
      {children}
    </ScrollView>
  );
}

function StepHeader({ title, subtitle }: { title: string; subtitle?: string }) {
  return <ScreenHeader title={title} subtitle={subtitle} />;
}

function SignInPage({
  onContinue,
  signedIn,
  advancing,
}: {
  onContinue: () => void;
  signedIn: boolean;
  advancing: boolean;
}) {
  return (
    <PageBody>
      <View>
        <StepHeader title="sign in to your music" />
        <ProviderTiles />
      </View>

      <View className="mt-8">
        {advancing ? (
          <Note tone="ok">signed in · continuing</Note>
        ) : (
          <Button
            onPress={onContinue}
            icon="ArrowRight"
            size="lg"
            variant={signedIn ? 'primary' : 'secondary'}
          >
            {signedIn ? 'continue' : 'skip for now'}
          </Button>
        )}
      </View>
    </PageBody>
  );
}

function PairPage({
  paired,
  busy,
  notice,
  onPair,
  onContinue,
}: {
  paired: boolean;
  busy: boolean;
  notice: PairNotice | null;
  onPair: () => void;
  onContinue: () => void;
}) {
  const pairing = busy && !paired;
  return (
    <PageBody>
      <View>
        <StepHeader
          title={pairing ? 'pairing your car thing' : 'pair your car thing'}
          subtitle={
            pairing
              ? 'when the bluetooth pairing prompt appears, tap pair to continue.'
              : 'turn on your car thing and tap pair. it can take a few seconds for your car thing to appear.'
          }
        />

        <View className="my-10 items-center">
          {paired ? (
            <IconBadge name="Check" tone="ok" size="lg" />
          ) : (
            <View className="flex-row items-center gap-3">
              <IconBadge name="BellRing" tone="accent" size="lg" />
              <Caret />
            </View>
          )}
        </View>
      </View>

      <View className="gap-2.5">
        <PairNote notice={notice} />
        {paired ? (
          <Button onPress={onContinue} icon="ArrowRight" size="lg">
            continue
          </Button>
        ) : (
          <>
            <Button onPress={onPair} loading={busy} size="lg" icon="BellRing">
              pair
            </Button>
            <Button onPress={onContinue} variant="ghost" size="md">
              skip
            </Button>
          </>
        )}
      </View>
    </PageBody>
  );
}

function VoicePage({
  state,
  transcript,
  onSettled,
}: {
  state: VoiceIntroState;
  transcript: string | null;
  onSettled: (outcome: VoiceIntroOutcome) => void;
}) {
  const heard = state === 'heard';
  const model = useSession(s => s.voiceModel);

  const settle = useRef(onSettled);
  settle.current = onSettled;

  useEffect(() => {
    if (!heard) return undefined;
    const t = setTimeout(() => settle.current('heard'), 1200);
    return () => clearTimeout(t);
  }, [heard]);

  const blurb =
    state === 'listening'
      ? 'listening · ask for something, like "next song".'
      : state === 'heard'
        ? 'that went to your car thing through this phone. voice works.'
        : state === 'missed'
          ? 'that one did not land. try again.'
          : 'say it out loud to your car thing, then ask for something · like "next song".';

  return (
    <PageBody>
      <View>
        <StepHeader title="say hey bridgething" subtitle={blurb} />

        <View className="my-10 items-center">
          {heard ? (
            <IconBadge name="Check" tone="ok" size="lg" />
          ) : (
            <View className="flex-row items-center gap-3">
              <IconBadge name="Mic" tone="accent" size="lg" />
              <Caret />
            </View>
          )}
        </View>

        {heard && transcript ? (
          <Text
            className="text-center font-mono text-near"
            style={TEXT.row}
            numberOfLines={2}
          >
            {transcript}
          </Text>
        ) : null}

        {!heard && model.status === 'downloading' ? (
          <Text className="text-center font-sans text-muted" style={TEXT.hint}>
            the full understanding model is still downloading · built-in phrases
            like &ldquo;next song&rdquo; still work.
          </Text>
        ) : null}
      </View>

      <View className="gap-2.5">
        <Button
          onPress={() => onSettled('skipped')}
          variant={heard ? 'ghost' : 'secondary'}
          size={heard ? 'md' : 'lg'}
        >
          skip
        </Button>
      </View>
    </PageBody>
  );
}

function PermissionsPage({ onContinue }: { onContinue: () => void }) {
  const [status, setStatus] = useState<PermissionState>('denied');

  useEffect(() => {
    let cancelled = false;
    locationStatus().then(s => {
      if (!cancelled) setStatus(s);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const grant = async () => {
    if (status === 'blocked') {
      openAppSettings();
      return;
    }
    setStatus(await requestLocation());
  };

  const granted = status === 'granted';
  return (
    <PageBody>
      <View>
        <StepHeader
          title="share your location with your car thing"
          subtitle="some car thing apps (weather, maps) work better with your location."
        />

        <View className="my-10 items-center">
          <IconBadge
            name={granted ? 'Check' : 'MapPin'}
            tone={granted ? 'ok' : 'accent'}
            size="lg"
          />
        </View>
      </View>

      <View className="gap-2.5">
        {granted ? (
          <Button onPress={onContinue} icon="ArrowRight" size="lg">
            continue
          </Button>
        ) : (
          <>
            <Button onPress={grant} size="lg" icon="MapPin">
              {status === 'blocked' ? 'open settings' : 'allow location'}
            </Button>
            <Button onPress={onContinue} variant="ghost" size="md">
              not now
            </Button>
          </>
        )}
      </View>
    </PageBody>
  );
}

function FinishPage({
  steps,
  onGoBack,
  onFinish,
}: {
  steps: SetupStep[];
  onGoBack: (step: number) => void;
  onFinish: () => void;
}) {
  const pending = useConditions();

  return (
    <PageBody>
      <View>
        <ScreenHeader title={pendingSummary(pending.length)} />
        <ConditionList
          conditions={pending}
          action={condition => {
            const back = stepForCondition(steps, condition.id);
            if (back === null || !condition.action) return null;
            return (
              <View className="mt-1 flex-row">
                <Button
                  variant="secondary"
                  size="sm"
                  full={false}
                  onPress={() => onGoBack(back)}
                >
                  {condition.action.label}
                </Button>
              </View>
            );
          }}
        />
      </View>

      <View className="mt-8">
        <Button onPress={onFinish} icon="ArrowRight" size="lg">
          open bridgething
        </Button>
      </View>
    </PageBody>
  );
}
