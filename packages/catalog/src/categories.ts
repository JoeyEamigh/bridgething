import type { AppEntry } from './types.ts';

export const CATEGORIES = [
  'music',
  'clocks-weather',
  'dashboards',
  'home-control',
  'productivity',
  'ambient',
  'games',
  'travel',
  'tools',
] as const;

export type Category = (typeof CATEGORIES)[number];

export const MAX_CATEGORIES = 2;

export const CATEGORY_LABELS: Record<Category, string> = {
  music: 'music',
  'clocks-weather': 'clocks & weather',
  dashboards: 'dashboards',
  'home-control': 'home & control',
  productivity: 'productivity',
  ambient: 'ambient',
  games: 'games',
  travel: 'travel',
  tools: 'tools',
};

const KNOWN = new Set<string>(CATEGORIES);

export function isCategory(slug: string): slug is Category {
  return KNOWN.has(slug);
}

export function appCategories(app: AppEntry): Category[] {
  const out: Category[] = [];
  for (const slug of app.categories ?? []) {
    if (isCategory(slug) && !out.includes(slug)) out.push(slug);
    if (out.length === MAX_CATEGORIES) break;
  }
  return out;
}
