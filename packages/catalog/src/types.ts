export type Catalog = {
  $schema?: string;
  schema: 'catalog.v1';
  updated_at: string;
  repo: Repo;
  apps: AppEntry[];
  recommended_sources: RecommendedSource[];
};

export type Repo = {
  name: string;
  description: string;
  homepage: string | null;
  icon: string | null;
};

export type AppEntry = {
  id: string;
  name: string;
  description: string;
  author: string;
  icon: string | null;
  screenshots?: string[];
  homepage: string | null;
  source: string | null;
  versions: AppVersion[];
};

export type AppVersion = {
  version: string;
  released_at: string;
  download: Download;
  permissions: string[];
  role?: 'standard' | 'launcher';
  provides_overlay?: boolean;
  extension?: AppExtension;
  min_libbridgething_version: string;
  changelog: string | null;
};

export type AppExtension = {
  desktop: true;
  permissions: string[];
};

export type Download = {
  url: string;
  size: number;
  sha256: string;
};

export type RecommendedSource = {
  name: string;
  url: string;
  description: string | null;
  attested: boolean;
};

export type SourceCatalog = { url: string; catalog: Catalog };

export type InstallCount = { app_id: string; source_url: string; count: number };
