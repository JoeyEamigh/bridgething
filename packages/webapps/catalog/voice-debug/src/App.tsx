import {
  BridgethingClient,
  type NluSlots,
  type VoiceActivity,
  type VoiceIntent,
  type VoiceState,
} from '@bridgething/client';
import { daemonUrl } from '@bridgething/webapp-shared/daemon';
import { useEffect, useMemo, useRef, useState } from 'react';

const HISTORY_MAX = 40;

type Turn = {
  key: string;
  at: number;
  activity: VoiceActivity;
  display?: VoiceIntent;
};

const NEUTRAL_TONE = 'border-rule bg-neutral-soft text-soft';

const PHASE_TONE: Record<string, string> = {
  idle: NEUTRAL_TONE,
  listening: 'border-accent/30 bg-accent-soft text-accent',
  thinking: 'border-experimental/30 bg-experimental-soft text-experimental',
  done: 'border-ok/30 bg-ok-soft text-ok',
  failed: 'border-err/40 bg-err-soft text-err',
};

const STAGE_TONE: Record<string, string> = {
  fastPath: 'border-ok/30 bg-ok-soft text-ok',
  model: 'border-accent/30 bg-accent-soft text-accent',
  rejectedNoIntent: NEUTRAL_TONE,
  rejectedClarify: 'border-experimental/30 bg-experimental-soft text-experimental',
  noModel: 'border-err/40 bg-err-soft text-err',
};

