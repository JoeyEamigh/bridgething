export type IconBody = { kind: 'svg'; xml: string } | { kind: 'raster' };

export type IconHit = { body: IconBody; fresh: boolean };

export type IconStorage = {
  getString(key: string): string | undefined;
  set(key: string, value: string): void;
  remove(key: string): boolean | void;
};

export const ICON_CACHE_LIMIT = 256;
export const ICON_FRESH_MS = 7 * 24 * 60 * 60 * 1000;

const INDEX_KEY = 'index';
const entryKey = (url: string) => `icon:${url}`;

type Entry = IconBody & { at: number };

const RASTER_PATH = /\.(png|jpe?g|webp|gif)$/i;

export function rasterByPath(url: string): boolean {
  try {
    return RASTER_PATH.test(new URL(url).pathname);
  } catch {
    return false;
  }
}

export function iconStore(
  storage: IconStorage,
  limit = ICON_CACHE_LIMIT,
  now: () => number = Date.now,
) {
  let index: string[] | null = null;

  const order = (): string[] => {
    if (index) return index;
    try {
      const parsed: unknown = JSON.parse(storage.getString(INDEX_KEY) ?? '[]');
      index = Array.isArray(parsed)
        ? parsed.filter((url): url is string => typeof url === 'string')
        : [];
    } catch {
      index = [];
    }
    return index;
  };

  return {
    get(url: string): IconHit | null {
      const raw = storage.getString(entryKey(url));
      if (!raw) return null;
      try {
        const { at, ...body } = JSON.parse(raw) as Entry;
        return { body, fresh: now() - at < ICON_FRESH_MS };
      } catch {
        storage.remove(entryKey(url));
        return null;
      }
    },
    set(url: string, body: IconBody): void {
      const next = order().filter(held => held !== url);
      next.push(url);
      while (next.length > limit) storage.remove(entryKey(next.shift()!));
      index = next;
      storage.set(entryKey(url), JSON.stringify({ ...body, at: now() }));
      storage.set(INDEX_KEY, JSON.stringify(next));
    },
  };
}
