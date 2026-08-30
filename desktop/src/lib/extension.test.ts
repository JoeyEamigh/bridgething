import { describe, expect, test } from 'bun:test';

import type { ExtensionEntry, ExtensionStatus } from '../desktop.ts';
import {
  describeExtensionStatus,
  extensionFor,
  needsRuntime,
  orphanedExtensions,
  sideloadConsent,
} from './extension.ts';

function entry(id: string, status: ExtensionStatus, orphaned = false): ExtensionEntry {
  return {
    id,
    name: 'weather',
    version: '1.0.0',
    permissions: ['net:example.com'],
    api: 1,
    enabled: true,
    dataDir: `/data/${id}`,
    status,
    orphaned,
  };
}

const EVERY: ExtensionStatus[] = [
  { kind: 'starting' },
  { kind: 'running' },
  { kind: 'crashed', reason: 'exited 1' },
  { kind: 'stopped' },
  { kind: 'runtime-missing', reason: 'offline' },
  { kind: 'refused', reason: 'asks for run, which the install did not offer' },
];

describe('describeExtensionStatus', () => {
  test('every status the host can report has copy, so no row ever renders blank', () => {
    for (const status of EVERY) {
      const copy = describeExtensionStatus(status);
      expect(copy.label.length).toBeGreaterThan(0);
      expect(copy.label).toBe(copy.label.toLowerCase());
    }
  });

  test('only the failure states are tinted as failures', () => {
    expect(describeExtensionStatus({ kind: 'running' }).tone).toBe('ok');
    expect(describeExtensionStatus({ kind: 'stopped' }).tint).toBe('default');
    expect(describeExtensionStatus({ kind: 'crashed', reason: 'exited 1' }).tone).toBe('err');
    expect(describeExtensionStatus({ kind: 'runtime-missing', reason: 'offline' }).tone).toBe('warn');
    expect(describeExtensionStatus({ kind: 'refused', reason: 'asks for run' }).tone).toBe('err');
  });

  test('a failure carries its reason, so the user is never told only that something broke', () => {
    expect(describeExtensionStatus({ kind: 'crashed', reason: 'exited 137' }).detail).toContain('exited 137');
    expect(describeExtensionStatus({ kind: 'runtime-missing', reason: 'no network' }).detail).toBe('no network');
    expect(describeExtensionStatus({ kind: 'refused', reason: 'asks for run' }).detail).toBe('asks for run');
  });

  test('a healthy status has nothing extra to say', () => {
    expect(describeExtensionStatus({ kind: 'running' }).detail).toBe(null);
    expect(describeExtensionStatus({ kind: 'stopped' }).detail).toBe(null);
  });
});

describe('needsRuntime', () => {
  test('only a missing runtime offers the retry, never a crash', () => {
    const offered = EVERY.filter(needsRuntime).map(status => status.kind);
    expect(offered).toEqual(['runtime-missing']);
  });
});

describe('extensionFor', () => {
  test('an app with no extension gets no row', () => {
    expect(extensionFor([entry('a', { kind: 'running' })], 'b')).toBe(null);
    expect(extensionFor([], 'a')).toBe(null);
  });

  test('the row belongs to the app that shipped it', () => {
    const held = [entry('a', { kind: 'running' }), entry('b', { kind: 'stopped' })];
    expect(extensionFor(held, 'b')?.status).toEqual({ kind: 'stopped' });
  });
});

describe('orphanedExtensions', () => {
  test('an extension a Car Thing still holds is reached through its app, not through a row of its own', () => {
    expect(orphanedExtensions([entry('a', { kind: 'running' }), entry('b', { kind: 'stopped' })])).toEqual([]);
  });

  test('an extension no device claims is surfaced, or its toggle and its removal are unreachable', () => {
    const held = [entry('a', { kind: 'running' }), entry('b', { kind: 'running' }, true)];
    expect(orphanedExtensions(held).map(found => found.id)).toEqual(['b']);
  });
});

describe('sideloadConsent', () => {
  test('a picked bundle that declares an extension has to be confirmed before it installs', () => {
    expect(sideloadConsent({ permissions: ['all'], api: 1 })).toEqual({ desktop: true, permissions: ['all'] });
  });

  test('an extension asking for nothing still gets a dialog, because it is still a native process', () => {
    expect(sideloadConsent({ permissions: [], api: 1 })).toEqual({ desktop: true, permissions: [] });
  });

  test('a plain webapp installs without a dialog', () => {
    expect(sideloadConsent(null)).toBeNull();
  });
});
