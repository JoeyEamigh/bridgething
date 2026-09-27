import { compositeVersion } from '@bridgething/browser';
import type { BridgeThingMeta } from '@bridgething/lib';
import { afterEach, beforeAll, describe, expect, test } from 'bun:test';
import { resolveUpdate } from './wired.ts';

const ROOT = 'https://ota.test';
const LATEST = '0.9.0+image.2026.06.0';
const realFetch = globalThis.fetch;

beforeAll(async () => {
  await compositeVersion(LATEST);
});

afterEach(() => {
  globalThis.fetch = realFetch;
});

function meta(): BridgeThingMeta {
  return {
    bridgethingVersion: '0.8.0',
    libbridgethingVersion: '0.4.0',
    appName: 'bridgething',
    nickname: 'the dashboard',
    launcherGesture: 'fivePress',
    appVersion: '0.8.1',
    daemonSha256: null,
    wakewordModelVersion: null,
    osName: 'superbird',
    osVersion: '1.2.3',
    osDescription: 'superbird 1.2.3',
    btMac: 'aa:bb:cc:dd:ee:ff',
    serialNumber: 'SB0001',
    fccId: '2AJHK-SB',
    icId: '22222-SB',
    modelName: 'Car Thing',
    channel: 'stable',
    imageVariant: 'prod',
    imageVersion: '2026.05.1',
    imageBuildId: 'b1',
    imageBuildDate: '2026-05-01',
    imageDistro: 'superbird',
    imageMachine: 'superbird',
    discord: 'https://discord.gg/x',
    credits: 'everyone',
  };
}

function serveManifest(release: { yanked: string | null; deprecated: boolean }): void {
  const body = {
    manifest_version: 1,
    updated_at: '2026-08-03T00:00:00Z',
    channels: {
      stable: { name: 'stable', stability: 'stable', default: true, latest: LATEST, releases: [LATEST] },
    },
    releases: {
      [LATEST]: {
        version: LATEST,
        channel: 'stable',
        yanked: release.yanked,
        deprecated: release.deprecated,
        builtin_webapps: {},
        wakeword: null,
        artifacts: null,
      },
    },
  };
  globalThis.fetch = (async () =>
    new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    })) as unknown as typeof fetch;
}

describe('resolveUpdate', () => {
  test('a healthy release is offered as a plan', async () => {
    serveManifest({ yanked: null, deprecated: false });

    const plan = await resolveUpdate(meta(), 'stable', ROOT);

    expect(plan).toMatchObject({ kind: 'image', version: LATEST, to: { daemon: '0.9.0', image: '2026.06.0' } });
  });

  test('a yanked release is refused', async () => {
    serveManifest({ yanked: 'bricks the boot slot', deprecated: false });

    await expect(resolveUpdate(meta(), 'stable', ROOT)).rejects.toThrow(/withdrawn/);
  });

  test('a deprecated release is refused, the same rule the daemon polls by', async () => {
    serveManifest({ yanked: null, deprecated: true });

    await expect(resolveUpdate(meta(), 'stable', ROOT)).rejects.toThrow(/deprecated/);
  });

  test('a channel the manifest does not carry is refused', async () => {
    serveManifest({ yanked: null, deprecated: false });

    await expect(resolveUpdate(meta(), 'nightly', ROOT)).rejects.toThrow(/nightly/);
  });
});
