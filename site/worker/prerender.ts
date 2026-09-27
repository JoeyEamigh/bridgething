import { appIdFromPath } from '../src/lib/app-routes.ts';
import { mergedApps, type MergedApps } from './apps.ts';
import { toDirectoryView, type DirectoryEntry } from './directory.ts';
import { listSources, type KvLike } from './store.ts';

export const STORE_DATA_ID = 'store-data';

const STORE_ROUTES = ['/apps', '/apps/'];

export function wantsStoreData(pathname: string): boolean {
  return STORE_ROUTES.includes(pathname) || appIdFromPath(pathname) !== null;
}

export type StoreData = { apps: MergedApps; directory: DirectoryEntry[] };

export async function storeData(args: { kv: KvLike; now: string; fetchImpl?: typeof fetch }): Promise<StoreData> {
  const { kv, now, fetchImpl } = args;
  const [apps, records] = await Promise.all([mergedApps({ kv, now, fetchImpl }), listSources(kv)]);
  return { apps, directory: toDirectoryView(records) };
}

export function injectStoreData(asset: Response, data: StoreData): Response {
  const response = new Response(asset.body, asset);
  response.headers.set('cache-control', 'public, max-age=0, s-maxage=300');

  return new HTMLRewriter()
    .on(`script#${STORE_DATA_ID}`, {
      element(element) {
        element.setInnerContent(JSON.stringify(data).replace(/</g, '\\u003c'), { html: true });
      },
    })
    .transform(response);
}
