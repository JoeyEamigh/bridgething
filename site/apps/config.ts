import type { AppExtension, Download, RecommendedSource, Repo } from '@bridgething/catalog';

export type CatalogCuration = {
  repo: Repo;
  apps?: AppCurationEntry[];
};

export type AppCurationEntry = {
  slug: string;
  author?: string;
  name?: string;
  description?: string;
  icon?: string | null;
  screenshots?: string[];
  homepage?: string | null;
  source?: string | null;
};

export type PublishedState = {
  recommended_sources?: RecommendedSource[];
  apps?: PublishedAppEntry[];
};

export type PublishedAppEntry = {
  slug: string;
  id: string;
  name: string;
  description: string;
  icon: string | null;
  versions: AppVersionConfig[];
};

export type AppConfigEntry = {
  slug: string;
  id: string;
  name: string;
  description: string;
  author: string;
  icon: string | null;
  screenshots?: string[];
  homepage?: string | null;
  source?: string | null;
  versions: AppVersionConfig[];
};

export type AppVersionConfig = {
  version: string;
  released_at: string;
  download: Download;
  settings?: Download | null;
  permissions: string[];
  role?: 'standard' | 'launcher';
  provides_overlay?: boolean;
  extension?: AppExtension;
  min_libbridgething_version: string;
  changelog?: string | null;
};
