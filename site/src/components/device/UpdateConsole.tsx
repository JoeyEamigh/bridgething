import { useState } from 'preact/hooks';
import {
  applyUpdate,
  connectWired,
  resolveUpdate,
  watchProgress,
  type UpdatePlan,
  type WiredSession,
} from '../../lib/wired';
import { ConsoleLog } from '../console/ConsoleLog';
import { useConsoleLog } from '../console/useConsoleLog';
import { deviceName } from '@bridgething/client';

type Phase =
  | { step: 'idle' }
  | { step: 'connecting' }
  | { step: 'ready'; session: WiredSession; plan: UpdatePlan | null; summary: string }
  | { step: 'applying'; session: WiredSession; plan: UpdatePlan; summary: string; percent: number; note: string }
  | { step: 'done'; summary: string };

function mb(bytes: number): string {
  return `${(bytes / 1024 / 1024).toFixed(1)} mb`;
}

export function UpdateConsole() {
  const { lines, say } = useConsoleLog();
  const [host, setHost] = useState('bridgething.local');
  const [phase, setPhase] = useState<Phase>({ step: 'idle' });

  if (typeof WebSocket === 'undefined') {
    return (
      <div class="border border-dashed border-white/25 p-6">
        <p class="m-0 font-mono text-sm text-white/70">
          this browser cannot reach local network devices. use chrome 147 or newer, or run{' '}
          <code>bunx @bridgething/updater</code> instead.
        </p>
      </div>
    );
  }

  const connect = async () => {
    setPhase({ step: 'connecting' });
    say(`opening ${host}`);
    try {
      const session = await connectWired(host);
      say(`connected to ${session.deviceId}`, 'ok');

      const meta = session.meta;
      if (!meta) {
        say('device connected but never announced its version; cannot compare releases', 'warn');
        setPhase({
          step: 'ready',
          session,
          plan: null,
          summary: 'connected, but the device did not report a version.',
        });
        return;
      }

      say(`daemon ${meta.appVersion} · image ${meta.imageVariant}@${meta.imageVersion} · ${meta.channel}`);
      const plan = await resolveUpdate(meta, meta.channel);
      const head = `${deviceName(meta)}\ndaemon ${meta.appVersion} · image ${meta.imageVersion} · ${meta.channel}`;

      if (!plan) {
        say('already on the latest release for its channel', 'ok');
        setPhase({ step: 'ready', session, plan: null, summary: `${head}\n\nalready up to date.` });
        return;
      }

      say(`update available: ${plan.version}`, 'ok');
      setPhase({
        step: 'ready',
        session,
        plan,
        summary: `${head}\n\nupdate to ${plan.version}\ndaemon ${plan.to.daemon} · image ${plan.to.image}`,
      });
    } catch (err) {
      say(`connect failed: ${err instanceof Error ? err.message : String(err)}`, 'err');
      setPhase({ step: 'idle' });
    }
  };

  const apply = async () => {
    if (phase.step !== 'ready' || !phase.plan) return;
    const { session, plan, summary } = phase;
    setPhase({ step: 'applying', session, plan, summary, percent: 0, note: 'starting' });

    const watching = watchProgress(session.device, (percent, note) =>
      setPhase(prev => (prev.step === 'applying' ? { ...prev, percent, note } : prev)),
    );

    try {
      await applyUpdate(session.device, plan, {
        log: say,
        download: (received, total) =>
          setPhase(prev =>
            prev.step === 'applying'
              ? {
                  ...prev,
                  percent: total > 0 ? (received / total) * 100 : prev.percent,
                  note: `downloading ${mb(received)}${total > 0 ? ` / ${mb(total)}` : ''}`,
                }
              : prev,
          ),
      });
      setPhase({ step: 'done', summary: `${summary}\n\nupdate applied.` });
    } catch (err) {
      say(`update failed: ${err instanceof Error ? err.message : String(err)}`, 'err');
      setPhase({ step: 'ready', session, plan, summary });
    } finally {
      watching.stop();
    }
  };

  const connected = phase.step === 'ready' || phase.step === 'applying' || phase.step === 'done';
  const summary = connected && 'summary' in phase ? phase.summary : null;
  const percent = phase.step === 'applying' ? phase.percent : phase.step === 'done' ? 100 : 0;

  return (
    <div class="flex flex-col gap-8">
      <div class="flex flex-col gap-4">
        <p class="text-accent m-0 font-mono text-sm">1 - connect</p>
        <p class="m-0 max-w-[60ch] text-base text-pretty text-white/60">
          plug the thing into this computer over usb-c. the browser will ask for permission to reach devices on your
          local network.
        </p>
        <div class="flex flex-wrap items-end gap-4">
          <label class="flex flex-col gap-1.5">
            <span class="font-mono text-xs text-white/40">host</span>
            <input
              type="text"
              spellcheck={false}
              value={host}
              disabled={connected}
              onInput={e => setHost((e.target as HTMLInputElement).value)}
              class="focus:border-accent border border-white/20 bg-black px-3 py-2 font-mono text-sm text-white/80 focus:outline-none"
            />
          </label>
          <button
            type="button"
            class="btn btn-primary"
            disabled={phase.step === 'connecting' || connected}
            onClick={() => void connect()}>
            connect
          </button>
          <p class="m-0 font-mono text-sm text-white/50">
            {phase.step === 'connecting' ? 'connecting' : connected ? 'connected' : 'not connected'}
          </p>
        </div>
        <p class="m-0 font-mono text-xs text-white/35">
          more than one thing on this machine? they show up as <code>bridgething-&lt;serial&gt;.local</code>.
        </p>
      </div>

      {summary ? (
        <div class="flex flex-col gap-4">
          <p class="text-accent m-0 font-mono text-sm">2 - what is on it</p>
          <div class="border border-dashed border-white/25 p-6">
            <p class="m-0 font-mono text-sm whitespace-pre-wrap text-white/70">{summary}</p>
          </div>
          <div class="flex flex-wrap items-center gap-4">
            <button
              type="button"
              class="btn btn-primary"
              disabled={phase.step !== 'ready' || !phase.plan}
              onClick={() => void apply()}>
              {phase.step === 'applying' ? 'installing…' : 'install update'}
            </button>
            {phase.step === 'applying' ? <p class="m-0 font-mono text-sm text-white/50">{phase.note}</p> : null}
          </div>
          <div class="h-1.5 w-full bg-white/10">
            <div
              class="bg-accent h-full transition-[width] duration-200"
              style={{ width: `${Math.max(0, Math.min(100, percent)).toFixed(1)}%` }}
            />
          </div>
          <p class="m-0 font-mono text-xs text-white/35">leave it plugged in until it finishes.</p>
        </div>
      ) : null}

      <ConsoleLog title="/var/log/update" lines={lines} />
    </div>
  );
}
