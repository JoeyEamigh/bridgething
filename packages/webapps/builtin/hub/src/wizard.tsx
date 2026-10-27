import {
  type BluetoothPairingResult,
  type BridgethingClient,
  type ConnectedDevice,
  type LauncherGesture,
} from '@bridgething/client';
import { useEffect, useRef, useState } from 'react';

import frame from './carthing-frame.png';
import { GESTURES, practiceFor } from './gesture.ts';

const PHASE_KEY = 'onboarding_phase';

type Step = 'pair' | 'gesture-choice' | 'gesture';

export function Wizard({ client, onDone }: { client: BridgethingClient; onDone: () => void }) {
  const [step, setStep] = useState<Step | null>(null);
  const [gesture, setGesture] = useState<LauncherGesture>('longPress');

  useEffect(() => {
    let cancelled = false;
    client.bluetooth
      .list()
      .then(r => {
        if (cancelled) return;
        setStep(r.ok && Object.keys(r.response).length > 0 ? 'gesture-choice' : 'pair');
      })
      .catch(() => {
        if (!cancelled) setStep('pair');
      });
    return () => {
      cancelled = true;
    };
  }, [client]);

  const finish = async () => {
    await client.store.put({ key: PHASE_KEY, value: 'done' });
    onDone();
  };

  if (step === null) {
    return <div className="h-full w-full bg-bg" />;
  }

  return (
    <div className="flex h-full w-full flex-col bg-bg">
      <StepIndicator step={step} />
      <div className="flex-1 overflow-hidden">
        {step === 'pair' && <PairStep client={client} onNext={() => setStep('gesture-choice')} />}
        {step === 'gesture-choice' && (
          <GestureChoiceStep
            client={client}
            onNext={g => {
              setGesture(g);
              setStep('gesture');
            }}
          />
        )}
        {step === 'gesture' && <GestureStep gesture={gesture} onNext={finish} />}
      </div>
    </div>
  );
}

function StepIndicator({ step }: { step: Step }) {
  const order = ['pair', 'gesture'] as const;
  const mapped = step === 'gesture-choice' ? 'gesture' : step;
  const idx = order.indexOf(mapped);
  return (
    <div className="flex items-center justify-center gap-2 pt-4">
      {order.map((s, i) => (
        <div key={s} className={`h-0.5 w-10 transition ${i <= idx ? 'bg-accent' : 'bg-rule-strong'}`} />
      ))}
    </div>
  );
}

function PairStep({ client, onNext }: { client: BridgethingClient; onNext: () => void }) {
  const [alias, setAlias] = useState<string | null>(null);
  const [connected, setConnected] = useState<ConnectedDevice | null>(null);
  const [pairResult, setPairResult] = useState<BluetoothPairingResult | null>(null);

  useEffect(() => {
    let cancelled = false;
    client.system.versionRequest().then(r => {
      if (cancelled || !r.ok) return;
      const tail = r.response.serialNumber.slice(-4);
      setAlias(tail ? `Car Thing (SN: ${tail})` : 'Car Thing');
    });
    return () => {
      cancelled = true;
    };
  }, [client]);

  useEffect(() => {
    const offConnected = client.bluetooth.onConnectedDevice(setConnected);
    const offResult = client.bluetooth.onPairingResult(setPairResult);
    return () => {
      offConnected();
      offResult();
    };
  }, [client]);

  const ready = pairResult?.success === true || connected !== null;

  return (
    <div className="flex h-full flex-col items-center justify-between px-8 pt-6 pb-8">
      <div className="flex flex-1 flex-col items-center justify-center gap-6">
        <div className="font-display text-hero font-medium tracking-display text-off-white">pair your phone</div>
        <div className="max-w-104 text-center text-body text-soft">
          {ready ? 'paired. you can move on.' : 'open bluetooth on your phone and look for this device.'}
        </div>

        {!ready && alias && (
          <div className="border border-rule-strong bg-screen px-6 py-4">
            <div className="font-mono text-row-lg text-off-white">{alias}</div>
          </div>
        )}

        {ready && connected && (
          <div className="flex flex-col items-center gap-1">
            <div className="font-mono text-eyebrow tracking-[0.25em] text-dim uppercase">connected</div>
            <div className="text-row-lg text-off-white">{connected.name}</div>
          </div>
        )}
      </div>

      <div className="flex w-full max-w-104 items-center justify-between">
        <button
          type="button"
          onClick={onNext}
          className="px-2 py-2 font-mono text-hint text-dim underline-offset-4 active:underline">
          skip for now
        </button>
        <button
          type="button"
          onClick={onNext}
          disabled={!ready}
          className="border border-accent bg-accent px-10 py-2.5 font-mono text-row text-screen transition active:opacity-80 disabled:border-rule disabled:bg-transparent disabled:text-dim">
          next
        </button>
      </div>
    </div>
  );
}

