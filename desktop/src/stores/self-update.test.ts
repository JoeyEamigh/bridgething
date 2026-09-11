import { beforeEach, describe, expect, mock, test } from 'bun:test';

type Fake = {
  available: boolean;
  version: string;
  date?: string;
  body?: string;
  download: () => Promise<void>;
  install: () => Promise<void>;
};

let offered: Fake | null = null;
let refuse: Error | null = null;
let installed = 0;
let restarted = 0;

mock.module('@tauri-apps/plugin-updater', () => ({
  check: async () => {
    if (refuse) throw refuse;
    return offered;
  },
}));

mock.module('../lib/lifecycle.ts', () => ({
  restart: async () => {
    restarted += 1;
  },
  quit: async () => {},
}));

const { applySelfUpdate, checkSelfUpdate, selfUpdate } = await import('./self-update.ts');

function available(version: string): Fake {
  return {
    available: true,
    version,
    download: async () => {},
    install: async () => {
      installed += 1;
    },
  };
}

describe('the desktop app keeping itself current', () => {
  beforeEach(() => {
    selfUpdate.value = { kind: 'idle' };
    offered = null;
    refuse = null;
    installed = 0;
    restarted = 0;
  });

  test('a published build is fetched on sight, without waiting to be asked', async () => {
    offered = available('0.9.0');

    await checkSelfUpdate();

    expect(selfUpdate.value.kind).toBe('ready');
  });

  test('nothing newer leaves the app saying so', async () => {
    await checkSelfUpdate();

    expect(selfUpdate.value.kind).toBe('current');
  });

  test('a fetched build is applied and the app comes back on it', async () => {
    offered = available('0.9.0');
    await checkSelfUpdate();

    await applySelfUpdate();

    expect(installed).toBe(1);
    expect(restarted).toBe(1);
  });

  test('a build already waiting is not re-fetched by the next sweep', async () => {
    offered = available('0.9.0');
    await checkSelfUpdate();
    offered = available('1.0.0');

    await checkSelfUpdate();

    expect(selfUpdate.value.kind === 'ready' && selfUpdate.value.update.version).toBe('0.9.0');
  });

  test('a feed that cannot be reached is reported, and the next sweep tries again', async () => {
    refuse = new Error('no signing key yet');
    await checkSelfUpdate();
    expect(selfUpdate.value).toEqual({ kind: 'unavailable', reason: 'no signing key yet' });

    refuse = null;
    offered = available('0.9.0');
    await checkSelfUpdate();

    expect(selfUpdate.value.kind).toBe('ready');
  });
});
