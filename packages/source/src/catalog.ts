import { CatalogValidationError, validate as validateCatalog, type Catalog } from '@bridgething/catalog';
import { existsSync } from 'node:fs';
import { copyFile, mkdir, readdir, rename } from 'node:fs/promises';
import { basename, extname, join } from 'node:path';
import {
  bundle,
  readChangelog,
  readJson,
  readSource,
  writeJson,
  writeText,
  type App,
  type Ledger,
  type LedgerVersion,
  type SourceConfig,
} from './lib.ts';
import { CATALOG_FILE, fail, LEDGER_FILE } from './paths.ts';

export interface StageResult {
  catalog: Catalog;
  ledger: Ledger;
  added: { slug: string; version: string }[];
}

export async function readLedger(site: string): Promise<Ledger> {
  const path = join(site, LEDGER_FILE);
  if (!existsSync(path)) return {};
  return readJson<Ledger>(path);
}

export async function stage(site: string, apps: App[], ledger: Ledger, base: string): Promise<StageResult> {
  const source = await readSource();
  const added: { slug: string; version: string }[] = [];

  for (const app of apps) {
    const published = (ledger[app.manifest.id] ??= {});
    if (published[app.manifest.version]) {
      console.log(`${app.slug} ${app.manifest.version} already published, skipping`);
      continue;
    }

    console.log(`::group::${app.slug} ${app.manifest.version}`);
    const file = `r/${app.manifest.id}/${app.manifest.version}.zip`;
    await mkdir(join(site, 'r', app.manifest.id), { recursive: true });
    const built = await bundle(app, join(site, `${app.slug}.staging.zip`));
    await rename(built.zip, join(site, file));

    if (built.iconPath && built.iconExt) {
      await mkdir(join(site, 'icons'), { recursive: true });
      await copyFile(built.iconPath, join(site, 'icons', `${app.manifest.id}.${built.iconExt}`));
    }
    await copyScreenshots(app, site);

    let settings: LedgerVersion['settings'];
    if (built.settingsPath && built.settingsSha256) {
      const settingsFile = `s/${app.manifest.id}/${app.manifest.version}.html`;
      await mkdir(join(site, 's', app.manifest.id), { recursive: true });
      await copyFile(built.settingsPath, join(site, settingsFile));
      settings = { file: settingsFile, size: built.settingsSize, sha256: built.settingsSha256 };
    }

    published[app.manifest.version] = {
      released_at: new Date().toISOString(),
      sha256: built.sha256,
      size: built.size,
      file,
      ...(settings ? { settings } : {}),
    };
    added.push({ slug: app.slug, version: app.manifest.version });
    console.log('::endgroup::');
  }

  const catalog = await settleTimestamp(await generate(source, apps, ledger, base, site), site);
  validate(catalog);

  await writeJson(join(site, CATALOG_FILE), catalog);
  await writeJson(join(site, LEDGER_FILE), ledger);
  await writeText(join(site, '.nojekyll'), '');
  await writeText(join(site, 'index.html'), landingPage(source, catalog, base));

  return { catalog, ledger, added };
}

async function settleTimestamp(catalog: Catalog, site: string): Promise<Catalog> {
  const path = join(site, CATALOG_FILE);
  if (!existsSync(path)) return catalog;
  const previous = await readJson<Catalog>(path);
  const same = JSON.stringify({ ...previous, updated_at: null }) === JSON.stringify({ ...catalog, updated_at: null });
  return same ? { ...catalog, updated_at: previous.updated_at } : catalog;
}

async function copyScreenshots(app: App, site: string): Promise<void> {
  const dir = join(app.dir, 'screenshots');
  if (!existsSync(dir)) return;
  const shots = (await readdir(dir)).filter(f => !f.startsWith('.')).sort();
  if (!shots.length) return;
  const out = join(site, 'screenshots', app.manifest.id);
  await mkdir(out, { recursive: true });
  for (const shot of shots) await copyFile(join(dir, shot), join(out, shot));
}

async function screenshotUrls(app: App, site: string, base: string): Promise<string[] | undefined> {
  if (app.meta.screenshots.length) return app.meta.screenshots;
  const dir = join(site, 'screenshots', app.manifest.id);
  if (!existsSync(dir)) return undefined;
  const shots = (await readdir(dir)).filter(f => !f.startsWith('.')).sort();
  if (!shots.length) return undefined;
  return shots.slice(0, 6).map(shot => `${base}/screenshots/${app.manifest.id}/${shot}`);
}

