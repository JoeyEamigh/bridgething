import { useEffect, useMemo, useState } from 'preact/hooks';
import { storeCatalog, type StoreCatalog } from '@bridgething/catalog';
import { fetchDirectory, fetchMergedApps, readStoreData, type StoreData } from './directory-client';

export type Store = {
  data: StoreData | null;
  catalog: StoreCatalog | null;
  failed: boolean;
  seeded: boolean;
};

export function catalogOf(data: StoreData): StoreCatalog {
  return storeCatalog({
    subscribed: [],
    merged: data.apps.catalogs,
    installed: [],
    deviceLibVersion: null,
    installs: data.apps.installs,
    trending: data.apps.trending,
    categoryBackfill: data.apps.category_backfill,
    extensions: 'listed',
  });
}

export function currentHref(): string | null {
  return typeof window === 'undefined' ? null : `${window.location.pathname}${window.location.search}`;
}

export function useStore(initial?: StoreData | null): Store {
  const [seeded] = useState(() => initial ?? readStoreData());
  const [data, setData] = useState<StoreData | null>(seeded);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (seeded) return;

    const controller = new AbortController();
    Promise.all([
      fetchMergedApps({ origin: '', signal: controller.signal }),
      fetchDirectory({ signal: controller.signal }).catch(() => []),
    ])
      .then(([apps, directory]) => setData({ apps, directory }))
      .catch(() => {
        if (!controller.signal.aborted) setFailed(true);
      });
    return () => controller.abort();
  }, [seeded]);

  const catalog = useMemo(() => (data ? catalogOf(data) : null), [data]);

  return { data, catalog, failed, seeded: seeded !== null };
}
