import { describe, expect, test } from 'bun:test';
import { blendStoreListings } from '../src/blend.ts';
import type { InstalledWebapp } from '../src/resolve.ts';
import type { MergedCatalog } from '../src/sources.ts';
import type { AppEntry, AppVersion, Catalog, InstallCount, SourceCatalog } from '../src/types.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';
const WEATHER_ID = '019e6701-13f8-71b5-ba04-81f347137de2';
const RADIO_ID = '019e6701-13f8-71b5-ba04-7c1120aa0e11';
const OFFICIAL = 'https://apps.bridgething.com/catalog.json';
const ATTESTED = 'https://attested.example.com/catalog.json';
const COMMUNITY = 'https://someone.example.com/catalog.json';

function ver(version: string, minLib = '0.4.0'): AppVersion {
  return {
    version,
    released_at: '2026-05-31T00:00:00Z',
    download: { url: `https://apps.bridgething.com/r/${version}.zip`, size: 1, sha256: '0'.repeat(64) },
    permissions: [],
    min_libbridgething_version: minLib,
    changelog: null,
  };
}

function app(id: string, name: string, versions: AppVersion[]): AppEntry {
  return { id, name, description: 'test', author: 'JoeyEamigh', icon: null, homepage: null, source: null, versions };
}

function catalog(name: string, apps: AppEntry[]): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-05-31T00:00:00Z',
    repo: { name, description: 'test', homepage: null, icon: null },
    apps,
    recommended_sources: [],
  };
}

function merged(url: string, name: string, apps: AppEntry[], flags: { official?: boolean; attested?: boolean } = {}) {
  return {
    url,
    official: flags.official ?? false,
    attested: flags.attested ?? false,
    catalog: catalog(name, apps),
  } satisfies MergedCatalog;
}

function blend(args: {
  catalogs?: SourceCatalog[];
  installed?: InstalledWebapp[];
  installs?: InstallCount[];
  subscribed?: string[];
  merged?: MergedCatalog[];
}) {
  return blendStoreListings({
    catalogs: args.catalogs ?? [
      { url: OFFICIAL, catalog: catalog('the bridgething catalog', [app(CALENDAR_ID, 'Calendar', [ver('0.2.0')])]) },
    ],
    merged: args.merged ?? [
      merged(OFFICIAL, 'the bridgething catalog', [app(CALENDAR_ID, 'Calendar', [ver('0.2.0')])], { official: true }),
      merged(ATTESTED, 'attested repo', [app(WEATHER_ID, 'Weather', [ver('0.1.0')])], { attested: true }),
      merged(COMMUNITY, 'someone repo', [app(RADIO_ID, 'Radio', [ver('0.1.0')])]),
    ],
    installed: args.installed ?? [],
    deviceLibVersion: 'v0.4.1',
    extensions: 'listed',
    installs: args.installs ?? [],
    subscribed: args.subscribed ?? [OFFICIAL],
  });
}

describe('blendStoreListings', () => {
  test('official and attested directory sources are vouched, everything else is community', () => {
    const { vouched, community } = blend({});

    expect(vouched.map(l => l.app.id)).toEqual([CALENDAR_ID, WEATHER_ID]);
    expect(community.map(l => l.app.id)).toEqual([RADIO_ID]);
  });

  test('a subscribed source is vouched even when the directory calls it community', () => {
    const { vouched, community } = blend({
      subscribed: [OFFICIAL, COMMUNITY],
      catalogs: [
        { url: OFFICIAL, catalog: catalog('the bridgething catalog', [app(CALENDAR_ID, 'Calendar', [ver('0.2.0')])]) },
        { url: COMMUNITY, catalog: catalog('someone repo', [app(RADIO_ID, 'Radio', [ver('0.1.0')])]) },
      ],
    });

    expect(vouched.map(l => l.app.id).sort()).toEqual([CALENDAR_ID, RADIO_ID, WEATHER_ID].sort());
    expect(community).toHaveLength(0);
  });

  test('a subscribed catalog outranks the directory copy of the same app', () => {
    const { vouched } = blend({
      merged: [
        merged(OFFICIAL, 'the bridgething catalog', [app(CALENDAR_ID, 'Calendar', [ver('0.2.0')])], { official: true }),
        merged(COMMUNITY, 'someone repo', [app(CALENDAR_ID, 'Calendar', [ver('0.9.0')])]),
      ],
    });

    expect(vouched).toHaveLength(1);
    expect(vouched[0]!.sourceUrl).toBe(OFFICIAL);
    expect(vouched[0]!.alsoAvailableFrom).toEqual([COMMUNITY]);
  });

  test('install counts rank each section, ties fall back to name', () => {
    const { vouched } = blend({
      installs: [
        { app_id: WEATHER_ID, source_url: ATTESTED, count: 40 },
        { app_id: CALENDAR_ID, source_url: OFFICIAL, count: 2 },
      ],
    });

    expect(vouched.map(l => l.app.id)).toEqual([WEATHER_ID, CALENDAR_ID]);
    expect(vouched[0]!.installs).toBe(40);
  });

  test('installed state carries into the blended listings', () => {
    const { community } = blend({
      installed: [{ id: RADIO_ID, version: '0.0.9', source: 'installed', role: 'standard', provenance: COMMUNITY }],
    });

    expect(community[0]!.installedVersion).toBe('0.0.9');
    expect(community[0]!.updateAvailable).toBe(true);
  });

  test('source names cover both the subscribed catalogs and the directory extras', () => {
    const { sourceNames } = blend({});

    expect(sourceNames[OFFICIAL]).toBe('the bridgething catalog');
    expect(sourceNames[ATTESTED]).toBe('attested repo');
    expect(sourceNames[COMMUNITY]).toBe('someone repo');
  });

  test('an empty directory answer leaves the subscribed catalogs alone', () => {
    const { vouched, community } = blend({ merged: [] });

    expect(vouched.map(l => l.app.id)).toEqual([CALENDAR_ID]);
    expect(community).toHaveLength(0);
  });

  test('a host that omits extensions sees neither section list an app that ships one', () => {
    const withExtension = (id: string, name: string) =>
      app(id, name, [{ ...ver('0.1.0'), extension: { desktop: true, permissions: ['all'] } }]);
    const merged_ = [
      merged(OFFICIAL, 'the bridgething catalog', [withExtension(CALENDAR_ID, 'Calendar')], { official: true }),
      merged(COMMUNITY, 'someone repo', [withExtension(RADIO_ID, 'Radio')]),
    ];
    const catalogs = [
      { url: OFFICIAL, catalog: catalog('the bridgething catalog', [withExtension(CALENDAR_ID, 'Calendar')]) },
    ];

    const listed = blend({ catalogs, merged: merged_ });
    expect(listed.vouched.map(l => l.app.id)).toEqual([CALENDAR_ID]);
    expect(listed.community.map(l => l.app.id)).toEqual([RADIO_ID]);

    const omitted = blendStoreListings({
      catalogs,
      merged: merged_,
      installed: [],
      deviceLibVersion: 'v0.4.1',
      installs: [],
      subscribed: [OFFICIAL],
      extensions: 'omitted',
    });
    expect(omitted.vouched).toHaveLength(0);
    expect(omitted.community).toHaveLength(0);
    expect(omitted.sourceNames[COMMUNITY]).toBe('someone repo');
  });
});
