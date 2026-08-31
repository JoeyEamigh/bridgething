#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { basename, dirname, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { v7 as uuidv7 } from 'uuid';
import { asker, isSlug, slugify, type Asker } from './prompt.ts';
import {
  copyTemplate,
  linkOrCopy,
  move,
  patchJson,
  readJson,
  sortKeys,
  writeJson,
  type Substitutions,
} from './template.ts';

const __dirname = dirname(fileURLToPath(import.meta.url));
const PACKAGE_ROOT = resolve(__dirname, '..');
const TEMPLATE_DIR = join(PACKAGE_ROOT, 'template');
const SOURCE_TEMPLATE_DIR = join(PACKAGE_ROOT, 'template-source');
const SKILL_IN_APP = join('.claude', 'skills', 'bridgething');

const SDK_VERSIONS = readJson<Record<string, string>>(join(PACKAGE_ROOT, 'template-versions.json'));

const ESBUILD_VERSION = '^0.28.2';
const DENO_VERSION = '2.9.6';

type Variant = 'app' | 'launcher' | 'overlay';

const EXTENSION = {
  dir: 'template-extension',
  manifest: {
    extension: { entry: 'extension/desktop.mjs', permissions: ['all'], api: 1 },
    config: [{ type: 'string', data: { key: 'greeting', label: 'Greeting the extension sends on connect' } }],
  },
  dependencies: { '@bridgething/extension': SDK_VERSIONS['@bridgething/extension']! },
  devDependencies: { deno: DENO_VERSION, esbuild: ESBUILD_VERSION },
  summary: 'native extension (a desktop-side Deno process)',
};

const VARIANTS: Record<
  Variant,
  { dir?: string; manifest?: Record<string, unknown>; scripts?: Record<string, string>; summary: string }
> = {
  app: { summary: 'webapp' },
  launcher: {
    dir: 'template-launcher',
    manifest: { role: 'launcher', description: 'A custom home screen.' },
    summary: 'launcher (a replacement home screen)',
  },
  overlay: {
    dir: 'template-overlay',
    manifest: { overlay: 'overlay.js', description: 'A custom system overlay.' },
    scripts: {
      build: 'vite build && vite build -c vite.settings.config.ts && vite build -c vite.overlay.config.ts',
    },
    summary: 'overlay (system UI drawn over every webapp)',
  },
};

interface Args {
  target: string | null;
  install: boolean;
  git: boolean;
  interactive: boolean;
  variant: Variant;
  extension: boolean;
  sourceName: string | null;
  sourceDescription: string | null;
  repo: string | null;
}

function parseArgs(argv: string[]): Args {
  const args: Args = {
    target: null,
    install: true,
    git: true,
    interactive: true,
    variant: 'app',
    extension: false,
    sourceName: null,
    sourceDescription: null,
    repo: null,
  };
  const value = (at: number, flag: string): string => argv[at] ?? die(`${flag} needs a value`);

  for (let at = 0; at < argv.length; at++) {
    const arg = argv[at]!;
    if (arg === '--no-install') args.install = false;
    else if (arg === '--no-git') args.git = false;
    else if (arg === '--yes' || arg === '-y') args.interactive = false;
    else if (arg === '--launcher') args.variant = 'launcher';
    else if (arg === '--overlay') args.variant = 'overlay';
    else if (arg === '--extension') args.extension = true;
    else if (arg === '--source-name') args.sourceName = value(++at, '--source-name');
    else if (arg === '--source-description') args.sourceDescription = value(++at, '--source-description');
    else if (arg === '--repo') args.repo = value(++at, '--repo');
    else if (arg === '--help' || arg === '-h') {
      printHelp();
      process.exit(0);
    } else if (arg.startsWith('-')) {
      die(`unknown flag: ${arg}`);
    } else if (args.target) {
      die('name one target');
    } else {
      args.target = arg;
    }
  }
  return args;
}

function printHelp(): void {
  console.log(`Usage: create-bridgething [target] [options]

scaffold a bridgething app at [target].

Variants:
  (default)     a normal webapp
  --launcher    a home screen that can take the device's launcher slot, replacing the built-in hub
  --overlay     A system overlay that can be injected into every webapp, replacing the built-in one

Add-ons:
  --extension   A native extension with a Deno process the desktop app runs alongside the webapp

Options:
  --source-name <name>         name the source (app store repo) instead of asking
  --source-description <text>  describe the source instead of asking
  --repo <owner/repo>          the github repo this source publishes from
  -y, --yes                    take every default
  --no-install                 skip 'bun install'
  --no-git                     skip 'git init'
`);
}

