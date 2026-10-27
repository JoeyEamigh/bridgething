import type { BrowseQuery } from '@bridgething/catalog';
import { describe, expect, test } from 'bun:test';

import { browseQuery, PATHS, sectionFor } from './routes.ts';

function roundTrip(query: BrowseQuery): BrowseQuery {
  const url = new URL(PATHS.storeBrowse(query), 'http://desktop');
  return browseQuery(Object.fromEntries(url.searchParams));
}

describe('store browse paths', () => {
  test('every filter shape survives the url', () => {
    const queries: BrowseQuery[] = [
      { filter: null, sort: 'trending' },
      { filter: { kind: 'category', category: 'clocks-weather' }, sort: 'new' },
      { filter: { kind: 'launchers' }, sort: 'updated' },
      { filter: { kind: 'overlays' }, sort: 'popular' },
      { filter: { kind: 'author', author: 'Ousa Chea & co/2' }, sort: 'popular' },
    ];
    for (const query of queries) expect(roundTrip(query)).toEqual(query);
  });

  test('unknown values fall back to every app by popularity', () => {
    expect(browseQuery({ sort: 'loudest', category: 'spaceships', kind: 'widgets' })).toEqual({
      filter: null,
      sort: 'popular',
    });
  });

  test('browse lives under the store section', () => {
    expect(sectionFor(new URL(PATHS.storeBrowse({ filter: null, sort: 'new' }), 'http://desktop').pathname)).toBe(
      PATHS.store,
    );
  });
});
