import { describe, expect, test } from 'bun:test';
import {
  aggregate,
  compareVersions,
  type InstalledWebapp,
  listedWebapps,
  newestCompatible,
  pinsFrom,
  satisfies,
  settingsOrigin,
  settingsOriginFor,
  updates,
  versionCompatible,
} from '../src/resolve.ts';
import type { AppEntry, AppVersion, Catalog, Download } from '../src/types.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const WEATHER_ID = '019e6701-13f8-71b5-ba04-81f347137de2';
const SOURCE_A = 'https://apps.bridgething.com/catalog.json';
const SOURCE_B = 'https://repo.example.com/catalog.json';

function ver(version: string, opts: { minLib?: string; released?: string } = {}): AppVersion {
  return {
    version,
    released_at: opts.released ?? '2026-05-31T00:00:00Z',
    download: { url: `https://apps.bridgething.com/r/${version}.zip`, size: 1, sha256: '0'.repeat(64) },
    permissions: ['net.fetch'],
    min_libbridgething_version: opts.minLib ?? '0.4.0',
    changelog: null,
  };
}

function app(id: string, name: string, versions: AppVersion[]): AppEntry {
  return {
    id,
    name,
    description: 'test',
    author: 'JoeyEamigh',
    icon: null,
    homepage: null,
    source: null,
    versions,
  };
}

function catalog(apps: AppEntry[]): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-05-31T00:00:00Z',
    repo: { name: 'test', description: 'test', homepage: null, icon: null },
    apps,
    recommended_sources: [],
  };
}

function installed(
  id: string,
  version: string,
  opts: { source?: 'builtin' | 'installed'; role?: 'standard' | 'launcher'; provenance?: string } = {},
): InstalledWebapp {
  return {
    id,
    version,
    source: opts.source ?? 'installed',
    role: opts.role ?? 'standard',
    provenance: opts.provenance ?? null,
  };
}

function orderedCatalogs() {
  const a = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.2.0'), ver('0.1.0', { released: '2026-04-01T00:00:00Z' })])]);
  const b = catalog([
    app(CALENDAR_ID, 'Calendar', [
      ver('0.3.0', { minLib: '99.0.0' }),
      ver('0.1.5', { released: '2026-04-15T00:00:00Z' }),
    ]),
    app(WEATHER_ID, 'Weather', [ver('0.1.0')]),
  ]);
  return [
    { url: SOURCE_A, catalog: a },
    { url: SOURCE_B, catalog: b },
  ];
}

describe('semver compat', () => {
  test('strips prefix and suffix', () => {
    expect(satisfies('v0.4.1', '0.4.0')).toBe(true);
    expect(satisfies('0.4.0', '0.4.0')).toBe(true);
    expect(satisfies('v0.3.9', '0.4.0')).toBe(false);
    expect(satisfies('v0.5.0-dev', '0.4.0')).toBe(true);
    expect(satisfies('v2.0.0', '2')).toBe(true);
  });
});

describe('compareVersions', () => {
  test('orders by dotted component, not by string', () => {
    expect(compareVersions('0.10.0', '0.9.0')).toBe(1);
    expect(compareVersions('0.9.0', '0.10.0')).toBe(-1);
    expect(compareVersions('2026.06.0', '2026.06.0')).toBe(0);
  });

  test('build metadata ends the numeric prefix, so a composite version orders by its daemon half', () => {
    expect(compareVersions('0.9.0+image.2026.06.0', '0.9.0')).toBe(0);
    expect(compareVersions('0.9.0+image.2026.06.0', '0.9.0+image.2025.01.0')).toBe(0);
  });

  test('a prerelease sorts with the release it hangs off, and a v prefix is noise', () => {
    expect(compareVersions('v0.9.0-rc.1', '0.9.0')).toBe(0);
    expect(compareVersions('0.9.0-rc.1', '0.8.9')).toBe(1);
  });
});

