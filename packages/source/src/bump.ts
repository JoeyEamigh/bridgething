import { existsSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { listApps, readApp, readJson, requireApp, writeJson, writeText, type App, type Manifest } from './lib.ts';
import { fail } from './paths.ts';

const RELEASES = ['major', 'minor', 'patch'] as const;
type Release = (typeof RELEASES)[number];

export const BUMP_USAGE = `Usage: bun run bump <slug | --all> <major | minor | patch | x.y.z> [-m "note"]

Moves an app to its next version, writing public/manifest.json, package.json, and adding a CHANGELOG.md section.`;

function next(current: string, release: Release): string {
  const match = /^(\d+)\.(\d+)\.(\d+)/.exec(current);
  if (!match) fail(`cannot ${release}-bump "${current}": not semver`);
  const [major, minor, patch] = match.slice(1, 4).map(Number) as [number, number, number];
  if (release === 'major') return `${major + 1}.0.0`;
  if (release === 'minor') return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

async function bumpOne(app: App, target: string, note: string | null): Promise<void> {
  const manifestPath = join(app.dir, 'public', 'manifest.json');
  const manifest = await readJson<Manifest>(manifestPath);
  await writeJson(manifestPath, { ...manifest, version: target });

  const pkgPath = join(app.dir, 'package.json');
  if (existsSync(pkgPath)) {
    const pkg = await readJson<Record<string, unknown>>(pkgPath);
    await writeJson(pkgPath, { ...pkg, version: target });
  }

  const changelogPath = join(app.dir, 'CHANGELOG.md');
  const body = existsSync(changelogPath) ? await readFile(changelogPath, 'utf8') : `# ${manifest.name}\n`;
  const [heading, ...rest] = body.split(/\n(?=## )/);
  const section = `## ${target}\n\n${note ?? 'description of what changed'}\n`;
  await writeText(changelogPath, [heading!.trimEnd(), '', section, ...rest].join('\n').replace(/\n{3,}/g, '\n\n'));

  console.log(`${app.slug}  ${app.manifest.version} -> ${target}`);
}

export async function bump(argv: string[]): Promise<void> {
  const noteAt = argv.findIndex(a => a === '-m' || a === '--message');
  const note = noteAt === -1 ? null : (argv[noteAt + 1] ?? null);
  const positional = argv.filter((_, i) => noteAt === -1 || (i !== noteAt && i !== noteAt + 1));

  const [which, release] = positional;
  if (!which || !release) {
    console.log(BUMP_USAGE);
    process.exit(1);
  }

  const isRelease = (RELEASES as readonly string[]).includes(release);
  if (!isRelease && !/^[A-Za-z0-9](?:[A-Za-z0-9.+-]*[A-Za-z0-9])?$/.test(release)) {
    fail(`"${release}" is neither ${RELEASES.join('/')} nor a usable version string`);
  }

  const apps = which === '--all' ? await listApps() : [await requireApp(which)];
  if (!apps.length) fail('apps/ is empty, run "bun run new <slug>" first');
  if (!isRelease && apps.length > 1) fail('an explicit version can only be given to one app');

  for (const app of apps) {
    const fresh = await readApp(app.slug);
    await bumpOne(fresh, isRelease ? next(fresh.manifest.version, release as Release) : release, note);
  }

  console.log('\nedit the new CHANGELOG.md section, then push to main to publish.');
}
