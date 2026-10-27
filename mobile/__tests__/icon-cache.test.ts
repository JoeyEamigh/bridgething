import {
  ICON_FRESH_MS,
  iconStore,
  rasterByPath,
  type IconStorage,
} from '../lib/icon-cache';

function disk(): IconStorage & { keys(): string[] } {
  const held = new Map<string, string>();
  return {
    getString: key => held.get(key),
    set: (key, value) => void held.set(key, value),
    remove: key => held.delete(key),
    keys: () => [...held.keys()],
  };
}

const SVG = { kind: 'svg' as const, xml: '<svg/>' };

describe('the store icon cache', () => {
  test('an icon fetched on one launch is there on the next without a fetch', () => {
    const storage = disk();
    iconStore(storage).set('https://a.test/i.svg', SVG);

    expect(iconStore(storage).get('https://a.test/i.svg')).toEqual({
      body: SVG,
      fresh: true,
    });
  });

  test('it never holds more icons than its limit, dropping the oldest first', () => {
    const storage = disk();
    const icons = iconStore(storage, 2);
    icons.set('https://a.test/1.svg', SVG);
    icons.set('https://a.test/2.svg', SVG);
    icons.set('https://a.test/3.svg', SVG);

    const reopened = iconStore(storage, 2);
    expect(reopened.get('https://a.test/1.svg')).toBeNull();
    expect(reopened.get('https://a.test/3.svg')).not.toBeNull();
    expect(storage.keys().filter(key => key.startsWith('icon:'))).toHaveLength(
      2,
    );
  });

  test('refetching an icon refreshes its place instead of evicting it', () => {
    const storage = disk();
    const icons = iconStore(storage, 2);
    icons.set('https://a.test/1.svg', SVG);
    icons.set('https://a.test/2.svg', SVG);
    icons.set('https://a.test/1.svg', SVG);
    icons.set('https://a.test/3.svg', SVG);

    expect(icons.get('https://a.test/1.svg')).not.toBeNull();
    expect(icons.get('https://a.test/2.svg')).toBeNull();
  });

  test('an old icon still paints but asks to be revalidated', () => {
    const storage = disk();
    let now = 1_000;
    const icons = iconStore(storage, 8, () => now);
    icons.set('https://a.test/i.svg', SVG);
    now += ICON_FRESH_MS + 1;

    expect(icons.get('https://a.test/i.svg')).toEqual({
      body: SVG,
      fresh: false,
    });
  });

  test('a corrupt entry reads as a miss', () => {
    const storage = disk();
    storage.set('icon:https://a.test/i.svg', '{not json');

    expect(iconStore(storage).get('https://a.test/i.svg')).toBeNull();
  });

  test('a raster extension needs no sniffing fetch, whatever the query string', () => {
    expect(rasterByPath('https://a.test/icons/x.PNG?v=2')).toBe(true);
    expect(rasterByPath('https://a.test/icons/x.jpeg')).toBe(true);
    expect(rasterByPath('https://a.test/icons/x.svg')).toBe(false);
    expect(rasterByPath('https://a.test/icons/x')).toBe(false);
  });
});