export default function App() {
  const client = useMemo(() => new BridgethingClient({ url: daemonUrl() }), []);
  const [state, setState] = useState<VoiceState>({ muted: false, capturing: false, phase: 'idle' });
  const [live, setLive] = useState<VoiceActivity | null>(null);
  const [turns, setTurns] = useState<Turn[]>([]);
  const seq = useRef(0);

  useEffect(() => {
    const push = (activity: VoiceActivity) => {
      setLive(activity.phase === 'done' || activity.phase === 'failed' ? null : activity);
      if (activity.phase !== 'done' && activity.phase !== 'failed') return;
      setTurns(prev => [{ key: `t${seq.current++}`, at: Date.now(), activity }, ...prev].slice(0, HISTORY_MAX));
    };

    const offActivity = client.voice.onActivity(push);
    const offState = client.voice.onStateChanged(setState);
    const offIntent = client.voice.onIntent(intent => {
      setTurns(prev => (prev.length === 0 ? prev : [{ ...prev[0], display: intent }, ...prev.slice(1)]));
    });

    void client.voice.stateGet().then(result => {
      if (result.ok) setState(result.response.state);
    });

    return () => {
      offActivity();
      offState();
      offIntent();
    };
  }, [client]);

  const current = live ?? turns[0]?.activity ?? null;

  return (
    <div className="flex h-full w-full flex-col bg-bg text-off-white">
      <header className="flex items-center justify-between gap-3 border-b border-rule px-4 py-2">
        <div className="flex items-center gap-2">
          <Chip tone={PHASE_TONE[state.phase] ?? PHASE_TONE.idle}>{state.phase}</Chip>
          {state.muted && <Chip tone="border-err/40 bg-err-soft text-err">mic muted</Chip>}
          {state.capturing && <Chip tone="border-accent/30 bg-accent-soft text-accent">capturing</Chip>}
        </div>
        <div className="flex gap-2">
          <button
            className="border border-edge px-3 py-1.5 font-mono text-hint text-near active:bg-neutral-soft"
            onPointerDown={() => void client.voice.pushToTalk()}
            onPointerUp={() => void client.voice.release()}
            onPointerCancel={() => void client.voice.cancel()}>
            hold to talk
          </button>
          <button
            className="border border-edge px-3 py-1.5 font-mono text-hint text-near active:bg-neutral-soft"
            onClick={() => void client.voice.cancel()}>
            cancel
          </button>
          <button
            className="border border-edge px-3 py-1.5 font-mono text-hint text-near active:bg-neutral-soft"
            onClick={() =>
              void (state.muted
                ? client.voice.unmuteMic({ preserve: false })
                : client.voice.muteMic({ preserve: false }))
            }>
            {state.muted ? 'unmute' : 'mute'}
          </button>
          <button
            className="border border-edge px-3 py-1.5 font-mono text-hint text-near active:bg-neutral-soft"
            onClick={() => setTurns([])}>
            clear
          </button>
        </div>
      </header>

      <section className="border-b border-rule bg-screen px-4 py-3">
        {current ? (
          <Detail activity={current} />
        ) : (
          <p className="m-0 font-mono text-body text-dim">waiting for a wake word</p>
        )}
      </section>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-2">
        {turns.length === 0 ? (
          <p className="py-6 text-center font-mono text-body text-dim">no turns yet</p>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {turns.map(turn => (
              <li key={turn.key} className="border border-rule bg-screen px-3 py-2">
                <div className="flex items-center gap-2 text-hint">
                  <span className="font-mono tabular-nums text-dim">{clock(turn.at)}</span>
                  <Chip tone={PHASE_TONE[turn.activity.phase] ?? PHASE_TONE.idle}>{turn.activity.phase}</Chip>
                  {turn.activity.stage && (
                    <Chip tone={STAGE_TONE[turn.activity.stage] ?? NEUTRAL_TONE}>{turn.activity.stage}</Chip>
                  )}
                  <span className="font-mono text-near">{turn.activity.intent ?? '-'}</span>
                  {turn.activity.asrConfidence != null && (
                    <span className="font-mono tabular-nums text-dim">
                      asr {turn.activity.asrConfidence.toFixed(2)}
                    </span>
                  )}
                  {turn.activity.target && <span className="font-mono text-dim">to {turn.activity.target}</span>}
                  {turn.display && (
                    <Chip tone="border-accent/30 bg-accent-soft text-accent">rendered {turn.display.intent}</Chip>
                  )}
                </div>
                {turn.activity.transcript && (
                  <p className="m-0 mt-1 truncate text-row text-near">&ldquo;{turn.activity.transcript}&rdquo;</p>
                )}
                {turn.activity.error && (
                  <p className="m-0 mt-1 font-mono text-hint text-err">
                    {turn.activity.error.code}: {turn.activity.error.msg}
                  </p>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function Detail({ activity }: { activity: VoiceActivity }) {
  const slots = slotPairs(activity.slots);
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2 text-hint">
        <Chip tone={PHASE_TONE[activity.phase] ?? PHASE_TONE.idle}>{activity.phase}</Chip>
        {activity.reason && <Chip tone={NEUTRAL_TONE}>{activity.reason}</Chip>}
        {activity.score != null && <Chip tone={NEUTRAL_TONE}>score {activity.score.toFixed(3)}</Chip>}
        {activity.asrConfidence != null && <Chip tone={NEUTRAL_TONE}>asr {activity.asrConfidence.toFixed(3)}</Chip>}
        {activity.stage && <Chip tone={STAGE_TONE[activity.stage] ?? NEUTRAL_TONE}>{activity.stage}</Chip>}
        {activity.target && <Chip tone={NEUTRAL_TONE}>{activity.target}</Chip>}
      </div>
      <p className="m-0 text-title leading-tight">
        {activity.transcript ? (
          <span>&ldquo;{activity.transcript}&rdquo;</span>
        ) : (
          <span className="text-dim">{activity.phase === 'listening' ? 'listening...' : 'no transcript'}</span>
        )}
      </p>
      <div className="flex items-center gap-2 text-row">
        <span className="font-mono text-near">{activity.intent ?? '-'}</span>
        {slots.length > 0 && (
          <span className="flex flex-wrap gap-1">
            {slots.map(([key, value]) => (
              <span
                key={key}
                className="border border-rule bg-neutral-soft px-1.5 py-0.5 font-mono text-hint text-soft">
                {key}={value}
              </span>
            ))}
          </span>
        )}
      </div>
      {activity.error && (
        <p className="m-0 font-mono text-hint text-err">
          {activity.error.code}: {activity.error.msg}
        </p>
      )}
    </div>
  );
}

function Chip({ tone, children }: { tone: string; children: React.ReactNode }) {
  return <span className={`border px-1.5 py-0.5 font-mono text-eyebrow tracking-[0.06em] ${tone}`}>{children}</span>;
}

function slotPairs(slots: NluSlots | undefined): [string, string][] {
  if (!slots) return [];
  return Object.entries(slots)
    .filter(([, value]) => value !== null && value !== undefined)
    .map(([key, value]) => [key, String(value)]);
}

function clock(at: number): string {
  return new Date(at).toLocaleTimeString('en-US', { hour12: false });
}
