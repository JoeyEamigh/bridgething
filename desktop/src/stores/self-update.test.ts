import { beforeEach, describe, expect, test } from 'bun:test';

import type { Invalidation } from '@bridgething/ui';

import type { DesktopSession, SelfUpdateState } from '../desktop.ts';
import { postponedUpdate, promptedUpdate, seed, selfUpdate } from './session.ts';

class Host {
  held: SelfUpdateState = { kind: 'current', checkedAt: '2026-10-22T17:47:58Z' };
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
      selfUpdate: () => {
        this.pulls += 1;
        return Promise.resolve(this.held);
      },
      extensions: answer([]),
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

  hint(topic: Invalidation['topic'], next: SelfUpdateState): void {
    this.held = next;
    for (const watcher of this.watchers) watcher({ topic, id: null });
  }
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe('the self-update store', () => {
  let host: Host;

  beforeEach(async () => {
    host = new Host();
    selfUpdate.data.value = { kind: 'idle' };
    await seed(host.session());
  });

  test('the shell state is there before the first render, never fetched on mount', () => {
    expect(host.pulls).toBe(1);
    expect(selfUpdate.data.value.kind).toBe('current');
  });

  test('a build the shell fetched in the background shows up off the hint', async () => {
    host.hint('self-update', { kind: 'ready', version: '0.15.0', date: null, notes: null });
    await settle();

    expect(selfUpdate.data.value).toEqual({ kind: 'ready', version: '0.15.0', date: null, notes: null });
  });

  test('a ready build prompts until it is put off, and a newer one prompts again', async () => {
    postponedUpdate.value = null;
    host.hint('self-update', { kind: 'ready', version: '0.15.0', date: null, notes: null });
    await settle();
    expect(promptedUpdate.value?.version).toBe('0.15.0');

    postponedUpdate.value = '0.15.0';
    host.hint('self-update', { kind: 'ready', version: '0.15.0', date: null, notes: null });
    await settle();
    expect(promptedUpdate.value).toBeNull();
    expect(selfUpdate.data.value.kind).toBe('ready');

    host.hint('self-update', { kind: 'ready', version: '0.16.0', date: null, notes: null });
    await settle();
    expect(promptedUpdate.value?.version).toBe('0.16.0');
  });
});
