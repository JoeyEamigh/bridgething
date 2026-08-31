import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { readJson, run, writeBytes, writeJson } from './lib.ts';
import { fail, sourceRoot } from './paths.ts';

const PACKAGE = 'create-bridgething';
const SKILL_PATH = 'template/_claude/skills/bridgething';

export function skillsDir(root: string = sourceRoot()): string {
  return join(root, '.claude', 'skills', 'bridgething');
}

export interface Stamp {
  package: string;
  version: string;
  fetched_at: string;
  local?: true;
}

function stampPath(root?: string): string {
  return join(skillsDir(root), '.source.json');
}

async function latestVersion(): Promise<string> {
  const response = await fetch(`https://registry.npmjs.org/${PACKAGE}/latest`);
  if (!response.ok) fail(`could not reach the npm registry (${response.status})`);
  return ((await response.json()) as { version: string }).version;
}

export async function installedStamp(root?: string): Promise<Stamp | null> {
  return existsSync(stampPath(root)) ? readJson<Stamp>(stampPath(root)) : null;
}

async function install(from: string, stamp: Stamp, root?: string): Promise<void> {
  if (!existsSync(from)) fail(`${from} does not exist`);
  const target = skillsDir(root);
  await rm(target, { recursive: true, force: true });
  await mkdir(dirname(target), { recursive: true });
  run('cp', ['-R', from, target]);
  await writeJson(stampPath(root), stamp);
}

async function vendorLocal(checkout: string): Promise<string> {
  const pkg = join(checkout, 'package.json');
  if (!existsSync(pkg)) fail(`${checkout} is not a ${PACKAGE} checkout (no package.json)`);
  const { name, version } = await readJson<{ name: string; version: string }>(pkg);
  if (name !== PACKAGE) fail(`${checkout} is ${name}, not ${PACKAGE}`);

  await install(join(checkout, SKILL_PATH), {
    package: PACKAGE,
    version,
    fetched_at: new Date().toISOString(),
    local: true,
  });
  return version;
}

export async function download(version: string, root?: string): Promise<void> {
  const meta = await fetch(`https://registry.npmjs.org/${PACKAGE}/${version}`);
  if (!meta.ok) fail(`no ${PACKAGE}@${version} on the registry (${meta.status})`);
  const tarball = ((await meta.json()) as { dist: { tarball: string } }).dist.tarball;

  const scratch = await mkdtemp(join(tmpdir(), 'bridgething-skills-'));
  try {
    const archive = join(scratch, 'package.tgz');
    const bytes = await fetch(tarball);
    if (!bytes.ok) fail(`could not download ${tarball} (${bytes.status})`);
    await writeBytes(archive, new Uint8Array(await bytes.arrayBuffer()));

    run('tar', ['xzf', archive, '-C', scratch, `package/${SKILL_PATH}`]);
    const extracted = join(scratch, 'package', SKILL_PATH);
    if (!existsSync(extracted)) fail(`${PACKAGE}@${version} does not carry ${SKILL_PATH}`);

    await install(extracted, { package: PACKAGE, version, fetched_at: new Date().toISOString() }, root);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

export async function reportSkillDrift(): Promise<void> {
  const stamp = await installedStamp();
  if (!stamp) return;
  const latest = await latestVersion().catch(() => null);
  if (!latest || stamp.local || stamp.version === latest) return;
  console.log(`skill is from ${PACKAGE}@${stamp.version}; ${latest} is out. run "bun run skills" to refresh`);
}

export async function skills(argv: string[]): Promise<void> {
  const fromAt = argv.indexOf('--from');
  if (fromAt !== -1) {
    const checkout = argv[fromAt + 1] ?? fail('--from needs a path to a create-bridgething checkout');
    console.log(`vendored the skill from ${checkout} at ${await vendorLocal(checkout)}`);
    console.log('a later "bun run skills" replaces it with whatever is published.');
    return;
  }

  if (argv.includes('--check')) {
    const [stamp, latest] = await Promise.all([installedStamp(), latestVersion()]);
    if (!stamp) {
      console.log(`no skill installed; run "bun run skills" to fetch ${PACKAGE}@${latest}`);
      process.exit(1);
    }
    if (stamp.local) {
      console.log(`skill is vendored from a checkout at ${stamp.version}; published is ${latest}`);
      return;
    }
    if (stamp.version === latest) {
      console.log(`skill is current (${PACKAGE}@${stamp.version})`);
      return;
    }
    console.log(`skill is from ${PACKAGE}@${stamp.version}; ${latest} is out. run "bun run skills" to refresh`);
    process.exit(1);
  }

  const version = await latestVersion();
  const stamp = await installedStamp();
  await download(version);
  const had = stamp?.version ?? 'none';
  console.log(had === version ? `refreshed the skill at ${version}` : `skill ${had} -> ${version}`);
  console.log('.claude/skills/bridgething is shared by every app in apps/; commit the change.');
}
