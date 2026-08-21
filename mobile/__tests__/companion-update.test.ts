import { rig, type Rig } from './harness';

const MANIFEST = 'https://ota.bridgething.com/companion.json';

const RELEASE = {
  version: '0.9.0',
  url: 'https://ota.bridgething.com/companion/android/0.9.0/bridgething-0.9.0.apk',
  size: 48_000_000,
  sha256: 'A'.repeat(64),
  released_at: '2026-08-01T00:00:00Z',
};

function serve(body: unknown, status = 200): jest.Mock {
  const fetchMock = jest.fn(() =>
    Promise.resolve({ ok: status < 400, status, json: () => body }),
  );
  globalThis.fetch = fetchMock as unknown as typeof fetch;
  return fetchMock;
}

async function booted(r: Rig): Promise<void> {
  await r.bridge.reconcileAll();
}

const tick = () => new Promise(resolve => setTimeout(resolve, 0));

describe('reading the companion manifest', () => {
  test('takes the platform entry and lowercases the digest', () => {
    const r = rig({ platform: 'android' });

    expect(
      r.companionUpdate.releaseFrom({ android: RELEASE }, 'android'),
    ).toEqual({
      version: '0.9.0',
      url: RELEASE.url,
      size: RELEASE.size,
      sha256: 'a'.repeat(64),
    });
  });

  test('refuses malformed entries rather than guessing', () => {
    const r = rig({ platform: 'android' });
    const { releaseFrom } = r.companionUpdate;

    expect(releaseFrom(null, 'android')).toBeNull();
    expect(releaseFrom({ ios: RELEASE }, 'android')).toBeNull();
    expect(
      releaseFrom({ android: { ...RELEASE, url: 'ftp://plain' } }, 'android'),
    ).toBeNull();
    expect(
      releaseFrom({ android: { ...RELEASE, sha256: 'abc' } }, 'android'),
    ).toBeNull();
    expect(
      releaseFrom({ android: { ...RELEASE, size: 0 } }, 'android'),
    ).toBeNull();
    expect(
      releaseFrom({ android: { ...RELEASE, version: '' } }, 'android'),
    ).toBeNull();
  });

  test('only a strictly newer version counts', () => {
    const r = rig({ platform: 'android' });
    const { isNewer } = r.companionUpdate;

    expect(isNewer('0.9.0', '0.6.0')).toBe(true);
    expect(isNewer('0.6.0', '0.6.0')).toBe(false);
    expect(isNewer('0.5.9', '0.6.0')).toBe(false);
    expect(isNewer('0.9.0', null)).toBe(false);
  });
});

describe('checking for an app update', () => {
  test('learns the installed version from the snapshot and surfaces a newer release', async () => {
    const r = rig({ platform: 'android' });
    serve({ android: RELEASE });
    await booted(r);

    const found = await r.companionUpdate.checkCompanionUpdate();

    expect(found?.version).toBe('0.9.0');
    expect(
      r.companionUpdate.useCompanionUpdateStore.getState().release?.version,
    ).toBe('0.9.0');
  });

  test('a release no newer than the installed app is not an update', async () => {
    const r = rig({ platform: 'android' });
    serve({ android: { ...RELEASE, version: '0.6.0' } });
    await booted(r);

    expect(await r.companionUpdate.checkCompanionUpdate()).toBeNull();
    expect(
      r.companionUpdate.useCompanionUpdateStore.getState().release,
    ).toBeNull();
  });

  test('an unreachable manifest throws instead of clearing a held release', async () => {
    const r = rig({ platform: 'android' });
    serve({ android: RELEASE });
    await booted(r);
    await r.companionUpdate.checkCompanionUpdate();
    serve({}, 503);

    await expect(r.companionUpdate.checkCompanionUpdate()).rejects.toThrow(
      '503',
    );
    expect(
      r.companionUpdate.useCompanionUpdateStore.getState().release?.version,
    ).toBe('0.9.0');
  });

  test('a held update host override is where the manifest comes from', async () => {
    const r = rig({ platform: 'android' });
    const fetchMock = serve({ android: RELEASE });
    await booted(r);

    await r.companionUpdate.checkCompanionUpdate('http://10.0.2.2:8899/');

    expect(fetchMock.mock.calls[0]?.[0]).toBe(
      'http://10.0.2.2:8899/companion.json',
    );
    expect(r.companionUpdate.manifestHost('http://10.0.2.2:8899/')).toBe(
      '10.0.2.2',
    );
    expect(r.companionUpdate.manifestHost(r.storage.DEFAULT_OTA_ROOT_URL)).toBe(
      'ota.bridgething.com',
    );
  });

  test('hits the manifest under the stock ota root', async () => {
    const r = rig({ platform: 'android' });
    const fetchMock = serve({ android: RELEASE });
    await booted(r);

    await r.companionUpdate.checkCompanionUpdate();

    expect(fetchMock.mock.calls[0]?.[0]).toBe(MANIFEST);
  });
});

describe('installing an app update', () => {
  test('hands native the url, filename, and digest, and tracks progress from native events', async () => {
    const r = rig({ platform: 'android' });
    const attempts: unknown[][] = [];
    let finish: () => void = () => undefined;
    r.native.__returns.set('installCompanionUpdate', (...args: unknown[]) => {
      attempts.push(args);
      return new Promise<void>(resolve => {
        finish = resolve;
      });
    });
    const release = r.companionUpdate.releaseFrom(
      { android: RELEASE },
      'android',
    );
    if (!release) throw new Error('fixture did not parse');

    const running = r.companionUpdate.startCompanionUpdate(release);
    await tick();
    expect(attempts).toEqual([
      [RELEASE.url, 'bridgething-0.9.0.apk', RELEASE.size, 'a'.repeat(64)],
    ]);
    expect(r.companionUpdate.useCompanionUpdateStore.getState().phase).toEqual({
      kind: 'downloading',
      received: 0,
      total: RELEASE.size,
    });

    r.emit('companionUpdateProgress', 24_000_000, RELEASE.size);
    expect(r.companionUpdate.useCompanionUpdateStore.getState().phase).toEqual({
      kind: 'downloading',
      received: 24_000_000,
      total: RELEASE.size,
    });

    finish();
    await running;
    expect(r.companionUpdate.useCompanionUpdateStore.getState().phase).toEqual({
      kind: 'idle',
    });
  });

  test('a failed download is reported, not swallowed', async () => {
    const r = rig({ platform: 'android' });
    r.native.__returns.set('installCompanionUpdate', () =>
      Promise.reject(new Error('sha256 mismatch')),
    );
    const release = r.companionUpdate.releaseFrom(
      { android: RELEASE },
      'android',
    );
    if (!release) throw new Error('fixture did not parse');

    await r.companionUpdate.startCompanionUpdate(release);

    expect(r.companionUpdate.useCompanionUpdateStore.getState().phase).toEqual({
      kind: 'failed',
      reason: 'sha256 mismatch',
    });
  });

  test('skipping a version survives a relaunch', async () => {
    const r = rig({ platform: 'android' });
    r.companionUpdate.dismissCompanionUpdate('0.9.0');

    const again = r.relaunch();

    expect(
      again.companionUpdate.useCompanionUpdateStore.getState().dismissed,
    ).toBe('0.9.0');
  });
});
