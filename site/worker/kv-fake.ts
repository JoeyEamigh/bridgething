import type { KvLike } from './store.ts';

export type FakeKv = KvLike & {
  snapshot(): Record<string, string>;
  counts: { get: number; put: number; list: number };
  resetCounts(): void;
};

export function fakeKv(seed: Record<string, string> = {}): FakeKv {
  const data = new Map<string, string>(Object.entries(seed));
  const counts = { get: 0, put: 0, list: 0 };

  return {
    counts,
    async get(key) {
      counts.get += 1;
      return data.get(key) ?? null;
    },
    async put(key, value) {
      counts.put += 1;
      data.set(key, value);
    },
    async delete(key) {
      data.delete(key);
    },
    async list({ prefix = '' } = {}) {
      counts.list += 1;
      const keys = [...data.keys()]
        .filter(name => name.startsWith(prefix))
        .sort()
        .map(name => ({ name }));
      return { keys, list_complete: true };
    },
    snapshot() {
      return Object.fromEntries(data);
    },
    resetCounts() {
      counts.get = 0;
      counts.put = 0;
      counts.list = 0;
    },
  };
}

export function withListLag(kv: FakeKv): FakeKv {
  const lagging = new Set<string>();

  return {
    ...kv,
    async put(key, value, options) {
      if ((await kv.get(key)) === null) lagging.add(key);
      return kv.put(key, value, options);
    },
    async list(options) {
      const page = await kv.list(options);
      return { ...page, keys: page.keys.filter(({ name }) => !lagging.has(name)) };
    },
  };
}

export function withRivalWriter(
  kv: FakeKv,
  key: string,
  items: unknown[],
  wins = Number.POSITIVE_INFINITY,
): FakeKv & { rivals: number } {
  const wrapper = {
    ...kv,
    rivals: 0,
    async put(name: string, value: string, options?: { expirationTtl?: number }) {
      await kv.put(name, value, options);
      if (name !== key || wrapper.rivals >= wins) return;
      wrapper.rivals += 1;
      const held = JSON.parse(value) as { version: number };
      await kv.put(name, JSON.stringify({ version: held.version + 1, writer: `rival-${wrapper.rivals}`, items }));
    },
  };

  return wrapper;
}

export function withRefusedPut(kv: FakeKv, key: string): FakeKv {
  return {
    ...kv,
    async put(name, value, options) {
      if (name === key) throw new Error('kv put rate limit');
      return kv.put(name, value, options);
    },
  };
}

export function withRefusedWrites(kv: FakeKv, key: string): FakeKv {
  const refused = withRefusedPut(kv, key);

  return {
    ...refused,
    async delete(name) {
      if (name === key) throw new Error('kv delete rate limit');
      return refused.delete(name);
    },
  };
}
