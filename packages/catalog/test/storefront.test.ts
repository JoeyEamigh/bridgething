import { describe, expect, test } from 'bun:test';
import { appCategories } from '../src/categories.ts';
import type { CatalogAppListing } from '../src/resolve.ts';
import type { MergedCatalog } from '../src/sources.ts';
import { storeCatalog } from '../src/store.ts';
import { browse, featuredListing, moreFromAuthor, search, storefront } from '../src/storefront.ts';
import type { AppEntry, AppVersion, Catalog } from '../src/types.ts';

const OFFICIAL = 'https://apps.bridgething.com/catalog.json';
const ATTESTED = 'https://attested.example.com/catalog.json';
const LISTED = 'https://someone.example.com/catalog.json';
const PRIVATE = 'https://mine.example.com/catalog.json';

let seq = 0;
function id(): string {
  seq += 1;
  return `019e6701-13f8-71b5-ba04-${seq.toString(16).padStart(12, '0')}`;
}

function ver(version: string, released: string, extra: Partial<AppVersion> = {}): AppVersion {
  return {
    version,
    released_at: released,
    download: { url: `https://example.com/${version}.zip`, size: 1, sha256: '0'.repeat(64) },
    permissions: [],
    min_libbridgething_version: '0.4.0',
    changelog: null,
    ...extra,
  };
}

function app(name: string, extra: Partial<AppEntry> = {}): AppEntry {
  return {
    id: id(),
    name,
    description: `${name} for your car thing`,
    author: 'someone',
    icon: null,
    homepage: null,
    source: null,
    versions: [ver('1.0.0', '2026-09-01T00:00:00Z')],
    ...extra,
  };
}

function catalog(name: string, apps: AppEntry[]): Catalog {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-10-01T00:00:00Z',
    repo: { name, description: 'test', homepage: null, icon: null },
    apps,
    recommended_sources: [],
  };
}

function merged(url: string, apps: AppEntry[], flags: { official?: boolean; attested?: boolean } = {}): MergedCatalog {
  return { url, official: flags.official ?? false, attested: flags.attested ?? false, catalog: catalog(url, apps) };
}

function listing(entry: AppEntry, installs = 0, trending = 0, official = false): CatalogAppListing {
  return {
    app: entry,
    sourceUrl: official ? OFFICIAL : LISTED,
    newestCompatible: entry.versions[0]!,
    installedVersion: null,
    updateAvailable: false,
    alsoAvailableFrom: [],
    installs,
    trending,
    official,
  };
}

const NOW = new Date('2026-10-27T12:00:00Z');

describe('appCategories', () => {
  test('keeps known slugs in order, drops unknown ones, and caps at two', () => {
    const entry = app('Mixed', { categories: ['music', 'spaceships', 'music', 'ambient', 'games'] });
    expect(appCategories(entry)).toEqual(['music', 'ambient']);
  });

  test('an app without the field has no categories', () => {
    expect(appCategories(app('Plain'))).toEqual([]);
  });
});

describe('storeCatalog', () => {
  test('every directory app is a listing and trust comes from the directory tier', () => {
    const weather = app('Weather');
    const radio = app('Radio');
    const metro = app('Metro');
    const { listings, sources } = storeCatalog({
      subscribed: [],
      merged: [
        merged(OFFICIAL, [weather], { official: true }),
        merged(ATTESTED, [metro], { attested: true }),
        merged(LISTED, [radio]),
      ],
      installed: [],
      deviceLibVersion: null,
      installs: [],
      trending: [],
      categoryBackfill: {},
      extensions: 'listed',
    });

    expect(listings.map(l => l.app.name).sort()).toEqual(['Metro', 'Radio', 'Weather']);
    expect(sources[OFFICIAL]!.trust).toBe('official');
    expect(sources[ATTESTED]!.trust).toBe('attested');
    expect(sources[LISTED]!.trust).toBe('listed');
  });

  test('a subscribed source outside the directory is unlisted and wins a contested id', () => {
    const shared = app('Clock');
    const { listings, sources } = storeCatalog({
      subscribed: [{ url: PRIVATE, catalog: catalog('mine', [shared]) }],
      merged: [merged(OFFICIAL, [shared], { official: true })],
      installed: [],
      deviceLibVersion: null,
      installs: [],
      trending: [],
      categoryBackfill: {},
      extensions: 'listed',
    });

    expect(sources[PRIVATE]!.trust).toBe('unlisted');
    expect(listings).toHaveLength(1);
    expect(listings[0]!.sourceUrl).toBe(PRIVATE);
    expect(listings[0]!.alsoAvailableFrom).toEqual([OFFICIAL]);
  });

  test('a backfilled category fills in only for apps that declare none', () => {
    const untagged = app('Radio');
    const tagged = app('Clock', { categories: ['clocks-weather'] });
    const { listings } = storeCatalog({
      subscribed: [],
      merged: [merged(LISTED, [untagged, tagged])],
      installed: [],
      deviceLibVersion: null,
      installs: [],
      trending: [],
      categoryBackfill: { [untagged.id]: ['music'], [tagged.id]: ['games'] },
      extensions: 'listed',
    });

    const byName = Object.fromEntries(listings.map(l => [l.app.name, appCategories(l.app)]));
    expect(byName).toEqual({ Radio: ['music'], Clock: ['clocks-weather'] });
  });

  test('install and trending tallies land on the listing', () => {
    const weather = app('Weather');
    const { listings } = storeCatalog({
      subscribed: [],
      merged: [merged(OFFICIAL, [weather], { official: true })],
      installed: [],
      deviceLibVersion: null,
      installs: [{ app_id: weather.id, source_url: OFFICIAL, count: 12, versions: {} }],
      trending: [{ app_id: weather.id, source_url: OFFICIAL, count: 3 }],
      categoryBackfill: {},
      extensions: 'listed',
    });

    expect(listings[0]!.installs).toBe(12);
    expect(listings[0]!.trending).toBe(3);
  });
});