export async function generate(
  source: SourceConfig,
  apps: App[],
  ledger: Ledger,
  base: string,
  site: string,
): Promise<Catalog> {
  const entries: Catalog['apps'] = [];

  for (const app of apps) {
    const published = ledger[app.manifest.id] ?? {};
    const versions = Object.entries(published);
    if (!versions.length) continue;

    const changelog = await readChangelog(app);
    const icon = app.meta.icon ?? (await hostedIcon(app, site, base));
    const screenshots = await screenshotUrls(app, site, base);

    entries.push({
      id: app.manifest.id,
      name: app.manifest.name,
      description: app.manifest.description,
      author: app.meta.author,
      icon,
      ...(screenshots ? { screenshots } : {}),
      homepage: app.meta.homepage,
      source: app.meta.source,
      versions: versions
        .sort(([, a], [, b]) => b.released_at.localeCompare(a.released_at))
        .map(([version, row]) => versionEntry(app, version, row, changelog, base)),
    });
  }

  return {
    $schema: 'https://apps.bridgething.com/schemas/catalog/v1.json',
    schema: 'catalog.v1',
    updated_at: new Date().toISOString(),
    repo: {
      name: source.name,
      description: source.description,
      homepage: source.homepage ?? base,
      icon: source.icon,
    },
    apps: entries,
    recommended_sources: source.recommended_sources,
  };
}

function versionEntry(
  app: App,
  version: string,
  row: LedgerVersion,
  changelog: Record<string, string>,
  base: string,
): Catalog['apps'][number]['versions'][number] {
  const manifest = app.manifest;
  return {
    version,
    released_at: row.released_at,
    download: { url: `${base}/${row.file}`, size: row.size, sha256: row.sha256 },
    ...(row.settings
      ? { settings: { url: `${base}/${row.settings.file}`, size: row.settings.size, sha256: row.settings.sha256 } }
      : {}),
    permissions: manifest.permissions ?? [],
    ...(manifest.role === 'launcher' ? { role: 'launcher' as const } : {}),
    ...(manifest.overlay ? { provides_overlay: true } : {}),
    ...(manifest.extension
      ? { extension: { desktop: true as const, permissions: manifest.extension.permissions } }
      : {}),
    min_libbridgething_version: app.meta.min_libbridgething_version,
    changelog: changelog[version] ?? null,
  };
}

async function hostedIcon(app: App, site: string, base: string): Promise<string | null> {
  const dir = join(site, 'icons');
  if (!existsSync(dir)) return null;
  const match = (await readdir(dir)).find(f => basename(f, extname(f)) === app.manifest.id);
  return match ? `${base}/icons/${match}` : null;
}

export function validate(catalog: unknown): Catalog {
  try {
    return validateCatalog(catalog);
  } catch (err) {
    if (err instanceof CatalogValidationError)
      fail(`the generated catalog is not valid:\n  - ${err.errors.join('\n  - ')}`);
    throw err;
  }
}

function landingPage(source: SourceConfig, catalog: Catalog, base: string): string {
  const url = `${base}/${CATALOG_FILE}`;
  const rows = catalog.apps
    .map(app => {
      const newest = app.versions[0]!;
      const icon = app.icon ? `<img src="${app.icon}" alt="" width="48" height="48">` : '<div class="noicon"></div>';
      return `<li>${icon}<div><strong>${escape(app.name)}</strong> <span>${escape(newest.version)}</span><p>${escape(app.description)}</p></div></li>`;
    })
    .join('\n');

  return `<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escape(source.name)}</title>
<style>
  :root { color-scheme: dark; }
  body { margin: 0 auto; padding: 3rem 1.5rem; max-width: 44rem; background: #0b0b0d; color: #e8e8ea;
         font: 16px/1.6 ui-sans-serif, system-ui, sans-serif; }
  h1 { margin: 0 0 .25rem; font-size: 1.75rem; }
  p.lede { margin: 0 0 2rem; color: #a0a0a8; }
  code { display: block; padding: .85rem 1rem; border-radius: .5rem; background: #17171b; color: #9ecbff;
         overflow-x: auto; font-size: .9rem; }
  ul { list-style: none; margin: 2rem 0 0; padding: 0; }
  li { display: flex; gap: 1rem; align-items: flex-start; padding: 1rem 0; border-top: 1px solid #232329; }
  li img, .noicon { border-radius: .6rem; background: #17171b; flex: none; width: 48px; height: 48px; }
  li span { color: #6e6e78; font-size: .85rem; }
  li p { margin: .2rem 0 0; color: #a0a0a8; font-size: .95rem; }
  footer { margin-top: 3rem; color: #6e6e78; font-size: .85rem; }
  a { color: #9ecbff; }
</style>
<h1>${escape(source.name)}</h1>
<p class="lede">${escape(source.description)}</p>
<p>Add this url as a source in the bridgething companion app:</p>
<code>${url}</code>
<ul>
${rows || '<li><div><p>No apps published yet.</p></div></li>'}
</ul>
<footer>a <a href="https://bridgething.com">bridgething</a> app source</footer>
</html>
`;
}

function escape(value: string): string {
  return value.replace(
    /[&<>"]/g,
    c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c as '&' | '<' | '>' | '"'],
  );
}
