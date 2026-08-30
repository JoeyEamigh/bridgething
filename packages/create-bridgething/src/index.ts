#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { v7 as uuidv7 } from 'uuid';

const __dirname = dirname(fileURLToPath(import.meta.url));
const TEMPLATE_DIR = resolve(__dirname, '..', 'template');

type Variant = 'app' | 'launcher' | 'overlay';

const SELF_VERSION = (JSON.parse(readFileSync(resolve(__dirname, '..', 'package.json'), 'utf8')) as { version: string })
  .version;

const EXTENSION_PACKAGE_VERSION = `^${SELF_VERSION}`;
const ESBUILD_VERSION = '^0.28.2';
const DENO_VERSION = '2.9.6';

const EXTENSION = {
  dir: 'template-extension',
  manifest: {
    extension: { entry: 'extension/desktop.mjs', permissions: ['all'], api: 1 },
    config: [{ type: 'string', data: { key: 'greeting', label: 'Greeting the extension sends on connect' } }],
  },
  dependencies: { '@bridgething/extension': EXTENSION_PACKAGE_VERSION },
  devDependencies: { deno: DENO_VERSION, esbuild: ESBUILD_VERSION },
  summary: 'native extension (a desktop-side Deno process)',
};

const VARIANTS: Record<
  Variant,
  {
    dir?: string;
    manifest?: Record<string, unknown>;
    scripts?: Record<string, string>;
    summary: string;
  }
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

type Args = {
  target: string;
  install: boolean;
  git: boolean;
  variant: Variant;
  extension: boolean;
};

function parseArgs(argv: string[]): Args {
  const positional: string[] = [];
  let install = true;
  let git = true;
  let variant: Variant = 'app';
  let extension = false;
  for (const arg of argv) {
    if (arg === '--no-install') install = false;
    else if (arg === '--no-git') git = false;
    else if (arg === '--launcher') variant = 'launcher';
    else if (arg === '--overlay') variant = 'overlay';
    else if (arg === '--extension') extension = true;
    else if (arg === '--help' || arg === '-h') {
      printHelp();
      process.exit(0);
    } else if (arg.startsWith('-')) {
      console.error(`unknown flag: ${arg}`);
      printHelp();
      process.exit(1);
    } else {
      positional.push(arg);
    }
  }
  if (positional.length !== 1) {
    printHelp();
    process.exit(1);
  }
  return { target: positional[0], install, git, variant, extension };
}

function printHelp(): void {
  console.log(`Usage: create-bridgething <target-dir> [--launcher | --overlay] [--extension] [--no-install] [--no-git]

Scaffold a new bridgething webapp at <target-dir>.

Variants:
  (default)     A normal webapp.
  --launcher    A home screen. Declares 'role: launcher' and can take the
                device's launcher slot, replacing the built-in hub.
  --overlay     A system overlay. Ships an 'overlay.js' the daemon injects
                into every webapp, replacing the built-in one.

Add-ons (combine with any variant):
  --extension   A native extension: a Deno process the desktop app runs
                alongside the webapp, with host access the manifest declares.

Options:
  --no-install  Skip 'bun install' after copying.
  --no-git      Skip 'git init' after copying.
`);
}

function patchJson(path: string, patch: Record<string, unknown>): void {
  const parsed = JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>;
  writeFileSync(path, `${JSON.stringify({ ...parsed, ...patch }, null, 2)}\n`);
}

type PackagePatch = {
  scripts?: Record<string, string>;
  dependencies?: Record<string, string>;
  devDependencies?: Record<string, string>;
};

function patchPackage(target: string, patch: PackagePatch): void {
  const path = join(target, 'package.json');
  const pkg = JSON.parse(readFileSync(path, 'utf8')) as {
    scripts: Record<string, string>;
    dependencies: Record<string, string>;
    devDependencies: Record<string, string>;
  };
  patchJson(path, {
    scripts: { ...pkg.scripts, ...patch.scripts },
    dependencies: sortKeys({ ...pkg.dependencies, ...patch.dependencies }),
    devDependencies: sortKeys({ ...pkg.devDependencies, ...patch.devDependencies }),
  });
}

function sortKeys(record: Record<string, string>): Record<string, string> {
  return Object.fromEntries(Object.entries(record).sort(([a], [b]) => a.localeCompare(b)));
}

function overlayTemplate(target: string, dir: string, subs: Substitutions): void {
  copyTemplate(resolve(__dirname, '..', dir), target, subs);

  const appendPath = join(target, '_claude_append.md');
  if (!existsSync(appendPath)) return;
  const claudePath = join(target, 'CLAUDE.md');
  writeFileSync(claudePath, `${readFileSync(claudePath, 'utf8').trimEnd()}\n\n${readFileSync(appendPath, 'utf8')}`);
  rmSync(appendPath);
}

function applyVariant(target: string, variant: Variant, subs: Substitutions): void {
  const spec = VARIANTS[variant];
  if (spec.dir) overlayTemplate(target, spec.dir, subs);
  if (spec.manifest) patchJson(join(target, 'public', 'manifest.json'), spec.manifest);
  if (spec.scripts) patchPackage(target, { scripts: spec.scripts });
}

function applyExtension(target: string, subs: Substitutions): void {
  overlayTemplate(target, EXTENSION.dir, subs);
  patchJson(join(target, 'public', 'manifest.json'), EXTENSION.manifest);
  patchPackage(target, {
    dependencies: EXTENSION.dependencies,
    devDependencies: EXTENSION.devDependencies,
  });
}

type Substitutions = {
  projectName: string;
  webappUuid: string;
};

