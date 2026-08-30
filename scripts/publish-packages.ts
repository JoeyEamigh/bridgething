#!/usr/bin/env bun
import { existsSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const ROOT = join(import.meta.dir, '..');

type Manifest = { name: string; dir: string };
export type Pkg = Manifest & { scoped: boolean };

export const PACKAGES: Pkg[] = [
  { name: '@bridgething/lib', dir: 'crates/lib', scoped: true },
  { name: '@bridgething/browser', dir: 'packages/browser', scoped: true },
  { name: '@bridgething/updater', dir: 'packages/updater', scoped: true },
  { name: '@bridgething/client', dir: 'packages/client-ts', scoped: true },
  { name: '@bridgething/extension', dir: 'packages/extension-ts', scoped: true },
  { name: 'create-bridgething', dir: 'packages/create-bridgething', scoped: false },
];

const VERSION_ONLY: Manifest[] = [{ name: '@bridgething/core-node', dir: 'crates/delivery/napi' }];

const BOOTSTRAP_PENDING = ['@bridgething/session-react-native', '@bridgething/webapp-shared'];

const SCAFFOLD = 'create-bridgething';

const SCAFFOLD_DEPENDENCIES = ['@bridgething/client', '@bridgething/extension'];

const TEMPLATE_MANIFEST = 'packages/create-bridgething/template/package.json';
const TEMPLATE_DEP = '@bridgething/client';

function daemonVersion(): string {
  const cargo = readFileSync(join(ROOT, 'Cargo.toml'), 'utf8');
  const version = cargo.match(/^\s*version\s*=\s*["']([^"']+)["']/m)?.[1];
  if (!version) {
    console.error('could not read version from Cargo.toml [workspace.package]');
    process.exit(1);
  }
  return version;
}

function setVersion(manifestPath: string, version: string): string | null {
  const full = join(ROOT, manifestPath);
  const src = readFileSync(full, 'utf8');
  const current = src.match(/"version"\s*:\s*"([^"]+)"/)?.[1] ?? null;
  const next = src.replace(/("version"\s*:\s*")[^"]+(")/, `$1${version}$2`);
  if (next !== src) writeFileSync(full, next);
  return current;
}

function setTemplateDep(version: string): string | null {
  const full = join(ROOT, TEMPLATE_MANIFEST);
  const src = readFileSync(full, 'utf8');
  const re = new RegExp(`("${TEMPLATE_DEP.replace('/', '\\/')}"\\s*:\\s*")([^"]+)(")`);
  const current = src.match(re)?.[2] ?? null;
  const next = src.replace(re, `$1^${version}$3`);
  if (next !== src) writeFileSync(full, next);
  return current;
}

function reEscape(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&');
}

function patchLockfile(version: string): void {
  const full = join(ROOT, 'bun.lock');
  if (!existsSync(full)) return;
  let src = readFileSync(full, 'utf8');
  for (const p of [...PACKAGES, ...VERSION_ONLY]) {
    const re = new RegExp(
      `("${reEscape(p.dir)}":\\s*\\{\\s*"name":\\s*"${reEscape(p.name)}",\\s*"version":\\s*")[^"]+(")`,
    );
    src = src.replace(re, `$1${version}$2`);
  }
  writeFileSync(full, src);
}

async function run(cmd: string, args: string[], cwd: string): Promise<void> {
  console.log(`\n$ ${cmd} ${args.join(' ')}  (cwd: ${cwd.replace(ROOT, '.')})`);
  const proc = Bun.spawn([cmd, ...args], { cwd, stdout: 'inherit', stderr: 'inherit', stdin: 'inherit' });
  const code = await proc.exited;
  if (code !== 0) {
    console.error(`\ncommand failed (exit ${code}): ${cmd} ${args.join(' ')}`);
    process.exit(code);
  }
}

async function npmViewOk(spec: string): Promise<boolean> {
  const proc = Bun.spawn(['npm', 'view', spec, 'version'], { cwd: ROOT, stdout: 'pipe', stderr: 'pipe' });
  await new Response(proc.stdout).text();
  await new Response(proc.stderr).text();
  return (await proc.exited) === 0;
}

export type Disposition = 'publish' | 'already-published' | 'needs-bootstrap';

export function scaffoldBlockers(plan: Map<string, Disposition>): string[] {
  return SCAFFOLD_DEPENDENCIES.filter(name => plan.get(name) === 'needs-bootstrap');
}

export function publishable(packages: Pkg[], plan: Map<string, Disposition>): Pkg[] {
  const blocked = scaffoldBlockers(plan).length > 0;
  return packages.filter(p => plan.get(p.name) === 'publish' && !(blocked && p.name === SCAFFOLD));
}

async function disposition(pkg: Pkg, version: string): Promise<Disposition> {
  if (!(await npmViewOk(pkg.name))) return 'needs-bootstrap';
  return (await npmViewOk(`${pkg.name}@${version}`)) ? 'already-published' : 'publish';
}

async function capture(cmd: string, args: string[], cwd: string): Promise<string> {
  console.log(`\n$ ${cmd} ${args.join(' ')}  (cwd: ${cwd.replace(ROOT, '.')})`);
  const proc = Bun.spawn([cmd, ...args], { cwd, stdout: 'pipe', stderr: 'inherit', stdin: 'inherit' });
  const out = await new Response(proc.stdout).text();
  process.stdout.write(out);
  const code = await proc.exited;
  if (code !== 0) {
    console.error(`\ncommand failed (exit ${code}): ${cmd} ${args.join(' ')}`);
    process.exit(code);
  }
  return out;
}

async function main(): Promise<void> {
  const doPublish = process.argv.includes('--publish');
  const version = daemonVersion();
  console.log(`daemon version: ${version}`);
  console.log(doPublish ? 'mode: PUBLISH (real)\n' : 'mode: dry run (pass --publish to publish for real)\n');

  console.log('aligning package versions:');
  for (const p of [...PACKAGES, ...VERSION_ONLY]) {
    const prev = setVersion(`${p.dir}/package.json`, version);
    console.log(`  ${p.name}: ${prev} -> ${version}`);
  }
  const prevDep = setTemplateDep(version);
  console.log(`  template dep ${TEMPLATE_DEP}: ${prevDep ?? '(none)'} -> ^${version}`);
  patchLockfile(version);
  console.log('  bun.lock: workspace members synced');

  console.log('\nregistry check:');
  const plan = new Map<string, Disposition>();
  for (const p of PACKAGES) {
    const d = await disposition(p, version);
    plan.set(p.name, d);
    const note = {
      publish: `will publish ${version}`,
      'already-published': `${version} already on registry, skipping`,
      'needs-bootstrap': 'NOT on registry - needs a manual first publish, skipping',
    }[d];
    console.log(`  ${p.name}: ${note}`);
  }
  if (BOOTSTRAP_PENDING.length > 0) {
    console.log(`\nnot yet publishable (never published, no trusted publisher possible):`);
    for (const name of BOOTSTRAP_PENDING) console.log(`  ${name}`);
  }

  const blockers = scaffoldBlockers(plan);
  if (blockers.length > 0) {
    console.error(
      `\nrefusing to publish ${SCAFFOLD}: it scaffolds a dependency on ${blockers.join(', ')}, which needs a manual first publish.`,
    );
    console.error('a scaffold pinning an unpublished package fails at bun install with a 404.');
  }

  const toPublish = publishable(PACKAGES, plan);
  if (toPublish.length === 0) {
    console.log('\nnothing to publish.');
    process.exit(0);
  }

  const filters = toPublish.flatMap(p => ['--filter', p.name]);
  await run('bunx', ['turbo', 'run', 'build', ...filters], ROOT);

  for (const p of toPublish) {
    const dir = join(ROOT, p.dir);
    if (!doPublish) {
      await run('bun', ['pm', 'pack', '--dry-run'], dir);
      continue;
    }
    const packOut = await capture('bun', ['pm', 'pack'], dir);
    const tarball = packOut
      .trim()
      .split('\n')
      .map(l => l.trim())
      .reverse()
      .find(l => l.endsWith('.tgz'));
    if (!tarball) {
      console.error(`could not find packed tarball name for ${p.name}`);
      process.exit(1);
    }
    const npmArgs = ['publish', tarball];
    if (p.scoped) npmArgs.push('--access', 'public');
    console.log(`\n$ npm ${npmArgs.join(' ')}  (cwd: ${dir.replace(ROOT, '.')})`);
    const proc = Bun.spawn(['npm', ...npmArgs], { cwd: dir, stdout: 'inherit', stderr: 'inherit', stdin: 'inherit' });
    const code = await proc.exited;
    const tgz = join(dir, tarball);
    if (existsSync(tgz)) unlinkSync(tgz);
    if (code !== 0) {
      console.error(`\nnpm publish failed (exit ${code}) for ${p.name}`);
      process.exit(code);
    }
  }

  const names = toPublish.map(p => p.name).join(', ');
  console.log(
    doPublish
      ? `\npublished ${names} at ${version}`
      : `\ndry run complete. re-run with --publish to publish ${names} at ${version}`,
  );
}

if (import.meta.main) await main();
