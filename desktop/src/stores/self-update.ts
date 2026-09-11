import { describeError } from '@bridgething/ui';
import { signal } from '@preact/signals';
import { check, type Update } from '@tauri-apps/plugin-updater';

import { restart } from '../lib/lifecycle.ts';

export type SelfUpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current' }
  | { kind: 'downloading'; update: Update; received: number; total: number | null }
  | { kind: 'ready'; update: Update }
  | { kind: 'unavailable'; reason: string };

export const selfUpdate = signal<SelfUpdateState>({ kind: 'idle' });

const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

export async function checkSelfUpdate(): Promise<void> {
  const held = selfUpdate.value;
  if (held.kind === 'checking' || held.kind === 'downloading' || held.kind === 'ready') return;

  selfUpdate.value = { kind: 'checking' };
  let found: Update | null;
  try {
    found = await check();
  } catch (reason) {
    selfUpdate.value = { kind: 'unavailable', reason: describeError(reason) };
    return;
  }
  if (!found?.available) {
    selfUpdate.value = { kind: 'current' };
    return;
  }
  await pull(found);
}

async function pull(update: Update): Promise<void> {
  selfUpdate.value = { kind: 'downloading', update, received: 0, total: null };
  try {
    let received = 0;
    let total: number | null = null;
    await update.download(progress => {
      if (progress.event === 'Started') total = progress.data.contentLength ?? null;
      else if (progress.event === 'Progress') received += progress.data.chunkLength;
      selfUpdate.value = { kind: 'downloading', update, received, total };
    });
    selfUpdate.value = { kind: 'ready', update };
  } catch (reason) {
    selfUpdate.value = { kind: 'unavailable', reason: describeError(reason) };
  }
}

export async function applySelfUpdate(): Promise<void> {
  const held = selfUpdate.value;
  if (held.kind !== 'ready') return;
  try {
    await held.update.install();
    await restart();
  } catch (reason) {
    selfUpdate.value = { kind: 'unavailable', reason: describeError(reason) };
  }
}

export function watchSelfUpdates(): void {
  void checkSelfUpdate();
  setInterval(() => void checkSelfUpdate(), CHECK_EVERY_MS);
}
