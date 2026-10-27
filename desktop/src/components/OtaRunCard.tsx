import type { OtaRun, OtaRunProgress } from '@bridgething/companion-types';
import { Button, Pill } from '@bridgething/ui';
import type { VNode } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { useDesktop } from '../desktop.ts';
import { rate } from '../lib/format.ts';
import { isRunning, phaseWord, runTitle, runTone } from '../lib/ota.ts';
import { Progress } from './Progress.tsx';

const TICK_MS = 500;

function useRunProgress(run: OtaRun): OtaRunProgress | null {
  const session = useDesktop();
  const [progress, setProgress] = useState<OtaRunProgress | null>(null);
  const live = isRunning(run);

  useEffect(() => {
    let current = true;
    const read = () => {
      void session.otaRunProgress(run.deviceId).then(next => {
        if (current) setProgress(next);
      });
    };
    read();
    const timer = live ? setInterval(read, TICK_MS) : undefined;
    return () => {
      current = false;
      clearInterval(timer);
    };
  }, [session, run, live]);

  return progress;
}

function eta(seconds: number | null | undefined): string | null {
  if (seconds == null || seconds <= 0) return null;
  if (seconds < 60) return `${seconds}s left`;
  return `${Math.round(seconds / 60)}m left`;
}

export function OtaRunCard({ run, onDismiss }: { run: OtaRun; onDismiss?: () => void }): VNode {
  const tone = runTone(run);
  const live = isRunning(run);
  const progress = useRunProgress(run);
  const percent = run.outcome === 'succeeded' ? 100 : (progress?.percent ?? null);
  const detail = [progress?.stepLabel, live ? rate(run.ratePerSec) : null, live ? eta(progress?.etaSeconds) : null]
    .filter(Boolean)
    .join(' · ');

  return (
    <div class="border border-rule bg-screen">
      <div class="flex items-start gap-3 px-4 py-3">
        <div class="flex min-w-0 flex-1 flex-col gap-1">
          <div class="flex items-center gap-2">
            <span class="truncate text-row text-off-white">{runTitle(run)}</span>
            <Pill tone={tone} dot={live}>
              {phaseWord(run)}
            </Pill>
          </div>
          {detail ? <span class="truncate text-hint text-muted">{detail}</span> : null}
        </div>
        <div class="flex shrink-0 items-center gap-2">
          {live && percent !== null ? <span class="font-mono text-body tabular-nums text-soft">{percent}%</span> : null}
          {!live && onDismiss ? (
            <Button size="sm" variant="ghost" onClick={onDismiss}>
              dismiss
            </Button>
          ) : null}
        </div>
      </div>
      {live ? <Progress percent={percent} /> : null}
      {run.error ? <p class="border-t border-rule px-4 py-2 text-hint text-err">{run.error}</p> : null}
    </div>
  );
}
