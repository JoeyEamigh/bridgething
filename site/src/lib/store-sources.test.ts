import { describe, expect, it } from 'bun:test';
import { aggregate, type AppEntry, type Catalog } from '@bridgething/catalog';
import type { MergedCatalog } from './directory-client.ts';
import { orderedByTrust, sourceMap, vouchedFor } from './store-sources.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';

function app(id: string, name: string): AppEntry {
  return {
    id,
    name,
    description: 'an app',
    author: 'someone',
    icon: null,
    homepage: null,
    source: null,
    versions: [
      {
        version: '1.0.0',
        released_at: '2026-05-31T00:00:00Z',
        download: { url: 'https://example.com/a.zip', size: 10, sha256: 'a'.repeat(64) },
        permissions: [],
        min_libbridgething_version: '0.4.0',
        changelog: null,
      },
    ],
  };
}

function merged(url: string, flags: { official?: boolean; attested?: boolean }, apps: AppEntry[]): MergedCatalog {
  const catalog: Catalog = {
    schema: 'catalog.v1',
    updated_at: '2026-05-31T00:00:00Z',
    repo: { name: url, description: 'a source', homepage: null, icon: null },
    apps,
    recommended_sources: [],
  };
  return { url, official: flags.official ?? false, attested: flags.attested ?? false, catalog };
}

const OFFICIAL = merged('https://official', { official: true, attested: true }, [app(CALENDAR_ID, 'Calendar')]);
const VOUCHED = merged('https://vouched', { attested: true }, [app('019e6701-13f8-71b5-ba04-000000000001', 'Vouched')]);
const COMMUNITY = merged('https://community', {}, [app('019e6701-13f8-71b5-ba04-000000000002', 'Community')]);

describe('vouchedFor', () => {
  it('covers official and attested sources only', () => {
    expect(vouchedFor(OFFICIAL)).toBe(true);
    expect(vouchedFor(VOUCHED)).toBe(true);
    expect(vouchedFor(COMMUNITY)).toBe(false);
  });
});

describe('orderedByTrust', () => {
  it('puts vouched-for sources first whatever order they arrive in', () => {
    const ordered = orderedByTrust([COMMUNITY, OFFICIAL, VOUCHED]);
    expect(ordered.map(entry => entry.url)).toEqual(['https://official', 'https://vouched', 'https://community']);
  });

  it('resolves an app offered by two sources to the vouched-for one', () => {
    const squatter = merged('https://community', {}, [app(CALENDAR_ID, 'Calendar')]);
    const listings = aggregate({
      orderedCatalogs: orderedByTrust([squatter, OFFICIAL]),
      installed: [],
      deviceLibVersion: null,
      extensions: 'listed',
    });

    const calendar = listings.find(listing => listing.app.id === CALENDAR_ID)!;
    expect(calendar.sourceUrl).toBe('https://official');
    expect(calendar.alsoAvailableFrom).toEqual(['https://community']);
  });
});

describe('sourceMap', () => {
  it('keys the display model by source url', () => {
    const sources = sourceMap([OFFICIAL, COMMUNITY]);
    expect(sources.get('https://official')).toMatchObject({ official: true, attested: true, name: 'https://official' });
    expect(sources.get('https://community')).toMatchObject({ official: false, attested: false });
  });
});