describe('provenance', () => {
  test('pins come from device reported provenance', () => {
    const pins = pinsFrom([installed(CALENDAR_ID, '0.1.0', { provenance: SOURCE_B }), installed(WEATHER_ID, '0.1.0')]);
    expect(pins.get(CALENDAR_ID)).toBe(SOURCE_B);
    expect(pins.get(WEATHER_ID)).toBeUndefined();
  });

  test('unrecognized provenance never resolves to a subscribed source', () => {
    const pins = pinsFrom([installed(CALENDAR_ID, '0.1.0', { provenance: 'not a url at all' })]);
    expect(pins.get(CALENDAR_ID)).not.toBe(SOURCE_A);
    expect(pins.get(CALENDAR_ID)).not.toBe(SOURCE_B);
  });

  test('a device that predates provenance degrades to first subscribed source', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [installed(CALENDAR_ID, '0.1.5')],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    const cal = listings.find(l => l.app.id === CALENDAR_ID)!;
    expect(cal.sourceUrl).toBe(SOURCE_A);
  });
});

describe('version ordering', () => {
  test('newest is by released_at not array order', () => {
    const a = app(CALENDAR_ID, 'Calendar', [
      ver('0.1.0', { released: '2026-01-01T00:00:00Z' }),
      ver('0.9.0', { released: '2026-06-01T00:00:00Z' }),
    ]);
    expect(newestCompatible(a, 'v0.4.1')?.version).toBe('0.9.0');
  });

  test('non utc offsets compare as instants', () => {
    const a = app(CALENDAR_ID, 'Calendar', [
      ver('0.2.0', { released: '2026-06-01T00:00:00+02:00' }),
      ver('0.1.0', { released: '2026-06-01T00:00:00Z' }),
    ]);
    expect(newestCompatible(a, 'v0.4.1')?.version).toBe('0.1.0');
  });

  test('unparseable timestamps sort last', () => {
    const a = app(CALENDAR_ID, 'Calendar', [
      ver('0.1.0', { released: 'whenever' }),
      ver('0.3.0', { released: '2026-02-01T00:00:00Z' }),
    ]);
    expect(newestCompatible(a, 'v0.4.1')?.version).toBe('0.3.0');
  });

  test('any single version can be checked, not just the newest', () => {
    const older = ver('0.1.0');
    const gated = ver('0.9.0', { minLib: '99.0.0' });

    expect(versionCompatible(older, 'v0.4.1')).toBe(true);
    expect(versionCompatible(gated, 'v0.4.1')).toBe(false);
    expect(versionCompatible(gated, null)).toBe(true);
  });

  test('compat filter applies after sorting', () => {
    const a = app(CALENDAR_ID, 'Calendar', [
      ver('0.1.0', { released: '2026-01-01T00:00:00Z' }),
      ver('0.9.0', { minLib: '99.0.0', released: '2026-06-01T00:00:00Z' }),
      ver('0.5.0', { released: '2026-03-01T00:00:00Z' }),
    ]);
    expect(newestCompatible(a, 'v0.4.1')?.version).toBe('0.5.0');
  });
});

describe('listedWebapps', () => {
  test('the builtin launcher is hidden and every other builtin stays', () => {
    const list = [
      installed('hub', '0.1.0', { source: 'builtin', role: 'launcher' }),
      installed('browser', '0.1.0', { source: 'builtin' }),
      installed(CALENDAR_ID, '0.1.0'),
    ];

    expect(listedWebapps(list).map(w => w.id)).toEqual(['browser', CALENDAR_ID]);
  });

  test('an installed launcher is a real app the user chose, so it stays listed', () => {
    const list = [installed(CALENDAR_ID, '0.1.0', { role: 'launcher' })];

    expect(listedWebapps(list)).toHaveLength(1);
  });
});

