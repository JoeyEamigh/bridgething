import { compareVersions } from '@bridgething/catalog';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readJson, run, writeBytes, writeJson } from './lib.ts';
import { fail, sourceRoot } from './paths.ts';

const PACKAGE = 'create-bridgething';
const SKILL_PATH = 'template/_claude/skills/bridgething';
const TEMPLATE_PACKAGE = 'template-source/package.json';
const TEMPLATE_PINS = 'template-versions.json';
const TOOLKIT = '@bridgething/source';
const TOOLKIT_BIN = 'bridgething-source';

export function skillsDir(root: string = sourceRoot()): string {
  return join(root, '.claude', 'skills', 'bridgething');
}

export interface Toolkit {
  version: string;
  scripts: Record<string, string>;
}

export interface Stamp {
  package: string;
  version: string;
  fetched_at: string;
  local?: true;
  toolkit?: Toolkit;
}

type Scripts = { scripts?: Record<string, string> };

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

async function readToolkit(packageRoot: string): Promise<Toolkit> {
  const pins = await readJson<Record<string, string>>(join(packageRoot, TEMPLATE_PINS));
  const pin = pins[TOOLKIT] ?? fail(`${PACKAGE} ${TEMPLATE_PINS} does not pin ${TOOLKIT}`);
  const { scripts = {} } = await readJson<Scripts>(join(packageRoot, TEMPLATE_PACKAGE));
  return {
    version: pin.replace(/^[\^~]/, ''),
    scripts: Object.fromEntries(Object.entries(scripts).filter(([, command]) => command.startsWith(`${TOOLKIT_BIN} `))),
  };
}

async function runningToolkit(): Promise<string> {
  const own = join(dirname(fileURLToPath(import.meta.url)), '..', 'package.json');
  return (await readJson<{ version: string }>(own)).version;
}

export async function toolkitDrift(stamp: Stamp | null, root: string = sourceRoot()): Promise<string[]> {
  if (!stamp?.toolkit) return [];
  const problems: string[] = [];

  const running = await runningToolkit();
  if (compareVersions(running, stamp.toolkit.version) < 0) {
    problems.push(
      `the skill expects ${TOOLKIT} ${stamp.toolkit.version} and this source has ${running}. ` +
        `run "bun add -d ${TOOLKIT}@latest"`,
    );
  }

  const { scripts = {} } = await readJson<Scripts>(join(root, 'package.json'));
  const missing = Object.entries(stamp.toolkit.scripts).filter(([name]) => !(name in scripts));
  if (missing.length) {
    const lines = missing.map(([name, command]) => `  "${name}": "${command}"`).join(',\n');
    problems.push(`package.json is missing scripts the skill uses. add them to "scripts":\n${lines}`);
  }

  return problems;
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
    toolkit: await readToolkit(checkout),
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

    run('tar', [
      'xzf',
      archive,
      '-C',
      scratch,
      `package/${SKILL_PATH}`,
      `package/${TEMPLATE_PACKAGE}`,
      `package/${TEMPLATE_PINS}`,
    ]);
    const unpacked = join(scratch, 'package');
    const extracted = join(unpacked, SKILL_PATH);
    if (!existsSync(extracted)) fail(`${PACKAGE}@${version} does not carry ${SKILL_PATH}`);

    const toolkit = await readToolkit(unpacked);
    await install(extracted, { package: PACKAGE, version, fetched_at: new Date().toISOString(), toolkit }, root);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

function skillBehind(stamp: Stamp, latest: string | null): string | null {
  if (!latest || stamp.local || stamp.version === latest) return null;
  return `skill is from ${PACKAGE}@${stamp.version}; ${latest} is out. run "bun run skills" to refresh`;
}

export async function reportSkillDrift(): Promise<void> {
  const stamp = await installedStamp();
  if (!stamp) return;
  const behind = skillBehind(stamp, await latestVersion().catch(() => null));
  for (const line of [...(behind ? [behind] : []), ...(await toolkitDrift(stamp))]) console.log(line);
}

export async function skills(argv: string[]): Promise<void> {
  const fromAt = argv.indexOf('--from');
  if (fromAt !== -1) {
    const checkout = argv[fromAt + 1] ?? fail('--from needs a path to a create-bridgething checkout');
    console.log(`vendored the skill from ${checkout} at ${await vendorLocal(checkout)}`);
    console.log('a later "bun run skills" replaces it with whatever is published.');
    for (const problem of await toolkitDrift(await installedStamp())) console.log(problem);
    return;
  }

  if (argv.includes('--check')) {
    const [stamp, latest] = await Promise.all([installedStamp(), latestVersion()]);
    if (!stamp) {
      console.log(`no skill installed; run "bun run skills" to fetch ${PACKAGE}@${latest}`);
      process.exit(1);
    }
    const behind = skillBehind(stamp, latest);
    const problems = [...(behind ? [behind] : []), ...(await toolkitDrift(stamp))];
    if (!problems.length) {
      console.log(
        stamp.local
          ? `skill is vendored from a checkout at ${stamp.version}; published is ${latest}`
          : `skill is current (${PACKAGE}@${stamp.version})`,
      );
      return;
    }
    for (const problem of problems) console.log(problem);
    process.exit(1);
  }

  const version = await latestVersion();
  const stamp = await installedStamp();
  await download(version);
  const had = stamp?.version ?? 'none';
  console.log(had === version ? `refreshed the skill at ${version}` : `skill ${had} -> ${version}`);
  console.log('.claude/skills/bridgething is shared by every app in apps/; commit the change.');
  for (const problem of await toolkitDrift(await installedStamp())) console.log(problem);
}
