#!/usr/bin/env bun
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { parse, stringify } from 'yaml';

type PublishedApp = {
  slug: string;
  id: string;
  name: string;
  description: string;
  version: string;
  permissions: string[];
  role?: 'standard' | 'launcher';
  provides_overlay?: boolean;
  extension?: { entry?: string; permissions?: string[]; api?: number } | null;
  icon: string | null;
  download: { url: string; size: number; sha256: string };
  settings?: { url: string; size: number; sha256: string } | null;
};

type VersionRow = { version: string; released_at: string; [k: string]: unknown };
type AppRow = { slug: string; versions: VersionRow[]; [k: string]: unknown };

const DEFAULT_MIN_LIB = '0.12.0';

function applyBundleTraits(row: VersionRow, app: PublishedApp): void {
  if (app.role === 'launcher') row['role'] = 'launcher';
  else delete row['role'];
  if (app.provides_overlay) row['provides_overlay'] = true;
  else delete row['provides_overlay'];
  if (app.extension) row['extension'] = { desktop: true, permissions: app.extension.permissions ?? [] };
  else delete row['extension'];
  if (app.settings) row['settings'] = app.settings;
  else delete row['settings'];
}

function parseArgs(argv: string[]): { payload: string; statePath: string; releasedAt: string } {
  const out: Record<string, string> = {};
  for (let i = 0; i < argv.length; i++) {
    const next = argv[i + 1];
    if (!next) continue;
    if (argv[i] === '--payload') out.payload = next;
    if (argv[i] === '--state-path') out.statePath = next;
    if (argv[i] === '--released-at') out.releasedAt = next;
  }
  if (!out.payload) throw new Error('missing --payload');
  return {
    payload: out.payload,
    statePath: out.statePath ?? resolve(import.meta.dirname, '..', 'apps', 'apps-published.yaml'),
    releasedAt: out.releasedAt ?? new Date().toISOString().replace(/\.\d{3}Z$/, 'Z'),
  };
}

const args = parseArgs(process.argv.slice(2));

const payload = JSON.parse(await readFile(args.payload, 'utf-8')) as { apps?: PublishedApp[] };
const published = payload.apps ?? [];
if (published.length === 0) {
  console.log('payload contains no apps; nothing to apply');
  process.exit(0);
}

const doc = parse(await readFile(args.statePath, 'utf-8')) as { apps?: AppRow[] };
const apps = doc.apps ?? [];

for (const app of published) {
  for (const field of ['slug', 'id', 'name', 'description', 'version'] as const) {
    if (!app[field]) throw new Error(`payload entry is missing "${field}": ${JSON.stringify(app)}`);
  }

  let row = apps.find(a => a.slug === app.slug);
  if (!row) {
    row = { slug: app.slug, versions: [] } as AppRow;
    apps.push(row);
    console.log(`added new app "${app.slug}"; give it a curation row in apps/apps.yaml`);
  }

  row['id'] = app.id;
  row['name'] = app.name;
  row['description'] = app.description;
  if (app.icon) row['icon'] = app.icon;
  else row['icon'] ??= null;

  const existing = row.versions.find(v => v.version === app.version);
  if (existing) {
    existing['download'] = app.download;
    existing['permissions'] = app.permissions;
    applyBundleTraits(existing, app);
    console.log(`rewrote ${app.slug}@${app.version} artifact metadata`);
  } else {
    const added: VersionRow = {
      version: app.version,
      released_at: args.releasedAt,
      download: app.download,
      permissions: app.permissions,
      min_libbridgething_version: DEFAULT_MIN_LIB,
      changelog: null,
    };
    applyBundleTraits(added, app);
    row.versions.push(added);
    console.log(`added ${app.slug}@${app.version}`);
  }

  row.versions.sort((a, b) => b.released_at.localeCompare(a.released_at));
}

await writeFile(args.statePath, stringify({ ...doc, apps }, { indent: 2 }));
console.log(`applied ${published.length} published webapp(s) to ${args.statePath}`);
