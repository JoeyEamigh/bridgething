import { renderToString } from 'preact-render-to-string';
import { AppDetail } from '../src/components/store/AppDetail.tsx';
import { SourcesPage } from '../src/components/store/SourcesPage.tsx';
import { StoreBrowse } from '../src/components/store/StoreBrowse.tsx';
import { Storefront } from '../src/components/store/Storefront.tsx';
import type { StoreRoute } from '../src/lib/app-routes.ts';
import type { StoreData } from '../src/lib/directory-client.ts';

export const STORE_DATA_ID = 'store-data';

const ISLAND_END = '<!--astro:end-->';

function island(route: StoreRoute, href: string, data: StoreData): { component: string; html: string } {
  switch (route.page) {
    case 'front':
      return { component: 'Storefront', html: renderToString(<Storefront data={data} />) };
    case 'browse':
      return { component: 'StoreBrowse', html: renderToString(<StoreBrowse data={data} href={href} />) };
    case 'app':
      return { component: 'AppDetail', html: renderToString(<AppDetail baked={null} data={data} href={href} />) };
    case 'sources':
      return { component: 'SourcesPage', html: renderToString(<SourcesPage data={data} />) };
  }
}

export function injectStoreData(asset: Response, route: StoreRoute, href: string, data: string): Response {
  const response = new Response(asset.body, asset);
  response.headers.set('cache-control', 'public, max-age=0, s-maxage=60');
  const rendered = island(route, href, JSON.parse(data) as StoreData);

  return new HTMLRewriter()
    .on(`script#${STORE_DATA_ID}`, {
      element(element) {
        element.setInnerContent(data.replace(/</g, '\\u003c'), { html: true });
      },
    })
    .on(`astro-island[component-export="${rendered.component}"]`, {
      element(element) {
        element.setInnerContent(rendered.html + ISLAND_END, { html: true });
      },
    })
    .transform(response);
}