describe('aggregate', () => {
  test('pinned source is primary and compat filters', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [installed(CALENDAR_ID, '0.1.5', { provenance: SOURCE_B })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    expect(listings).toHaveLength(2);
    const cal = listings.find(l => l.app.id === CALENDAR_ID)!;
    expect(cal.sourceUrl).toBe(SOURCE_B);
    expect(cal.newestCompatible?.version).toBe('0.1.5');
    expect(cal.installedVersion).toBe('0.1.5');
    expect(cal.updateAvailable).toBe(false);
    expect(cal.alsoAvailableFrom).toEqual([SOURCE_A]);

    const weather = listings.find(l => l.app.id === WEATHER_ID)!;
    expect(weather.installedVersion).toBeNull();
    expect(weather.newestCompatible?.version).toBe('0.1.0');
    expect(weather.alsoAvailableFrom).toEqual([]);
  });

  test('a newer listing than what is installed is an update', () => {
    const a = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.2.0')])]);
    const listings = aggregate({
      orderedCatalogs: [{ url: SOURCE_A, catalog: a }],
      installed: [installed(CALENDAR_ID, '0.1.0', { provenance: SOURCE_A })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    expect(listings[0]!.updateAvailable).toBe(true);
  });

  test('an older listing than what is installed is not an update', () => {
    const a = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.1.0', { released: '2026-06-01T00:00:00Z' })])]);
    const listings = aggregate({
      orderedCatalogs: [{ url: SOURCE_A, catalog: a }],
      installed: [installed(CALENDAR_ID, '0.2.0', { provenance: SOURCE_A })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    expect(listings[0]!.updateAvailable).toBe(false);
  });

  test('defaults to first source when unpinned', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    const cal = listings.find(l => l.app.id === CALENDAR_ID)!;
    expect(cal.sourceUrl).toBe(SOURCE_A);
    expect(cal.newestCompatible?.version).toBe('0.2.0');
    expect(cal.alsoAvailableFrom).toEqual([SOURCE_B]);
  });

  test('two catalogs spelling one id in different case offer one app, not two', () => {
    const a = catalog([app(CALENDAR_ID.toUpperCase(), 'Calendar', [ver('0.2.0')])]);
    const b = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.1.5')])]);
    const listings = aggregate({
      orderedCatalogs: [
        { url: SOURCE_A, catalog: a },
        { url: SOURCE_B, catalog: b },
      ],
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });

    expect(listings).toHaveLength(1);
    expect(listings[0]!.sourceUrl).toBe(SOURCE_A);
    expect(listings[0]!.alsoAvailableFrom).toEqual([SOURCE_B]);
  });

  test('a pin matches its source whatever case that catalog spells the id in', () => {
    const a = catalog([app(CALENDAR_ID.toUpperCase(), 'Calendar', [ver('0.2.0')])]);
    const b = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.1.5')])]);
    const listings = aggregate({
      orderedCatalogs: [
        { url: SOURCE_A, catalog: a },
        { url: SOURCE_B, catalog: b },
      ],
      installed: [installed(CALENDAR_ID, '0.1.5', { provenance: SOURCE_B })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });

    expect(listings).toHaveLength(1);
    expect(listings[0]!.sourceUrl).toBe(SOURCE_B);
    expect(listings[0]!.installedVersion).toBe('0.1.5');
  });

  test('no compatible version for an old device', () => {
    const a = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.3.0', { minLib: '99.0.0' })])]);
    const listings = aggregate({
      orderedCatalogs: [{ url: SOURCE_A, catalog: a }],
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    expect(listings[0]!.newestCompatible).toBeNull();
  });

  test('null device version lists newest', () => {
    const a = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.3.0', { minLib: '99.0.0' })])]);
    const listings = aggregate({
      orderedCatalogs: [{ url: SOURCE_A, catalog: a }],
      installed: [],
      deviceLibVersion: null,
      extensions: 'listed',
    });
    expect(listings[0]!.newestCompatible?.version).toBe('0.3.0');
  });

  test('a dead source never hides an installed app offered by a live one', () => {
    const a = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.2.0')])]);
    const listings = aggregate({
      orderedCatalogs: [{ url: SOURCE_A, catalog: a }],
      installed: [installed(CALENDAR_ID, '0.1.0', { provenance: SOURCE_B })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    const cal = listings.find(l => l.app.id === CALENDAR_ID)!;
    expect(cal.installedVersion).toBe('0.1.0');
    expect(cal.sourceUrl).toBe(SOURCE_A);
  });

  test('an app that ships a native extension is offered only where an extension can run', () => {
    const a = catalog([
      app(CALENDAR_ID, 'Calendar', [ver('0.2.0')]),
      app(WEATHER_ID, 'Stats', [{ ...ver('0.1.0'), extension: { desktop: true, permissions: ['all'] } }]),
    ]);
    const orderedCatalogs = [{ url: SOURCE_A, catalog: a }];

    const listed = aggregate({ orderedCatalogs, installed: [], deviceLibVersion: 'v0.4.1', extensions: 'listed' });
    expect(listed.map(l => l.app.id)).toEqual([CALENDAR_ID, WEATHER_ID]);

    const omitted = aggregate({
      orderedCatalogs,
      installed: [installed(WEATHER_ID, '0.0.9', { provenance: SOURCE_A })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'omitted',
    });
    expect(omitted.map(l => l.app.id)).toEqual([CALENDAR_ID]);
  });
});

describe('updates', () => {
  test('offers update only from the pinned source', () => {
    const a = catalog([
      app(CALENDAR_ID, 'Calendar', [ver('0.2.0'), ver('0.1.0', { released: '2026-04-01T00:00:00Z' })]),
    ]);
    const b = catalog([
      app(CALENDAR_ID, 'Calendar', [ver('0.3.0'), ver('0.1.0', { released: '2026-04-01T00:00:00Z' })]),
    ]);
    const catalogs = new Map([
      [SOURCE_A, a],
      [SOURCE_B, b],
    ]);

    const found = updates({
      catalogs,
      installed: [installed(CALENDAR_ID, '0.1.0', { provenance: SOURCE_A })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    expect(found).toHaveLength(1);
    expect(found[0]!.target.version).toBe('0.2.0');
    expect(found[0]!.sourceUrl).toBe(SOURCE_A);
    expect(found[0]!.installedVersion).toBe('0.1.0');
  });

  test('skips unpinned, builtin, and up to date', () => {
    const a = catalog([app(CALENDAR_ID, 'Calendar', [ver('0.2.0')])]);
    const catalogs = new Map([[SOURCE_A, a]]);

    expect(
      updates({
        catalogs,
        installed: [installed(CALENDAR_ID, '0.1.0')],
        deviceLibVersion: 'v0.4.1',
        extensions: 'listed',
      }),
    ).toHaveLength(0);
    expect(
      updates({
        catalogs,
        installed: [installed(CALENDAR_ID, '0.1.0', { source: 'builtin', provenance: SOURCE_A })],
        deviceLibVersion: 'v0.4.1',
        extensions: 'listed',
      }),
    ).toHaveLength(0);
    expect(
      updates({
        catalogs,
        installed: [installed(CALENDAR_ID, '0.2.0', { provenance: SOURCE_A })],
        deviceLibVersion: 'v0.4.1',
        extensions: 'listed',
      }),
    ).toHaveLength(0);
  });

  test('an installed app that ships a native extension is never updated from a host that cannot run one', () => {
    const a = catalog([
      app(CALENDAR_ID, 'Calendar', [{ ...ver('0.2.0'), extension: { desktop: true, permissions: ['all'] } }]),
    ]);
    const args = {
      catalogs: new Map([[SOURCE_A, a]]),
      installed: [installed(CALENDAR_ID, '0.1.0', { provenance: SOURCE_A })],
      deviceLibVersion: 'v0.4.1',
    };

    expect(updates({ ...args, extensions: 'listed' })).toHaveLength(1);
    expect(updates({ ...args, extensions: 'omitted' })).toHaveLength(0);
  });

  test('a dead pinned source offers nothing but does not throw', () => {
    const catalogs = new Map<string, Catalog>();
    expect(
      updates({
        catalogs,
        installed: [installed(CALENDAR_ID, '0.1.0', { provenance: SOURCE_B })],
        deviceLibVersion: 'v0.4.1',
        extensions: 'listed',
      }),
    ).toHaveLength(0);
  });

  test('an older version published later is not an update', () => {
    const a = catalog([
      app(CALENDAR_ID, 'Calendar', [
        ver('0.1.0', { released: '2026-06-01T00:00:00Z' }),
        ver('0.2.0', { released: '2026-05-01T00:00:00Z' }),
      ]),
    ]);
    const found = updates({
      catalogs: new Map([[SOURCE_A, a]]),
      installed: [installed(CALENDAR_ID, '0.2.0', { provenance: SOURCE_A })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    expect(found).toHaveLength(0);
  });

  test('matches a catalog id that differs only in case', () => {
    const a = catalog([app(CALENDAR_ID.toUpperCase(), 'Calendar', [ver('0.2.0')])]);
    const found = updates({
      catalogs: new Map([[SOURCE_A, a]]),
      installed: [installed(CALENDAR_ID, '0.1.0', { provenance: SOURCE_A })],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });
    expect(found).toHaveLength(1);
    expect(found[0]!.target.version).toBe('0.2.0');
  });
});

describe('popularity', () => {
  function counts(entries: [string, string, number][]) {
    return entries.map(([app_id, source_url, count]) => ({ app_id, source_url, count }));
  }

  test('the most installed app leads the listing, whatever its name', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
      installs: counts([[WEATHER_ID, SOURCE_B, 12]]),
    });

    expect(listings.map(l => l.app.id)).toEqual([WEATHER_ID, CALENDAR_ID]);
    expect(listings[0]!.installs).toBe(12);
  });

  test('an app nobody has installed sorts after every counted app', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
      installs: counts([[WEATHER_ID, SOURCE_B, 1]]),
    });

    expect(listings.map(l => l.app.id)).toEqual([WEATHER_ID, CALENDAR_ID]);
    expect(listings[1]!.installs).toBe(0);
  });

  test('installs from every source offering an app add up to one number', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
      installs: counts([
        [CALENDAR_ID, SOURCE_A, 3],
        [CALENDAR_ID, SOURCE_B, 4],
        [WEATHER_ID, SOURCE_B, 5],
      ]),
    });

    expect(listings.map(l => l.app.id)).toEqual([CALENDAR_ID, WEATHER_ID]);
    expect(listings[0]!.installs).toBe(7);
  });

  test('a count spelled in another case lands on the app it belongs to', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
      installs: counts([[WEATHER_ID.toUpperCase(), SOURCE_B, 9]]),
    });

    expect(listings.find(l => l.app.id === WEATHER_ID)!.installs).toBe(9);
  });

  test('a count for an app no source offers changes nothing', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
      installs: counts([['019e6701-13f8-71b5-ba04-0000000000ff', SOURCE_A, 400]]),
    });

    expect(listings.map(l => l.app.name)).toEqual(['Calendar', 'Weather']);
    expect(listings.every(l => l.installs === 0)).toBe(true);
  });

  test('a nonsense tally is ignored rather than sorted on', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
      installs: [
        { app_id: WEATHER_ID, source_url: SOURCE_B, count: Number.NaN },
        { app_id: CALENDAR_ID, source_url: SOURCE_A, count: -50 },
      ],
    });

    expect(listings.every(l => l.installs === 0)).toBe(true);
    expect(listings.map(l => l.app.name)).toEqual(['Calendar', 'Weather']);
  });

  test('equal counts fall back to name, so the order never wobbles between refreshes', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
      installs: counts([
        [CALENDAR_ID, SOURCE_A, 6],
        [WEATHER_ID, SOURCE_B, 6],
      ]),
    });

    expect(listings.map(l => l.app.name)).toEqual(['Calendar', 'Weather']);
  });

  test('a caller that knows no counts still gets the alphabetical listing it always had', () => {
    const listings = aggregate({
      orderedCatalogs: orderedCatalogs(),
      installed: [],
      deviceLibVersion: 'v0.4.1',
      extensions: 'listed',
    });

    expect(listings.map(l => l.app.name)).toEqual(['Calendar', 'Weather']);
    expect(listings.every(l => l.installs === 0)).toBe(true);
  });
});

