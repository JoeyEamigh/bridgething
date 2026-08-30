import type { KvLike } from './store.ts';

export type Env = {
  SOURCES: KVNamespace;
  ASSETS: Fetcher;
  ADMIN_TOKEN: string;
};

export function kvOf(env: Env): KvLike {
  return env.SOURCES as unknown as KvLike;
}
