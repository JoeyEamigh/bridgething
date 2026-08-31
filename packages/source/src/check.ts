import { existsSync } from 'node:fs';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { generate, validate } from './catalog.ts';
import {
  bundle,
  capture,
  githubRemote,
  listApps,
  publicBase,
  readJson,
  readSource,
  run,
  type App,
  type Ledger,
  type SourceConfig,
} from './lib.ts';
import { fail, LEDGER_FILE, SOURCE_FILE } from './paths.ts';

async function publishedLedger(base: string): Promise<Ledger> {
  try {
    const response = await fetch(`${base}/${LEDGER_FILE}`, { redirect: 'follow' });
    if (!response.ok) return {};
    return (await response.json()) as Ledger;
  } catch {
    return {};
  }
}

function changedApps(baseRef: string): Set<string> | null {
  const diff = capture('git', ['diff', '--name-only', `${baseRef}...HEAD`]);
  if (diff === null) return null;
  const changed = new Set<string>();
  for (const path of diff.split('\n')) {
    const match = /^apps\/([^/]+)\//.exec(path);
    if (match) changed.add(match[1]!);
  }
  return changed;
}

function assertOwnBase(source: SourceConfig): void {
  const pages = source.base_url && /^https:\/\/([^.]+)\.github\.io\/([^/]+)\/?$/i.exec(source.base_url);
  if (!pages) return;
  const origin = githubRemote();
  if (!origin) return;
  const [owner, repo] = origin.split('/') as [string, string];
  if (pages[1]!.toLowerCase() === owner.toLowerCase() && pages[2]!.toLowerCase() === repo.toLowerCase()) return;
  fail(
    `${SOURCE_FILE} publishes to ${source.base_url}, which is ${pages[1]}/${pages[2]}, but origin is ${origin}.\n` +
      '       If you forked this source, point "base_url" at your own pages site and give every app in apps/ a\n' +
      '       fresh uuid.',
  );
}

async function typecheck(app: App): Promise<void> {
  if (!existsSync(join(app.dir, 'tsconfig.json'))) {
    fail(`apps/${app.slug} has no tsconfig.json`);
  }
  const pkg = await readJson<{ scripts?: Record<string, string> }>(join(app.dir, 'package.json'));
  if (pkg.scripts?.typecheck) run('bun', ['run', 'typecheck'], app.dir);
  else run('bunx', ['tsc', '--noEmit'], app.dir);
}

export async function check(): Promise<void> {
  const source = await readSource();
  assertOwnBase(source);

  const apps = await listApps();
  if (!apps.length) {
    console.log('apps/ is empty. run "bun run new <slug>" to add an app.');
    return;
  }

  for (const app of apps)
    if (!app.manifest.description.trim()) fail(`apps/${app.slug}/public/manifest.json has an empty "description"`);

  const base = publicBase(source);
  const published = await publishedLedger(base);

  const baseRef = process.env.CHECK_BASE_REF || null;
  const changed = baseRef ? changedApps(baseRef) : null;

  const staging = await mkdtemp(join(tmpdir(), 'bridgething-check-'));
  const ledger: Ledger = structuredClone(published);
  try {
    for (const app of apps) {
      console.log(`\n=== ${app.slug} ${app.manifest.version}`);
      await typecheck(app);

      const already = published[app.manifest.id]?.[app.manifest.version];
      if (already && changed?.has(app.slug)) {
        fail(
          `apps/${app.slug} changed but still says ${app.manifest.version}, which is already published.\n` +
            `       run \`bun run bump ${app.slug} <version>\``,
        );
      }

      const built = await bundle(app, join(staging, `${app.slug}.zip`));
      console.log(`  bundle ok: ${(built.size / 1024).toFixed(0)} KiB, sha256 ${built.sha256.slice(0, 12)}`);

      (ledger[app.manifest.id] ??= {})[app.manifest.version] ??= {
        released_at: new Date().toISOString(),
        sha256: built.sha256,
        size: built.size,
        file: `r/${app.manifest.id}/${app.manifest.version}.zip`,
      };
    }

    validate(await generate(source, apps, ledger, base, staging));
    console.log(`\nall ${apps.length} app(s) build, bundle, and produce a valid catalog.v1 against ${base}`);
  } finally {
    await rm(staging, { recursive: true, force: true });
  }
}