describe('settingsOrigin', () => {
  const DIGEST = 'a'.repeat(64);
  const OTHER = 'b'.repeat(64);
  const hosted = { url: 'https://apps.example.com/s/x.html', size: 26909, sha256: DIGEST };

  function appWith(settings: Download | undefined, sha = DIGEST) {
    const v = {
      version: '1.0.0',
      released_at: '2026-08-01T00:00:00Z',
      download: { url: 'https://apps.example.com/r/x.zip', size: 1, sha256: sha },
      permissions: [],
      min_libbridgething_version: '0.5.0',
      changelog: null,
    } as AppEntry['versions'][number];
    if (settings) v.settings = settings;
    return {
      id: '019e6701-13f8-71b5-ba04-85d326630e98',
      name: 'x',
      description: 'x',
      author: 'x',
      icon: null,
      homepage: null,
      source: null,
      versions: [v],
    } as AppEntry;
  }

  test('offers the hosted copy when it hashes to what the device installed', () => {
    expect(settingsOrigin(appWith(hosted), DIGEST)).toEqual(hosted);
  });

  test('refuses a hosted copy whose bytes are not the installed ones', () => {
    expect(settingsOrigin(appWith(hosted), OTHER)).toBeNull();
  });

  test('is null when the source publishes no settings page', () => {
    expect(settingsOrigin(appWith(undefined), DIGEST)).toBeNull();
  });

  test('is null when the device reports no settings page', () => {
    expect(settingsOrigin(appWith(hosted), null)).toBeNull();
  });

  test('matches case-insensitively, since a digest is hex either way', () => {
    expect(settingsOrigin(appWith({ ...hosted, sha256: DIGEST.toUpperCase() }), DIGEST)).not.toBeNull();
  });
});

