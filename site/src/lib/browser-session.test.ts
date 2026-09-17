import type { UpdateEvent } from '@bridgething/browser';
import type { OtaRun } from '@bridgething/companion-types';
import type { BridgeThingMeta, WebappInfo } from '@bridgething/lib';
import { describe, expect, test } from 'bun:test';
import { advance, toConfigField, toDeviceMeta, toWebappInfo } from './browser-session.ts';

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
    imageVersion: '2026.5.1',
    imageBuildId: 'b1',
    imageBuildDate: '2026-05-01',
    imageDistro: 'superbird',
    imageMachine: 'superbird',
    discord: 'https://discord.gg/x',
    credits: 'everyone',
  };
}

function webapp(overrides: Partial<WebappInfo> = {}): WebappInfo {
  return {
    id: 'app',
    name: 'Weather',
    source: 'installed',
    role: 'standard',
    version: '1.0.0',
    description: 'the sky',
    iconHash: 'abc',
    settingsHash: null,
    overlayHash: null,
    config: [],
    permissions: ['net.fetch'],
    rendersVoiceDisplay: false,
    art: null,
    provenance: 'https://example.com/catalog.json',
    ...overrides,
  };
}

function run(overrides: Partial<OtaRun> = {}): OtaRun {
  return {
    runId: 'browser-1',
    deviceId: 'device',
    kind: 'image',
    phase: 'downloading',
    steps: [],
    stepId: 0,
    startedAtMs: 0,
    phaseStartedAtMs: 0,
    stageReceived: null,
    stageTotal: null,
    ratePerSec: null,
    dwlPercent: null,
    outcome: null,
    error: null,
    releaseVersion: null,
    daemonVersion: null,
    imageVersion: null,
    channel: null,
    rootUrl: null,
    resumable: false,
    webappId: null,
    webappName: null,
    ...overrides,
  };
}

describe('toDeviceMeta', () => {
  test('the daemon version is the app version the wire announces', () => {
    expect(toDeviceMeta(meta())).toEqual({
      daemonVersion: '0.8.1',
      libbridgethingVersion: '0.4.0',
      imageVersion: '2026.5.1',
      appName: 'bridgething',
      osName: 'superbird',
      osVersion: '1.2.3',
      channel: 'stable',
      modelName: 'Car Thing',
      serialNumber: 'SB0001',
      nickname: 'the dashboard',
      launcherGesture: 'fivePress',
    });
  });
});

describe('toConfigField', () => {
  test('a string field keeps its bounds and drops the numeric ones', () => {
    expect(
      toConfigField({
        type: 'string',
        data: { key: 'city', label: 'City', pattern: '^\\w+$', minLength: 2, maxLength: 40, default: 'boston' },
      }),
    ).toEqual({
      kind: 'string',
      key: 'city',
      label: 'City',
      pattern: '^\\w+$',
      minLength: 2,
      maxLength: 40,
      min: null,
      max: null,
      step: null,
      choices: [],
      defaultValue: 'boston',
    });
  });

  test('a secret keeps its own kind rather than collapsing into string', () => {
    expect(
      toConfigField({
        type: 'secret',
        data: { key: 'token', label: 'Token', pattern: null, minLength: null, maxLength: null, default: null },
      }).kind,
    ).toBe('secret');
  });

  test('non-string defaults are stringified, because the flat surface only carries text', () => {
    expect(
      toConfigField({ type: 'number', data: { key: 'n', label: 'N', min: 0, max: 9, step: 1, default: 4 } })
        .defaultValue,
    ).toBe('4');
    expect(toConfigField({ type: 'boolean', data: { key: 'b', label: 'B', default: false } }).defaultValue).toBe(
      'false',
    );
  });

  test('a zero default survives, since it is a value and not an absence', () => {
    expect(
      toConfigField({ type: 'number', data: { key: 'n', label: 'N', min: null, max: null, step: null, default: 0 } })
        .defaultValue,
    ).toBe('0');
  });

  test('an enum carries its choices', () => {
    expect(
      toConfigField({ type: 'enum', data: { key: 'u', label: 'Units', choices: ['c', 'f'], default: 'c' } }).choices,
    ).toEqual(['c', 'f']);
  });
});

describe('toWebappInfo', () => {
  test('the wire-only fields drop out and everything else survives', () => {
    const projected = toWebappInfo(webapp());
    expect(projected).not.toHaveProperty('rendersVoiceDisplay');
    expect(projected).not.toHaveProperty('art');
    expect(projected.provenance).toBe('https://example.com/catalog.json');
    expect(projected.permissions).toEqual(['net.fetch']);
  });

  test('declared config fields come across flattened', () => {
    const projected = toWebappInfo(
      webapp({ config: [{ type: 'boolean', data: { key: 'metric', label: 'Metric', default: true } }] }),
    );
    expect(projected.config).toEqual([
      {
        kind: 'boolean',
        key: 'metric',
        label: 'Metric',
        pattern: null,
        minLength: null,
        maxLength: null,
        min: null,
        max: null,
        step: null,
        choices: [],
        defaultValue: 'true',
      },
    ]);
  });
});

describe('advance', () => {
  test('a streaming phase moves the run and counts the bytes', () => {
    const event: UpdateEvent = { kind: 'progress', stepId: 2, phase: { kind: 'streaming', sent: 512, total: 2048 } };
    expect(advance(run(), event)).toMatchObject({
      phase: 'streaming',
      stepId: 2,
      stageReceived: 512,
      stageTotal: 2048,
    });
  });

  test('applying is a write, which is the phase the daemon is actually in', () => {
    const event: UpdateEvent = { kind: 'progress', phase: { kind: 'applying', writePercent: 40 } };
    expect(advance(run(), event)).toMatchObject({ phase: 'writing', dwlPercent: 40 });
  });

  test('a failure carries its reason and settles the outcome', () => {
    const failed = advance(run(), { kind: 'failed', reason: 'the slot would not flip' });
    expect(failed.outcome).toBe('failed');
    expect(failed.error).toBe('the slot would not flip');
  });

  test('a plan fills the steps a progress event then indexes into', () => {
    const planned = advance(run(), {
      kind: 'planned',
      steps: [{ id: 0, kind: 'download', label: 'system.img', bytes: 10 }],
    });
    expect(planned.steps).toHaveLength(1);
  });

  test('a plan carries the channel and host it was resolved against', () => {
    const planned = advance(run(), {
      kind: 'planned',
      steps: [],
      channel: 'dev',
      rootUrl: 'https://ota.example',
    });
    expect(planned.channel).toBe('dev');
    expect(planned.rootUrl).toBe('https://ota.example');
  });

  test('a plan that names neither keeps what the run already had', () => {
    const seeded = { ...run(), channel: 'stable', rootUrl: 'https://ota.bridgething.com' };
    const planned = advance(seeded, { kind: 'planned', steps: [] });
    expect(planned.channel).toBe('stable');
    expect(planned.rootUrl).toBe('https://ota.bridgething.com');
  });

  test('events with nothing to say leave the run alone', () => {
    const held = run();
    expect(advance(held, { kind: 'manifestPolled' })).toBe(held);
    expect(advance(held, { kind: 'progress' })).toBe(held);
  });
});