describe('browse', () => {
  const launcher = app('Metro', { versions: [ver('1.0.0', '2026-10-17T00:00:00Z', { role: 'launcher' })] });
  const overlay = app('Glassy', { versions: [ver('1.0.0', '2026-09-13T00:00:00Z', { provides_overlay: true })] });
  const radio = app('Radio', {
    categories: ['music'],
    versions: [ver('2.0.0', '2026-10-20T00:00:00Z'), ver('1.0.0', '2026-08-01T00:00:00Z')],
  });
  const clock = app('Clock', { categories: ['clocks-weather'], versions: [ver('1.0.0', '2026-10-25T00:00:00Z')] });
  const all = [listing(launcher, 14, 0), listing(overlay, 42, 5), listing(radio, 21, 2), listing(clock, 3, 0)];

  test('popular orders by installs', () => {
    expect(browse(all, { filter: null, sort: 'popular' }).map(l => l.app.name)).toEqual([
      'Glassy',
      'Radio',
      'Metro',
      'Clock',
    ]);
  });

  test('trending keeps only apps with recent installs', () => {
    expect(browse(all, { filter: null, sort: 'trending' }).map(l => l.app.name)).toEqual(['Glassy', 'Radio']);
  });

  test('new orders by first release and updated needs a second version', () => {
    expect(browse(all, { filter: null, sort: 'new' }).map(l => l.app.name)).toEqual([
      'Clock',
      'Metro',
      'Glassy',
      'Radio',
    ]);
    expect(browse(all, { filter: null, sort: 'updated' }).map(l => l.app.name)).toEqual(['Radio']);
  });

  test('filters by category, kind and author', () => {
    expect(
      browse(all, { filter: { kind: 'category', category: 'music' }, sort: 'popular' }).map(l => l.app.name),
    ).toEqual(['Radio']);
    expect(browse(all, { filter: { kind: 'launchers' }, sort: 'popular' }).map(l => l.app.name)).toEqual(['Metro']);
    expect(browse(all, { filter: { kind: 'overlays' }, sort: 'popular' }).map(l => l.app.name)).toEqual(['Glassy']);
    expect(browse(all, { filter: { kind: 'author', author: ' SOMEONE ' }, sort: 'popular' })).toHaveLength(4);
  });

  test('more from the author leaves the app itself out', () => {
    expect(moreFromAuthor(all, all[0]!).map(l => l.app.name)).toEqual(['Glassy', 'Radio', 'Clock']);
  });
});

describe('featuredListing', () => {
  const shot = { screenshots: ['https://example.com/1.png'] };
  const a = listing(app('A', shot), 10, 9);
  const b = listing(app('B', shot), 50, 1);
  const bare = listing(app('Bare'), 500, 50);

  test('a pin wins over the rotation', () => {
    expect(featuredListing([a, b, bare], { app_id: bare.app.id, source_url: LISTED }, NOW)).toBe(bare);
  });

  test('a pin for an app no source lists falls back to the rotation', () => {
    expect(featuredListing([a, b], { app_id: id(), source_url: OFFICIAL }, NOW)).not.toBeNull();
  });

  test('rotation only features apps with a screenshot, and is stable within a day', () => {
    const first = featuredListing([a, b, bare], null, NOW);
    expect(first === a || first === b).toBe(true);
    expect(featuredListing([a, b, bare], null, new Date('2026-10-27T23:59:00Z'))).toBe(first);
    expect(featuredListing([a, b, bare], null, new Date('2026-10-28T00:01:00Z'))).not.toBe(first);
  });

  test('nothing is featured when no app has a screenshot', () => {
    expect(featuredListing([bare], null, NOW)).toBeNull();
  });
});