function GestureChoiceStep({
  client,
  onNext,
}: {
  client: BridgethingClient;
  onNext: (gesture: LauncherGesture) => void;
}) {
  const [saving, setSaving] = useState(false);

  const choose = async (gesture: LauncherGesture) => {
    if (saving) return;
    setSaving(true);
    await client.system.launcherGestureSet({ gesture }).catch(() => {});
    const stored = await client.system.launcherGestureGet({ timeoutMs: 2000 }).catch(() => null);
    onNext(stored?.ok ? stored.response.gesture : gesture);
  };

  return (
    <div className="flex h-full flex-col items-center justify-center gap-6 px-8 pt-4 pb-8">
      <div className="font-display text-hero font-medium tracking-display text-off-white">jump back to apps</div>
      <div className="max-w-104 text-center text-body text-soft">
        pick how you get back here from any app. you can change this later in settings.
      </div>

      <div className="flex w-full max-w-104 flex-col gap-3">
        {GESTURES.map(option => (
          <button
            key={option.value}
            type="button"
            disabled={saving}
            onClick={() => choose(option.value)}
            className="flex items-center justify-between border border-accent bg-accent-soft px-5 py-4 text-left transition active:opacity-80 disabled:opacity-60">
            <span>
              <span className="block font-mono text-row text-off-white">{option.label}</span>
              <span className="block pt-1 text-body text-soft">{option.hint}.</span>
            </span>
          </button>
        ))}
      </div>
    </div>
  );
}

function GestureStep({ gesture, onNext }: { gesture: LauncherGesture; onNext: () => void }) {
  const practice = practiceFor(gesture);
  if (practice.kind === 'hold') {
    return <HoldPracticeStep key={gesture} holdMs={practice.holdMs} prompt={practice.prompt} onNext={onNext} />;
  }
  return (
    <PressPracticeStep
      key={gesture}
      count={practice.count}
      windowMs={practice.windowMs}
      prompt={practice.prompt}
      onNext={onNext}
    />
  );
}

function PressPracticeStep({
  count,
  windowMs,
  prompt,
  onNext,
}: {
  count: number;
  windowMs: number;
  prompt: string;
  onNext: () => void;
}) {
  const [presses, setPresses] = useState(0);
  const finish = useRef(onNext);
  finish.current = onNext;

  useEffect(() => {
    let hits: number[] = [];
    let timer: ReturnType<typeof setTimeout> | undefined;
    let fired = false;

    const prune = () => {
      const now = performance.now();
      hits = hits.filter(t => now - t < windowMs);
      setPresses(hits.length);
      clearTimeout(timer);
      if (hits.length > 0) timer = setTimeout(prune, windowMs - (now - hits[0]));
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (fired || e.repeat || e.code !== 'KeyM') return;
      hits.push(performance.now());
      prune();
      if (hits.length >= count) {
        fired = true;
        clearTimeout(timer);
        finish.current();
      }
    };

    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      clearTimeout(timer);
    };
  }, [count, windowMs]);

  return (
    <div className="flex h-full flex-col items-center justify-center gap-6 px-8 pt-4 pb-8">
      <div className="font-display text-hero font-medium tracking-display text-off-white">jump back to apps</div>
      <div className="max-w-104 text-center text-body text-soft">{prompt}</div>

      <GestureHint />

      <div className="flex items-center gap-2.5">
        {Array.from({ length: count }, (_, i) => (
          <div
            key={i}
            className={`size-2.5 border transition ${i < presses ? 'border-accent bg-accent' : 'border-rule-strong'}`}
          />
        ))}
      </div>
    </div>
  );
}

function HoldPracticeStep({ holdMs, prompt, onNext }: { holdMs: number; prompt: string; onNext: () => void }) {
  const [holding, setHolding] = useState(false);
  const finish = useRef(onNext);
  finish.current = onNext;

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let fired = false;

    const onKeyDown = (e: KeyboardEvent) => {
      if (fired || e.repeat || e.code !== 'KeyM') return;
      setHolding(true);
      timer = setTimeout(() => {
        fired = true;
        finish.current();
      }, holdMs);
    };

    const onKeyUp = (e: KeyboardEvent) => {
      if (e.code !== 'KeyM') return;
      clearTimeout(timer);
      setHolding(false);
    };

    document.addEventListener('keydown', onKeyDown);
    document.addEventListener('keyup', onKeyUp);
    return () => {
      document.removeEventListener('keydown', onKeyDown);
      document.removeEventListener('keyup', onKeyUp);
      clearTimeout(timer);
    };
  }, [holdMs]);

  return (
    <div className="flex h-full flex-col items-center justify-center gap-6 px-8 pt-4 pb-8">
      <div className="font-display text-hero font-medium tracking-display text-off-white">jump back to apps</div>
      <div className="max-w-104 text-center text-body text-soft">{prompt}</div>

      <GestureHint />

      <div className="h-2.5 w-64 border border-rule-strong">
        <div
          className="h-full bg-accent"
          style={{
            width: holding ? '100%' : '0%',
            transition: holding ? `width ${holdMs}ms linear` : 'none',
          }}
        />
      </div>
    </div>
  );
}

function GestureHint() {
  return (
    <div className="relative">
      <img src={frame} alt="car thing" className="h-52 w-auto" />
      <div className="absolute top-[15%] left-[86.4%] -translate-x-1/2 -translate-y-full">
        <svg
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeLinejoin="round"
          className="h-7 w-7 animate-bounce text-accent">
          <path d="M12 5v14" />
          <path d="m19 12-7 7-7-7" />
        </svg>
      </div>
    </div>
  );
}
