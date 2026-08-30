import { readFile } from 'node:fs/promises';
import { parse as parseYaml } from 'yaml';
import {
  sortNewestFirst,
  validate,
  type AppEntry,
  type Catalog,
  type RecommendedSource,
  type Repo,
} from '@bridgething/catalog';
import type { AppConfigEntry, CatalogCuration, PublishedState } from './config.ts';

export type GenerateInput = {
  repo: Repo;
  recommendedSources: RecommendedSource[];
  apps: AppConfigEntry[];
  updatedAt: string;
};

export function generate(input: GenerateInput): Catalog {
  const apps: AppEntry[] = [];
  for (const appConfig of input.apps) {
    for (const field of ['id', 'name', 'description'] as const) {
      if (!appConfig[field]) {
        throw new Error(`app "${appConfig.slug}" is missing "${field}"; the publish dispatch fills these in`);
      }
    }
    if (appConfig.versions.length === 0) {
      throw new Error(`app "${appConfig.slug}" has no versions`);
    }

    const versions = sortNewestFirst(
      appConfig.versions.map(v => ({
        version: v.version,
        released_at: v.released_at,
        download: v.download,
        permissions: v.permissions,
        ...(v.role === 'launcher' ? { role: v.role } : {}),
        ...(v.provides_overlay ? { provides_overlay: true } : {}),
        ...(v.extension ? { extension: { desktop: true as const, permissions: v.extension.permissions } } : {}),
        min_libbridgething_version: v.min_libbridgething_version,
        changelog: v.changelog ?? null,
      })),
    );

    apps.push({
      id: appConfig.id,
      name: appConfig.name,
      description: appConfig.description,
      author: appConfig.author,
      icon: appConfig.icon,
      ...(appConfig.screenshots && appConfig.screenshots.length > 0 ? { screenshots: appConfig.screenshots } : {}),
      homepage: appConfig.homepage ?? null,
      source: appConfig.source ?? null,
      versions,
    });
  }

  const catalog: Catalog = {
    $schema: 'https://apps.bridgething.com/schemas/catalog/v1.json',
    schema: 'catalog.v1',
    updated_at: input.updatedAt,
    repo: input.repo,
    apps,
    recommended_sources: input.recommendedSources,
  };

  return validate(catalog);
}

export function stringify(catalog: Catalog): string {
  return JSON.stringify(catalog, null, 2) + '\n';
}

const DEFAULT_AUTHOR = 'JoeyEamigh';
const DEFAULT_HOMEPAGE = 'https://bridgething.com/apps';
const DEFAULT_SOURCE = 'https://github.com/JoeyEamigh/bridgething';

function override<T>(curated: T | undefined, fallback: T): T {
  return curated === undefined ? fallback : curated;
}

export function mergeApps(curation: CatalogCuration, state: PublishedState): AppConfigEntry[] {
  const published = state.apps ?? [];
  const curated = curation.apps ?? [];

  for (const row of curated) {
    if (!published.some(app => app.slug === row.slug)) {
      console.warn(`curation lists "${row.slug}", which nothing has published; check the slug`);
    }
  }

  return published.map(app => {
    const row = curated.find(a => a.slug === app.slug);
    return {
      slug: app.slug,
      id: app.id,
      name: override(row?.name, app.name),
      description: override(row?.description, app.description),
      icon: override(row?.icon, app.icon),
      ...(row?.screenshots ? { screenshots: row.screenshots } : {}),
      author: override(row?.author, DEFAULT_AUTHOR),
      homepage: override(row?.homepage, DEFAULT_HOMEPAGE),
      source: override(row?.source, DEFAULT_SOURCE),
      versions: app.versions,
    };
  });
}

export async function loadCuration(path: string): Promise<CatalogCuration> {
  const parsed = parseYaml(await readFile(path, 'utf-8')) as CatalogCuration;
  if (!parsed?.repo) throw new Error(`${path}: missing "repo" section`);
  if (parsed.apps !== undefined && !Array.isArray(parsed.apps)) throw new Error(`${path}: "apps" must be an array`);
  return parsed;
}

export async function loadPublishedState(path: string): Promise<PublishedState> {
  const parsed = parseYaml(await readFile(path, 'utf-8')) as PublishedState;
  if (parsed?.apps !== undefined && !Array.isArray(parsed.apps)) throw new Error(`${path}: "apps" must be an array`);
  return parsed ?? {};
}

export type { AppEntry, Catalog } from '@bridgething/catalog';