describe('official apps', () => {
  const shot = { screenshots: ['https://example.com/1.png'] };
  const weather = listing(app('Weather', { ...shot, categories: ['clocks-weather'] }), 125, 40, true);
  const thing = listing(app('WeatherThing', { ...shot, categories: ['clocks-weather'] }), 73, 2);
  const radio = listing(app('Radio', { ...shot, categories: ['music'] }), 21, 1);
  const all = [weather, thing, radio];

  test('the image preinstalls them, so their counts never rank them', () => {
    expect(browse(all, { filter: null, sort: 'popular' }).map(l => l.app.name)).toEqual([
      'WeatherThing',
      'Radio',
      'Weather',
    ]);
    expect(browse(all, { filter: null, sort: 'trending' }).map(l => l.app.name)).toEqual(['WeatherThing', 'Radio']);
  });

  test('they stay off the popular shelf and out of essentials and the featured rotation', () => {
    const front = storefront({ listings: all, pin: null, now: NOW });
    expect(front.shelves.find(shelf => shelf.id === 'popular')?.listings.map(l => l.app.name)).toEqual([
      'WeatherThing',
      'Radio',
    ]);
    expect(front.featured?.official).toBe(false);
  });

  test('a pin can still feature one', () => {
    expect(featuredListing(all, { app_id: weather.app.id, source_url: OFFICIAL }, NOW)).toBe(weather);
  });

  test('they still show up by category and in search', () => {
    expect(browse(all, { filter: { kind: 'category', category: 'clocks-weather' }, sort: 'popular' })).toContain(
      weather,
    );
    expect(search(all, 'weather')[0]).toBe(weather);
  });
});

describe('storefront', () => {
  test('essentials picks the top app of each category and kind, and needs four to show', () => {
    const picks = [
      listing(app('Metro', { versions: [ver('1.0.0', '2026-10-17T00:00:00Z', { role: 'launcher' })] }), 14),
      listing(app('Flow', { versions: [ver('1.0.0', '2026-10-17T00:00:00Z', { role: 'launcher' })] }), 30),
      listing(app('Glassy', { versions: [ver('1.0.0', '2026-09-13T00:00:00Z', { provides_overlay: true })] }), 42),
      listing(app('Radio', { categories: ['music'] }), 21),
      listing(app('Cassette', { categories: ['music'] }), 5),
      listing(app('Clock', { categories: ['clocks-weather'] }), 3),
    ];
    const front = storefront({ listings: picks, pin: null, now: NOW });
    const essentials = front.shelves.find(shelf => shelf.id === 'essentials');
    expect(essentials?.listings.map(l => l.app.name)).toEqual(['Glassy', 'Flow', 'Radio', 'Clock']);

    const thin = storefront({ listings: picks.slice(0, 3), pin: null, now: NOW });
    expect(thin.shelves.some(shelf => shelf.id === 'essentials')).toBe(false);
  });

  test('empty shelves are dropped and category chips only count categories in use', () => {
    const front = storefront({
      listings: [listing(app('Radio', { categories: ['music'] }), 1), listing(app('Plain'), 2)],
      pin: null,
      now: NOW,
    });
    expect(front.shelves.map(shelf => shelf.id)).toEqual(['popular', 'new']);
    expect(front.categories).toEqual([{ category: 'music', label: 'music', count: 1 }]);
  });
});

describe('search', () => {
  const weather = listing(app('Weather', { categories: ['clocks-weather'], author: 'JoeyEamigh' }), 125);
  const weatherThing = listing(app('WeatherThing', { categories: ['clocks-weather'], author: 'Naga' }), 73);
  const radio = listing(app('Radio Atlas', { categories: ['music'], description: 'live world radio' }), 21);
  const metro = listing(app('Metro', { versions: [ver('1.0.0', '2026-10-17T00:00:00Z', { role: 'launcher' })] }), 14);
  const all = [radio, weatherThing, weather, metro];

  test('an exact name beats a prefix, then installs break ties', () => {
    expect(search(all, 'weather').map(l => l.app.name)).toEqual(['Weather', 'WeatherThing']);
  });

  test('every token must match somewhere', () => {
    expect(search(all, 'world radio').map(l => l.app.name)).toEqual(['Radio Atlas']);
    expect(search(all, 'world clock')).toEqual([]);
  });

  test('matches authors, category labels and kinds', () => {
    expect(search(all, 'naga').map(l => l.app.name)).toEqual(['WeatherThing']);
    expect(search(all, 'music').map(l => l.app.name)).toEqual(['Radio Atlas']);
    expect(search(all, 'home screen').map(l => l.app.name)).toEqual(['Metro']);
  });

  test('a blank query matches nothing', () => {
    expect(search(all, '   ')).toEqual([]);
  });
});