const BINARY_EXT = new Set([
  '.ttf',
  '.otf',
  '.woff',
  '.woff2',
  '.png',
  '.jpg',
  '.jpeg',
  '.gif',
  '.webp',
  '.avif',
  '.ico',
  '.wasm',
]);

function copyTemplate(src: string, dest: string, subs: Substitutions): void {
  for (const entry of readdirSync(src)) {
    const srcPath = join(src, entry);
    const renamed = entry === '_gitignore' ? '.gitignore' : entry === '_claude' ? '.claude' : entry;
    const destPath = join(dest, renamed);
    const stat = statSync(srcPath);
    if (stat.isDirectory()) {
      mkdirSync(destPath, { recursive: true });
      copyTemplate(srcPath, destPath, subs);
    } else if (BINARY_EXT.has(extname(entry).toLowerCase())) {
      copyFileSync(srcPath, destPath);
    } else {
      const raw = readFileSync(srcPath, 'utf8');
      const substituted = raw
        .replace(/__PROJECT_NAME__/g, subs.projectName)
        .replace(/__WEBAPP_UUID__/g, subs.webappUuid);
      writeFileSync(destPath, substituted);
    }
  }
}

function copyDir(src: string, dest: string): void {
  mkdirSync(dest, { recursive: true });
  for (const entry of readdirSync(src)) {
    const srcPath = join(src, entry);
    const destPath = join(dest, entry);
    if (statSync(srcPath).isDirectory()) copyDir(srcPath, destPath);
    else copyFileSync(srcPath, destPath);
  }
}

function linkAgentAliases(target: string): void {
  try {
    symlinkSync('CLAUDE.md', join(target, 'AGENTS.md'));
  } catch {
    copyFileSync(join(target, 'CLAUDE.md'), join(target, 'AGENTS.md'));
  }
  mkdirSync(join(target, '.agents'), { recursive: true });
  try {
    symlinkSync(join('..', '.claude', 'skills'), join(target, '.agents', 'skills'), 'dir');
  } catch {
    copyDir(join(target, '.claude', 'skills'), join(target, '.agents', 'skills'));
  }
}

function run(cmd: string, args: string[], cwd: string): boolean {
  const result = spawnSync(cmd, args, { cwd, stdio: 'inherit' });
  return result.status === 0;
}

function main(): void {
  const args = parseArgs(process.argv.slice(2));
  const target = resolve(process.cwd(), args.target);
  const projectName = args.target.replace(/^.*[\\/]/, '');

  if (existsSync(target)) {
    const entries = readdirSync(target);
    if (entries.length > 0) {
      console.error(`error: ${target} exists and is not empty`);
      process.exit(1);
    }
  } else {
    mkdirSync(target, { recursive: true });
  }

  const webappUuid = uuidv7();
  const subs = { projectName, webappUuid };
  console.log(`scaffolding ${projectName} (${webappUuid}) in ${target}`);
  copyTemplate(TEMPLATE_DIR, target, subs);
  applyVariant(target, args.variant, subs);
  if (args.extension) applyExtension(target, subs);
  const shape = args.extension
    ? `${VARIANTS[args.variant].summary} + ${EXTENSION.summary}`
    : VARIANTS[args.variant].summary;
  console.log(`  ✓ template copied (${shape})`);

  linkAgentAliases(target);
  console.log('  ✓ agent guides linked (CLAUDE.md, AGENTS.md, /bridgething skill)');

  if (args.git) {
    if (run('git', ['init', '--quiet'], target)) {
      console.log('  ✓ git initialized');
    } else {
      console.warn('  ! git init failed (skipping)');
    }
  }

  if (args.install) {
    console.log('  installing dependencies with bun...');
    if (!run('bun', ['install'], target)) {
      console.warn('  ! bun install failed; install manually with `bun install`');
    } else {
      console.log('  ✓ dependencies installed');
    }
  }

  console.log(`
Done! Next steps:

  cd ${args.target}
${args.install ? '' : '  bun install\n'}  bun run dev          # local dev server (http://localhost:5173/) against the connected Car Thing
  bun run dev:device   # the same server on the Car Thing's own screen, hot reload included

Open this folder with your coding agent (Claude Code, Codex, opencode, ...).
It reads CLAUDE.md / AGENTS.md and the /bridgething skill in .claude/skills
(mirrored at .agents/skills), which goes deep on the client API, running and
driving the app, and installing and sharing it.

  bun run build        # production bundle into dist/
  bun run push <addr>  # build + install onto a connected Car Thing (default bridgething.local)
  bun run share        # build first, then zip dist/ to hand to friends
  bun run update       # bring the connected Car Thing to the latest bridgething release
${
  args.extension
    ? `
'extension/main.ts' is the desktop-side half: a Deno process the bridgething
desktop app runs alongside this webapp, with the host access 'manifest.json'
declares. 'bun run dev' and 'bun run dev:device' also run it, rebuilt on every
save and wired to the connected Car Thing, so both halves iterate together.
'bun run build' bundles it to dist/extension/desktop.mjs.
`
    : ''
}${
    args.variant === 'app'
      ? ''
      : `
'bun run push' also claims the device's ${args.variant} slot, so this is live the
moment it lands. 'bun run push --release' hands the slot back to the built-in
${args.variant === 'launcher' ? 'hub' : 'overlay'}, which is how you recover if a build of this wedges the screen.
The companion phone app can do the same thing.
`
  }
The starter App connects to the daemon through daemonUrl() in src/daemon.ts: the
local daemon on the device, and the dev server's proxy to the Car Thing over USB
under bun run dev (SUPERBIRD_HOST picks another device).
`);
}

main();
