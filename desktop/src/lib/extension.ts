import type { AppExtension } from '@bridgething/catalog';
import type { WebappInfo } from '@bridgething/companion-types';
import type { RowTint, Tone } from '@bridgething/ui';

import type { BundleExtension, ExtensionEntry, ExtensionStatus } from '../desktop.ts';
import type { IconName } from './icons.tsx';

export type ExtensionCopy = { tone: Tone; tint: RowTint; label: string; detail: string | null; icon: IconName };

export function describeExtensionStatus(status: ExtensionStatus): ExtensionCopy {
  switch (status.kind) {
    case 'starting':
      return {
        tone: 'accent',
        tint: 'accent',
        label: 'starting',
        detail: 'waiting for the extension to say it is ready',
        icon: 'clock',
      };
    case 'running':
      return { tone: 'ok', tint: 'ok', label: 'running', detail: null, icon: 'play' };
    case 'crashed':
      return {
        tone: 'err',
        tint: 'err',
        label: 'crashed',
        detail: `${status.reason}. retrying with a growing delay.`,
        icon: 'undo',
      };
    case 'stopped':
      return { tone: 'neutral', tint: 'default', label: 'stopped', detail: null, icon: 'power' };
    case 'refused':
      return { tone: 'err', tint: 'err', label: 'not permitted', detail: status.reason, icon: 'shield' };
    case 'runtime-missing':
      return { tone: 'warn', tint: 'warn', label: 'no runtime', detail: status.reason, icon: 'download' };
  }
}

export function needsRuntime(status: ExtensionStatus): boolean {
  return status.kind === 'runtime-missing';
}

export function extensionFor(entries: readonly ExtensionEntry[], webappId: string): ExtensionEntry | null {
  return entries.find(entry => entry.id === webappId) ?? null;
}

export function orphanedExtensions(entries: readonly ExtensionEntry[]): ExtensionEntry[] {
  return entries.filter(entry => entry.orphaned);
}

export const EXTENSION_MISSING: ExtensionCopy = {
  tone: 'err',
  tint: 'err',
  label: 'extension missing',
  detail: 'nothing was taken out of the bundle here, so it cannot run. reinstall the app from this computer.',
  icon: 'undo',
};

export function sideloadConsent(declared: BundleExtension | null): AppExtension | null {
  return declared === null ? null : { desktop: true, permissions: declared.permissions };
}

export function extensionMissing(entries: readonly ExtensionEntry[], webapp: WebappInfo): boolean {
  return webapp.extension !== null && extensionFor(entries, webapp.id) === null;
}