function die(message: string): never {
  console.error(`error: ${message}`);
  process.exit(1);
}

function capture(cmd: string, args: string[], cwd = process.cwd()): string | null {
  const result = spawnSync(cmd, args, { encoding: 'utf8', cwd });
  return result.status === 0 ? result.stdout.trim() : null;
}

function run(cmd: string, args: string[], cwd: string): boolean {
  return spawnSync(cmd, args, { cwd, stdio: 'inherit' }).status === 0;
}

function findSourceRoot(from: string): string | null {
  let dir = resolve(from);
  for (;;) {
    if (existsSync(join(dir, 'source.json')) && existsSync(join(dir, 'apps'))) return dir;
    const parent = dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

function repoFromRemote(cwd: string): string | null {
  const remote = capture('git', ['remote', 'get-url', 'origin'], cwd);
  const match = remote ? /github\.com[:/]([^/]+)\/(.+?)(?:\.git)?$/.exec(remote) : null;
  return match ? `${match[1]}/${match[2]}` : null;
}

function sourceHomepage(root: string): string | null {
  const homepage = readJson<{ homepage?: string | null }>(join(root, 'source.json')).homepage ?? null;
  return homepage && /^https:\/\/github\.com\/[^/]+\/[^/]+\/?$/.test(homepage) ? homepage.replace(/\/$/, '') : null;
}

function githubLogin(): string | null {
  return capture('gh', ['api', 'user', '-q', '.login']) ?? capture('git', ['config', 'github.user']);
}

function splitRepo(repo: string): { owner: string; name: string } {
  const match = /^([A-Za-z0-9][A-Za-z0-9-]*)\/([A-Za-z0-9._-]+)$/.exec(repo.replace(/\.git$/, ''));
  if (!match) die(`"${repo}" is not owner/repo`);
  return { owner: match[1]!, name: match[2]! };
}

function repoSubs(repo: { owner: string; name: string }): Substitutions {
  return {
    __REPO_OWNER__: repo.owner,
    __REPO_OWNER_LOWER__: repo.owner.toLowerCase(),
    __REPO_NAME__: repo.name,
    __BASE_URL__: `https://${repo.owner.toLowerCase()}.github.io/${repo.name}`,
  };
}

type PackagePatch = {
  scripts?: Record<string, string>;
  dependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
};

function patchPackage(dir: string, patch: PackagePatch): void {
  const path = join(dir, 'package.json');
  const pkg = readJson<{
    scripts?: Record<string, string>;
    dependencies?: Record<string, string>;
    devDependencies?: Record<string, string>;
  }>(path);
  patchJson(path, {
    scripts: { ...pkg.scripts, ...patch.scripts },
    dependencies: sortKeys({ ...pkg.dependencies, ...patch.dependencies }),
    devDependencies: sortKeys({ ...pkg.devDependencies, ...patch.devDependencies }),
  });
}

function overlayTemplate(target: string, dir: string, subs: Substitutions): void {
  copyTemplate(join(PACKAGE_ROOT, dir), target, subs);

  const appendPath = join(target, '_claude_append.md');
  if (!existsSync(appendPath)) return;
  const claudePath = join(target, 'CLAUDE.md');
  const addition = readFileSync(appendPath, 'utf8');
  const existing = existsSync(claudePath) ? `${readFileSync(claudePath, 'utf8').trimEnd()}\n\n` : '';
  writeFileSync(claudePath, `${existing}${existing ? addition : addition.replace(/^## /, '# ')}`);
  rmSync(appendPath, { force: true });
}

interface AppSpec {
  slug: string;
  variant: Variant;
  extension: boolean;
  author: string;
  repoUrl: string | null;
}

function scaffoldApp(root: string, spec: AppSpec): { dir: string; uuid: string; shape: string } {
  const dir = join(root, 'apps', spec.slug);
  if (existsSync(dir) && readdirSync(dir).length) die(`apps/${spec.slug} already exists`);

  const uuid = uuidv7();
  const subs: Substitutions = { __PROJECT_NAME__: spec.slug, __WEBAPP_UUID__: uuid, __APP_SLUG__: spec.slug };

  copyTemplate(TEMPLATE_DIR, dir, subs);
  patchPackage(dir, { dependencies: { '@bridgething/client': SDK_VERSIONS['@bridgething/client']! } });

  const variant = VARIANTS[spec.variant];
  if (variant.dir) overlayTemplate(dir, variant.dir, subs);
  if (variant.manifest) patchJson(join(dir, 'public', 'manifest.json'), variant.manifest);
  if (variant.scripts) patchPackage(dir, { scripts: variant.scripts });

  if (spec.extension) {
    overlayTemplate(dir, EXTENSION.dir, subs);
    patchJson(join(dir, 'public', 'manifest.json'), EXTENSION.manifest);
    patchPackage(dir, { dependencies: EXTENSION.dependencies, devDependencies: EXTENSION.devDependencies });
  }

  const tsconfigPath = join(dir, 'tsconfig.json');
  const tsconfig = readJson<{ include: string[] }>(tsconfigPath);
  patchJson(tsconfigPath, { include: tsconfig.include.filter(entry => existsSync(join(dir, entry))) });

  writeJson(join(dir, 'catalog.json'), {
    author: spec.author,
    homepage: spec.repoUrl,
    source: spec.repoUrl,
    icon: null,
    screenshots: [],
    min_libbridgething_version: SDK_VERSIONS.libbridgething!,
  });

  const manifest = readJson<{ name: string; version: string }>(join(dir, 'public', 'manifest.json'));
  writeFileSync(join(dir, 'CHANGELOG.md'), `# ${manifest.name}\n\n## ${manifest.version}\n\nFirst release.\n`);

  const shape = spec.extension ? `${variant.summary} + ${EXTENSION.summary}` : variant.summary;
  return { dir, uuid, shape };
}

function linkAgentGuides(root: string, appDir: string): void {
  const rootSkill = join(root, SKILL_IN_APP);
  const appSkill = join(appDir, SKILL_IN_APP);

  if (!existsSync(rootSkill)) move(appSkill, rootSkill);
  linkOrCopy(relative(dirname(appSkill), rootSkill), appSkill, rootSkill);

  for (const link of [join(root, 'AGENTS.md'), join(appDir, 'AGENTS.md')]) {
    if (existsSync(link) || !existsSync(join(dirname(link), 'CLAUDE.md'))) continue;
    try {
      symlinkSync('CLAUDE.md', link);
    } catch {
      writeFileSync(link, readFileSync(join(dirname(link), 'CLAUDE.md'), 'utf8'));
    }
  }

  for (const dir of [root, appDir]) {
    mkdirSync(join(dir, '.agents'), { recursive: true });
    linkOrCopy(join('..', '.claude', 'skills'), join(dir, '.agents', 'skills'), join(dir, '.claude', 'skills'));
  }
}

async function askSourceDetails(
  ask: Asker,
  args: Args,
): Promise<{ name: string; description: string; repo: { owner: string; name: string }; app: string }> {
  const named = args.target && !isSlug(args.target) ? basename(resolve(args.target)) : args.target;
  const name = args.sourceName ?? (await ask.ask('Source name', named || 'My Car Thing Apps'));
  const description = args.sourceDescription ?? (await ask.ask('Source description', `Webapps by ${name}.`));

  const owner = githubLogin();
  const suggested = `${owner ?? 'your-github-username'}/${slugify(name)}`;
  const repo = splitRepo(args.repo ?? (await ask.ask('GitHub repo', suggested)));

  let app = args.target && isSlug(args.target) ? args.target : '';
  while (!app) {
    const answer = slugify(await ask.ask('First app', named ? slugify(named) : 'weather'));
    if (isSlug(answer)) app = answer;
  }
  return { name, description, repo, app };
}

async function createSource(args: Args): Promise<void> {
  const ask = asker(args.interactive);
  let details;
  try {
    details = await askSourceDetails(ask, args);
    if (details.repo.owner === 'your-github-username') {
      die('pass --repo <owner/repo>, or answer the GitHub repo prompt; the catalog url is derived from it');
    }
  } finally {
    ask.close();
  }

  const target = resolve(process.cwd(), args.target && !isSlug(args.target) ? args.target : details.repo.name);
  if (existsSync(target) && readdirSync(target).length) die(`${target} exists and is not empty`);

  const subs: Substitutions = {
    ...repoSubs(details.repo),
    __SOURCE_NAME__: details.name,
    __SOURCE_DESCRIPTION__: details.description,
    __APP_SLUG__: details.app,
  };

  console.log(`\nscaffolding ${details.name} in ${target}`);
  copyTemplate(SOURCE_TEMPLATE_DIR, target, subs);
  patchPackage(target, { devDependencies: { '@bridgething/source': SDK_VERSIONS['@bridgething/source']! } });

  const repoUrl = `https://github.com/${details.repo.owner}/${details.repo.name}`;
  const app = scaffoldApp(target, {
    slug: details.app,
    variant: args.variant,
    extension: args.extension,
    author: capture('git', ['config', 'user.name']) ?? details.repo.owner,
    repoUrl,
  });
  linkAgentGuides(target, app.dir);
  console.log(`  ✓ source and apps/${details.app} (${app.uuid}) created as a ${app.shape}`);

  if (args.git && run('git', ['init', '--quiet'], target)) console.log('  ✓ git initialized');
  if (args.install) {
    console.log('  installing dependencies with bun...');
    if (!run('bun', ['install'], target)) console.warn('  ! bun install failed; install manually with `bun install`');
    else console.log('  ✓ dependencies installed');
  }

  await offerRepoCreate(args, target, details.repo, details.description);
  printSourceNextSteps(target, details, repoUrl);
}

async function offerRepoCreate(
  args: Args,
  target: string,
  repo: { owner: string; name: string },
  description: string,
): Promise<void> {
  if (!args.interactive || !args.git || !capture('gh', ['auth', 'status'])) return;
  const ask = asker(true);
  try {
    if (!(await ask.confirm(`\nCreate github.com/${repo.owner}/${repo.name} and push now?`))) return;
  } finally {
    ask.close();
  }

  run('git', ['add', '--all'], target);
  run('git', ['commit', '--quiet', '-m', 'scaffold the app source'], target);
  const created = run(
    'gh',
    ['repo', 'create', `${repo.owner}/${repo.name}`, '--public', '--source', '.', '--push', '-d', description],
    target,
  );
  console.log(created ? '  ✓ repo created and pushed' : '  ! gh repo create failed; push it yourself');
}

function printSourceNextSteps(
  target: string,
  details: { name: string; repo: { owner: string; name: string }; app: string },
  repoUrl: string,
): void {
  const base = `https://${details.repo.owner.toLowerCase()}.github.io/${details.repo.name}`;
  console.log(`
Done. ${target}

  cd ${basename(target)}
  bun run dev          # local dev server against the connected Car Thing
  bun run dev:device   # dev server on the Car Thing's own screen
  bun run check        # check that your app and source is valid

Fill in the description in apps/${details.app}/public/manifest.json.

Push to ${repoUrl}, then in Settings > Pages set the branch to gh-pages, folder / (root).
Your catalog ends up at at ${base}/catalog.v1.json, which you can submit to the bridgething store.

  bun run new <slug>   add another app to this source
  bun run bump ${details.app} [version]
`);
}

async function addApp(root: string, args: Args): Promise<void> {
  const ask = asker(args.interactive);
  let slug = args.target ?? '';
  try {
    while (!isSlug(slug)) {
      if (slug) console.error(`"${slug}" is not a usable slug; use lowercase letters, digits and dashes`);
      slug = slugify(await ask.ask('App slug', 'weather'));
      if (!args.interactive) break;
    }
  } finally {
    ask.close();
  }
  if (!isSlug(slug)) die('name the app: create-bridgething <slug>');

  const repo = repoFromRemote(root);
  const repoUrl = repo ? `https://github.com/${repo}` : sourceHomepage(root);
  if (args.extension && !repoUrl) {
    console.warn(
      'warning: this source has no github origin, so catalog.json "source" is null. An app that ships an\n' +
        '         extension must have its repo url before it can be published.',
    );
  }

  const app = scaffoldApp(root, {
    slug,
    variant: args.variant,
    extension: args.extension,
    author: capture('git', ['config', 'user.name'], root) ?? 'Your Name',
    repoUrl,
  });
  linkAgentGuides(root, app.dir);
  console.log(`apps/${slug} (${app.uuid}) created as a ${app.shape}`);

  if (args.install && !run('bun', ['install'], root)) {
    console.warn('! bun install failed. install manually with `bun install`');
  }

  console.log(`
Next:

  fill in the description in apps/${slug}/public/manifest.json, and check the author
  and repo url in apps/${slug}/catalog.json

  bun run dev ${slug}      develop against a connected Car Thing
  bun run check            ensure your app and source are valid
  bun run bump ${slug} patch

Push to main and the publish workflow puts it in your catalog.`);
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const root = findSourceRoot(process.cwd());
  if (root) await addApp(root, args);
  else await createSource(args);
}

try {
  await main();
} catch (err) {
  if ((err as { name?: string }).name === 'AbortError') {
    console.log('\ncancelled; nothing was written.');
    process.exit(130);
  }
  throw err;
}
