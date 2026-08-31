import { mkdir, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { readLedger, stage } from './catalog.ts';
import { capture, listApps, publicBase, readSource, run } from './lib.ts';
import { fail, siteDir, sourceRoot } from './paths.ts';

const BRANCH = process.env.SITE_BRANCH ?? 'gh-pages';

function siteEnv(): { cwd: string; env: Record<string, string> } {
  return {
    cwd: siteDir(),
    env: {
      GIT_DIR: join(sourceRoot(), '.git'),
      GIT_WORK_TREE: siteDir(),
      GIT_INDEX_FILE: join(tmpdir(), 'bridgething-site.index'),
    },
  };
}

async function checkoutSite(): Promise<boolean> {
  const site = siteEnv();
  await rm(site.cwd, { recursive: true, force: true });
  await rm(site.env.GIT_INDEX_FILE, { force: true });
  await mkdir(site.cwd, { recursive: true });

  if (capture('git', ['ls-remote', '--exit-code', '--heads', 'origin', BRANCH]) === null) {
    console.log(`origin has no ${BRANCH} branch yet`);
    return false;
  }

  run('git', ['fetch', '--depth', '1', 'origin', BRANCH]);
  run('git', ['read-tree', 'FETCH_HEAD'], site);
  run('git', ['checkout-index', '--all', '--force'], site);
  return true;
}

function pushSite(message: string, hasParent: boolean): void {
  const site = siteEnv();
  run('git', ['add', '--all'], site);

  const tree = capture('git', ['write-tree'], site);
  if (!tree) fail('could not write a tree for the site branch');

  if (hasParent && tree === capture('git', ['rev-parse', 'FETCH_HEAD^{tree}'])) {
    console.log('the site branch is already up to date');
    return;
  }

  const parent = hasParent ? capture('git', ['rev-parse', 'FETCH_HEAD']) : null;
  const commit = capture('git', ['commit-tree', tree, ...(parent ? ['-p', parent] : []), '-m', message], site);
  if (!commit) fail('could not commit the site branch: git needs an author name and email');

  run('git', ['push', 'origin', `${commit}:refs/heads/${BRANCH}`]);
  console.log(`pushed ${commit.slice(0, 8)} to ${BRANCH}`);
}

export async function publish(argv: string[]): Promise<void> {
  const dryRun = argv.includes('--dry-run');
  const source = await readSource();
  const base = publicBase(source);

  const apps = await listApps();
  if (!apps.length) {
    console.log('apps/ is empty; nothing to publish');
    return;
  }

  const hasParent = await checkoutSite();
  const ledger = await readLedger(siteDir());
  const result = await stage(siteDir(), apps, ledger, base);

  const summary = result.added.map(a => `${a.slug} ${a.version}`).join(', ') || 'catalog refresh';
  if (!result.added.length) console.log('every app is already published at its current version');

  if (dryRun) {
    console.log(`\ndry run: would publish ${summary}`);
    console.log(`site/ now holds exactly what would land on ${BRANCH}`);
    return;
  }

  pushSite(`publish: ${summary}`, hasParent);
  console.log(`\ncatalog: ${base}/catalog.v1.json`);
}
