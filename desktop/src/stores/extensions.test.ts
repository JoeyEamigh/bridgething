import { beforeEach, describe, expect, test } from 'bun:test';

import type { Invalidation } from '@bridgething/ui';

import type { DesktopSession, ExtensionEntry } from '../desktop.ts';
import { extensions, seed } from './session.ts';

function entry(id: string, enabled: boolean): ExtensionEntry {
  return {
    id,
    name: 'weather',
    version: '1.0.0',
    permissions: ['all'],
    api: 1,
    enabled,
    dataDir: `/data/${id}`,
    status: enabled ? { kind: 'running' } : { kind: 'stopped' },
    orphaned: false,
  };
}

class Host {
  held: ExtensionEntry[] = [entry('a', true)];
  pulls = 0;
  private watchers = new Set<(event: Invalidation) => void>();

  session(): DesktopSession {
    const answer =
      <T>(value: T) =>
      () =>
        Promise.resolve(value);
    return {
      tier: 'companion',
      host: 'desktop',
      subscribe: (listener: (event: Invalidation) => void) => {
        this.watchers.add(listener);
        return () => this.watchers.delete(listener);
      },
      extensions: () => {
        this.pulls += 1;
        return Promise.resolve(this.held);
      },
      snapshot: answer(null),
      endpoints: answer([]),
      capabilitySupport: answer(null),
      defaultGateway: answer(null),
      route: answer(null),
      catalogSources: answer([]),
      knownDevices: answer([]),
      selectedDevice: answer(null),
      webapps: answer([]),
      webappActive: answer(null),
      webappSlots: answer({ launcher: null, overlay: null }),
      deviceAutoResume: answer(true),
      deviceResumeTarget: answer('anySpeaker'),
      deviceLogStreaming: answer(false),
      debugLogging: answer(false),
    } as unknown as DesktopSession;
  }

  change(next: ExtensionEntry[]): void {
    this.held = next;
    for (const watcher of this.watchers) watcher({ topic: 'extensions', id: null });
  }
}

describe('the extensions store', () => {
  let host: Host;

  beforeEach(async () => {
    host = new Host();
    extensions.data.value = [];
    await seed(host.session());
  });

  test('the list is there before the first render, never fetched on mount', () => {
    expect(host.pulls).toBe(1);
    expect(extensions.data.value).toEqual([entry('a', true)]);
  });

  test('a host-local status change refreshes the list without a device round trip', async () => {
    host.change([entry('a', false)]);
    await Promise.resolve();
    await Promise.resolve();

    expect(host.pulls).toBe(2);
    expect(extensions.data.value[0]?.status).toEqual({ kind: 'stopped' });
  });

  test('an uninstall drops the row rather than leaving a stale one', async () => {
    host.change([]);
    await Promise.resolve();
    await Promise.resolve();

    expect(extensions.data.value).toEqual([]);
  });
});
