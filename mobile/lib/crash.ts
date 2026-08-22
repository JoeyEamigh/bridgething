import { describeError } from '@bridgething/ui/errors';
import { create } from 'zustand';

import { storage } from './storage';

const LAST_KEY = 'crash.last';
const STACK_LIMIT = 8_000;

export type CrashRecord = {
  at: number;
  fatal: boolean;
  origin: 'handler' | 'boundary';
  message: string;
  stack: string | null;
  componentStack: string | null;
};

type CrashState = {
  last: CrashRecord | null;
};

export const useCrashStore = create<CrashState>(() => ({
  last: readLast(),
}));

function readLast(): CrashRecord | null {
  const held = storage.getString(LAST_KEY);
  if (!held) return null;
  try {
    const parsed: unknown = JSON.parse(held);
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function isRecord(value: unknown): value is CrashRecord {
  if (typeof value !== 'object' || value === null) return false;
  const held = value as Record<string, unknown>;
  return typeof held.at === 'number' && typeof held.message === 'string';
}

function clip(stack: string | null | undefined): string | null {
  if (typeof stack !== 'string' || stack.trim().length === 0) return null;
  return stack.length > STACK_LIMIT ? stack.slice(0, STACK_LIMIT) : stack;
}

export function recordCrash(
  reason: unknown,
  origin: CrashRecord['origin'],
  fatal: boolean,
  componentStack?: string | null,
): void {
  try {
    const record: CrashRecord = {
      at: Date.now(),
      fatal,
      origin,
      message: describeError(reason),
      stack: clip(reason instanceof Error ? reason.stack : null),
      componentStack: clip(componentStack),
    };
    storage.set(LAST_KEY, JSON.stringify(record));
    useCrashStore.setState({ last: record });
  } catch {
    // a handler that throws would replace the crash we are trying to describe
  }
}

export function clearLastCrash(): void {
  storage.remove(LAST_KEY);
  useCrashStore.setState({ last: null });
}

export function formatCrash(record: CrashRecord): string {
  const lines = [
    `${new Date(record.at).toISOString()} ${record.fatal ? 'fatal' : 'caught'} (${record.origin})`,
    record.message,
  ];
  if (record.stack) lines.push('', record.stack);
  if (record.componentStack) lines.push('', record.componentStack);
  return lines.join('\n');
}

let installed = false;

export function installCrashHandlers(): void {
  if (installed) return;
  installed = true;

  const previous = ErrorUtils.getGlobalHandler();
  ErrorUtils.setGlobalHandler((error, isFatal) => {
    recordCrash(error, 'handler', isFatal ?? false);
    previous(error, isFatal);
  });
}
