import { describe, expect, it } from 'bun:test';
import type { BrowseQuery } from '@bridgething/catalog';
import {
  APP_DETAIL_SHELL,
  appDetailPath,
  appIdFromPath,
  AUTHOR_SHELL,
  browsePath,
  searchPath,
  shellFor,
  storeRoute,
  viewPath,
  type BrowseView,
} from './app-routes.ts';

const CALENDAR_ID = '019e6701-13f8-71b5-ba04-85d326630e98';

function routeOf(href: string) {
  const url = new URL(href, 'https://bridgething.com');
  return storeRoute(url.pathname, url.search);
}

describe('appIdFromPath', () => {
  it('reads the id out of an app detail path', () => {
    expect(appIdFromPath(`/apps/${CALENDAR_ID}`)).toBe(CALENDAR_ID);
    expect(appIdFromPath(`/apps/${CALENDAR_ID}/`)).toBe(CALENDAR_ID);
    expect(appIdFromPath(`/apps/${CALENDAR_ID.toUpperCase()}`)).toBe(CALENDAR_ID);
  });

  it('ignores everything that is not an app detail path', () => {
    expect(appIdFromPath('/apps')).toBeNull();
    expect(appIdFromPath('/apps/')).toBeNull();
    expect(appIdFromPath(APP_DETAIL_SHELL)).toBeNull();
    expect(appIdFromPath('/apps/not-a-uuid')).toBeNull();
    expect(appIdFromPath(`/apps/${CALENDAR_ID}/versions`)).toBeNull();
    expect(appIdFromPath(`/docs/${CALENDAR_ID}`)).toBeNull();
  });

  it('round-trips with appDetailPath', () => {
    expect(appIdFromPath(appDetailPath(CALENDAR_ID))).toBe(CALENDAR_ID);
  });
});

describe('storeRoute', () => {
  it('knows the store front and the sources page', () => {
    expect(storeRoute('/apps')).toEqual({ page: 'front' });
    expect(storeRoute('/apps/')).toEqual({ page: 'front' });
    expect(storeRoute('/apps/sources/')).toEqual({ page: 'sources' });
  });

  it('reads sorts, kinds, categories and authors, defaulting to popular', () => {
    expect(routeOf('/apps/new')).toEqual({
      page: 'browse',
      view: { kind: 'query', query: { filter: null, sort: 'new' } },
    });
    expect(routeOf('/apps/home-screens')).toEqual({
      page: 'browse',
      view: { kind: 'query', query: { filter: { kind: 'launchers' }, sort: 'popular' } },
    });
    expect(routeOf('/apps/c/music?sort=trending')).toEqual({
      page: 'browse',
      view: { kind: 'query', query: { filter: { kind: 'category', category: 'music' }, sort: 'trending' } },
    });
    expect(routeOf('/apps/by/Ousa%20Chea/')).toEqual({
      page: 'browse',
      view: { kind: 'query', query: { filter: { kind: 'author', author: 'Ousa Chea' }, sort: 'popular' } },
    });
  });

  it('reads a search and its optional sort', () => {
    expect(routeOf('/apps/search?q=%20weather%20')).toEqual({
      page: 'browse',
      view: { kind: 'search', text: 'weather', sort: null },
    });
    expect(routeOf('/apps/search?q=x&sort=bogus')).toEqual({
      page: 'browse',
      view: { kind: 'search', text: 'x', sort: null },
    });
  });

  it('ignores a sort it does not know rather than failing the page', () => {
    expect(routeOf('/apps/overlays?sort=cheapest')).toEqual({
      page: 'browse',
      view: { kind: 'query', query: { filter: { kind: 'overlays' }, sort: 'popular' } },
    });
  });

  it('leaves everything else alone', () => {
    expect(storeRoute('/apps/c/not-a-category')).toBeNull();
    expect(storeRoute('/apps/c')).toBeNull();
    expect(storeRoute(AUTHOR_SHELL)).toBeNull();
    expect(storeRoute('/apps/by/%E0%A4%A')).toBeNull();
    expect(storeRoute(APP_DETAIL_SHELL)).toBeNull();
    expect(storeRoute('/apps/store')).toBeNull();
    expect(storeRoute('/device')).toBeNull();
    expect(storeRoute('/apps/c/music/extra')).toBeNull();
  });
});

describe('paths', () => {
  const queries: BrowseQuery[] = [
    { filter: null, sort: 'popular' },
    { filter: null, sort: 'updated' },
    { filter: { kind: 'category', category: 'clocks-weather' }, sort: 'popular' },
    { filter: { kind: 'category', category: 'games' }, sort: 'new' },
    { filter: { kind: 'launchers' }, sort: 'trending' },
    { filter: { kind: 'overlays' }, sort: 'popular' },
    { filter: { kind: 'author', author: 'Ousa Chea & co/ltd' }, sort: 'updated' },
  ];

  it('round-trips every browse query through its url', () => {
    for (const query of queries) {
      const view: BrowseView = { kind: 'query', query };
      expect(routeOf(browsePath(query))).toEqual({ page: 'browse', view });
      expect(viewPath(view)).toBe(browsePath(query));
    }
  });

  it('keeps the default sort out of the url', () => {
    expect(browsePath({ filter: { kind: 'category', category: 'music' }, sort: 'popular' })).toBe('/apps/c/music');
    expect(browsePath({ filter: { kind: 'launchers' }, sort: 'new' })).toBe('/apps/home-screens?sort=new');
  });

  it('round-trips a search', () => {
    expect(searchPath('')).toBe('/apps/search');
    expect(routeOf(searchPath('home & away', 'new'))).toEqual({
      page: 'browse',
      view: { kind: 'search', text: 'home & away', sort: 'new' },
    });
  });
});

describe('shellFor', () => {
  it('serves app ids and authors from their shells and nothing else', () => {
    expect(shellFor(`/apps/${CALENDAR_ID}`)).toBe(APP_DETAIL_SHELL);
    expect(shellFor('/apps/by/someone')).toBe(AUTHOR_SHELL);
    expect(shellFor('/apps/c/music')).toBeNull();
    expect(shellFor('/apps/c/nope')).toBeNull();
    expect(shellFor('/apps/popular')).toBeNull();
  });
});