describe('settingsOriginFor', () => {
  const hosted: Download = { url: 'https://example.test/s/page.html', size: 240, sha256: 'c'.repeat(64) };

  function withSettings(): AppEntry {
    const version = ver('1.0.0');
    version.settings = hosted;
    return app(CALENDAR_ID, 'Fixture', [version]);
  }

  const sources = (apps: AppEntry[]) => [{ url: SOURCE_A, catalog: catalog(apps) }];

  test('an id that differs only in case still resolves', () => {
    expect(settingsOriginFor(sources([withSettings()]), SOURCE_A, CALENDAR_ID.toUpperCase(), hosted.sha256)).toEqual(
      hosted,
    );
  });

  test('a provenance no held catalog matches resolves to nothing', () => {
    expect(settingsOriginFor(sources([withSettings()]), SOURCE_B, CALENDAR_ID, hosted.sha256)).toBeNull();
  });

  test('a device hash the catalog does not publish resolves to nothing', () => {
    expect(settingsOriginFor(sources([withSettings()]), SOURCE_A, CALENDAR_ID, 'd'.repeat(64))).toBeNull();
  });

  test('an app the catalog does not carry resolves to nothing', () => {
    expect(settingsOriginFor(sources([withSettings()]), SOURCE_A, WEATHER_ID, hosted.sha256)).toBeNull();
  });

  test('no provenance resolves to nothing', () => {
    expect(settingsOriginFor(sources([withSettings()]), null, CALENDAR_ID, hosted.sha256)).toBeNull();
  });
});
