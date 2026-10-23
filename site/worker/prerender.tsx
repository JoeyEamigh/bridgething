import { renderToString } from 'preact-render-to-string';
import { AppDetail } from '../src/components/store/AppDetail.tsx';
import { StoreBrowser } from '../src/components/store/StoreBrowser.tsx';
import { appIdFromPath } from '../src/lib/app-routes.ts';
import type { StoreData } from '../src/lib/directory-client.ts';

export const STORE_DATA_ID = 'store-data';

const STORE_ROUTES = ['/apps', '/apps/'];

const ISLAND_END = '<!--astro:end-->';

export function wantsStoreData(pathname: string): boolean {
  return STORE_ROUTES.includes(pathname) || appIdFromPath(pathname) !== null;
}

function island(pathname: string, data: StoreData): { selector: string; html: string } {
  if (STORE_ROUTES.includes(pathname)) {
    return {
      selector: 'astro-island[component-export="StoreBrowser"]',
      html: renderToString(<StoreBrowser initial={[]} data={data} />),
    };
  }
  return {
    selector: 'astro-island[component-export="AppDetail"]',
    html: renderToString(<AppDetail baked={null} data={data} path={pathname} />),
  };
}

export function injectStoreData(asset: Response, pathname: string, data: string): Response {
  const response = new Response(asset.body, asset);
  response.headers.set('cache-control', 'public, max-age=0, s-maxage=60');
  const rendered = island(pathname, JSON.parse(data) as StoreData);

  return new HTMLRewriter()
    .on(`script#${STORE_DATA_ID}`, {
      element(element) {
        element.setInnerContent(data.replace(/</g, '\\u003c'), { html: true });
      },
    })
    .on(rendered.selector, {
      element(element) {
        element.setInnerContent(rendered.html + ISLAND_END, { html: true });
      },
    })
    .transform(response);
}
