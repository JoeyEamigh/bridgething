import type { AppEntry, Catalog } from '@bridgething/catalog';

import { DEVICE, otaRun, peer } from './fixtures';
import { rig, type Rig } from './harness';

const OFFICIAL = 'https://apps.bridgething.com/catalog.json';

function app(version: string): AppEntry {
  return {
    id: '01890000-0000-7000-8000-0000000000AB',
    name: 'example',
    description: 'example does a thing',
    author: 'someone',
    icon: null,
    homepage: null,
    source: null,
    versions: [
      {
        version,
        released_at: '2026-05-31T00:00:00Z',
        download: {
          url: `https://example.test/r/app/${version}.zip`,
          size: 4096,
          sha256: 'a'.repeat(64),
        },
        permissions: [],
        min_libbridgething_version: '0.1.0',
        changelog: null,
      },
    ],
  };
}

function catalog(version: string): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-05-31T00:00:00Z',
    repo: {
      name: 'official',
      description: 'official',
      homepage: null,
      icon: null,
    },
    apps: [app(version)],
    recommended_sources: [],
  };
}

function serve(served: Record<string, Catalog>): void {
  globalThis.fetch = jest.fn((url: string) => {
    const body = served[url];
    if (!body)
      return Promise.resolve({ ok: false, status: 503, json: () => ({}) });
    return Promise.resolve({ ok: true, status: 200, json: () => body });
  }) as unknown as typeof fetch;
}

function installedEntry(version: string) {
  return {
    deviceId: DEVICE,
    webapps: [
      {
        id: '01890000-0000-7000-8000-0000000000ab',
        name: 'example',
        version,
        source: 'installed' as const,
        role: 'standard' as const,
        provenance: OFFICIAL,
        config: [],
        permissions: [],
      },
    ],
    active: undefined,
  };
}

function installs(r: Rig): unknown[][] {
  const calls: unknown[][] = [];
  r.native.__returns.set('installWebappFromUrl', (...args: unknown[]) => {
    calls.push(args);
    return Promise.resolve(null);
  });
  return calls;
}

function connect(r: Rig, version: string): void {
  r.native.__world.webapps = [installedEntry(version)];
  r.emit('peerConnected', peer());
  r.emit('webappsChanged', installedEntry(version));
}

const tick = () => new Promise<void>(resolve => setTimeout(resolve, 0));

describe('webapp auto-update', () => {
  test('a pending update on a connected device installs itself', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('2.0.0') });
    const calls = installs(r);
    r.catalog.startWebappAutoUpdate();
    connect(r, '1.0.0');

    await r.catalog.refreshCatalog();
    await tick();

    expect(calls).toEqual([
      [
        DEVICE,
        'https://example.test/r/app/2.0.0.zip',
        'a'.repeat(64),
        4096,
        OFFICIAL,
        '01890000-0000-7000-8000-0000000000AB',
        'example',
      ],
    ]);
  });

  test('nothing installs with the automatic switch off', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('2.0.0') });
    const calls = installs(r);
    const off = { intervalSeconds: 3600, autoPush: false, rootUrl: undefined };
    r.native.__world.otaPollConfig = off;
    r.session.useSessionStore.setState({ otaPollConfig: off });
    r.catalog.startWebappAutoUpdate();
    connect(r, '1.0.0');

    await r.catalog.refreshCatalog();
    await tick();

    expect(calls).toEqual([]);
  });

  test('an already-current app is left alone', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('1.0.0') });
    const calls = installs(r);
    r.catalog.startWebappAutoUpdate();
    connect(r, '1.0.0');

    await r.catalog.refreshCatalog();
    await tick();

    expect(calls).toEqual([]);
  });

  test('a failed install is tried once, not looped', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('2.0.0') });
    const attempts: unknown[][] = [];
    r.native.__returns.set('installWebappFromUrl', (...args: unknown[]) => {
      attempts.push(args);
      return Promise.reject(new Error('device refused'));
    });
    r.catalog.startWebappAutoUpdate();
    connect(r, '1.0.0');

    await r.catalog.refreshCatalog();
    await tick();
    r.emit('webappsChanged', installedEntry('1.0.0'));
    await tick();

    expect(attempts).toHaveLength(1);
  });

  test('a device mid-firmware-update is not poked', async () => {
    const r = rig();
    serve({ [OFFICIAL]: catalog('2.0.0') });
    const calls = installs(r);
    r.native.__world.otaRuns = [otaRun()];
    r.catalog.startWebappAutoUpdate();
    connect(r, '1.0.0');
    r.emit('otaRunChanged', otaRun());

    await r.catalog.refreshCatalog();
    await tick();

    expect(calls).toEqual([]);
  });
});

describe('webapp auto-update and native extensions', () => {
  test('an app that ships a native extension is never updated from the phone', async () => {
    const r = rig();
    const entry = app('2.0.0');
    serve({
      [OFFICIAL]: {
        ...catalog('2.0.0'),
        apps: [
          {
            ...entry,
            versions: entry.versions.map(version => ({
              ...version,
              extension: { desktop: true as const, permissions: ['all'] },
            })),
          },
        ],
      },
    });
    const calls = installs(r);
    r.catalog.startWebappAutoUpdate();
    connect(r, '1.0.0');

    await r.catalog.refreshCatalog();
    await tick();

    expect(calls).toEqual([]);
  });
});
