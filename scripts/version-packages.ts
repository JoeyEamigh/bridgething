#!/usr/bin/env bun
import { readFileSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { PACKAGES, SCAFFOLD_DEPENDENCIES, daemonVersion, packageVersion, type Pkg } from './publish-packages.ts';

const ROOT = join(import.meta.dir, '..');

const SCAFFOLD_VERSIONS = 'packages/create-bridgething/template-versions.json';

const RELEASES = ['major', 'minor', 'patch'] as const;
type Release = (typeof RELEASES)[number];

function usage(): never {
  const own = PACKAGES.filter(p => !p.daemonDerived);
  console.log(`Usage: bun scripts/version-packages.ts <package | --all> <major | minor | patch | x.y.z>

Moves a published package to its next version. These own their versions and ship
independently of the daemon:

${own.map(p => `  ${p.name}`).join('\n')}

@bridgething/lib and @bridgething/core-node are not here: they mirror rust crates
and take the cargo workspace version, because LIBBRIDGETHING_VERSION is the wire
surface every min_libbridgething_version in a catalog is compared against.

A package may be named in full or by the last segment, so "client" is enough.`);
  process.exit(1);
}

function resolvePackage(which: string): Pkg {
  const match = PACKAGES.find(p => p.name === which || basename(p.name) === which || p.dir.endsWith(`/${which}`));
  if (!match) {
    console.error(`error: no package "${which}"`);
    usage();
  }
  if (match.daemonDerived) {
    console.error(
      `error: ${match.name} mirrors a rust crate and takes the cargo workspace version.\n` +
        '       Bump [workspace.package] version in Cargo.toml instead.',
    );
    process.exit(1);
  }
  return match;
}

function next(current: string, release: Release): string {
  const match = /^(\d+)\.(\d+)\.(\d+)/.exec(current);
  if (!match) {
    console.error(`error: cannot ${release}-bump "${current}"; pass the next version explicitly`);
    process.exit(1);
  }
  const [major, minor, patch] = match.slice(1, 4).map(Number) as [number, number, number];
  if (release === 'major') return `${major + 1}.0.0`;
  if (release === 'minor') return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

function setVersion(dir: string, version: string): void {
  const path = join(ROOT, dir, 'package.json');
  const src = readFileSync(path, 'utf8');
  writeFileSync(path, src.replace(/("version"\s*:\s*")[^"]+(")/, `$1${version}$2`));
}

function syncScaffoldPins(): void {
  const path = join(ROOT, SCAFFOLD_VERSIONS);
  const pins = JSON.parse(readFileSync(path, 'utf8')) as Record<string, string>;
  for (const name of SCAFFOLD_DEPENDENCIES) {
    const pkg = PACKAGES.find(p => p.name === name);
    if (!pkg) continue;
    const pin = `^${packageVersion(pkg.dir)}`;
    if (pins[name] !== pin) console.log(`  scaffold pin ${name}: ${pins[name]} -> ${pin}`);
    pins[name] = pin;
  }
  // the scaffold writes this into every app's min_libbridgething_version, and it is the wire-surface
  // version a catalog is filtered against, so it tracks the cargo workspace and never a package.
  const daemon = daemonVersion();
  if (pins.libbridgething !== daemon) console.log(`  scaffold pin libbridgething: ${pins.libbridgething} -> ${daemon}`);
  pins.libbridgething = daemon;

  writeFileSync(path, `${JSON.stringify(pins, null, 2)}\n`);
}

function main(): void {
  const [which, bump] = process.argv.slice(2);
  if (!which || !bump) usage();

  const isRelease = (RELEASES as readonly string[]).includes(bump);
  if (!isRelease && !/^\d+\.\d+\.\d+(?:[.+-][0-9A-Za-z.-]+)?$/.test(bump)) {
    console.error(`error: "${bump}" is neither ${RELEASES.join('/')} nor a semver version`);
    process.exit(1);
  }

  const targets = which === '--all' ? PACKAGES.filter(p => !p.daemonDerived) : [resolvePackage(which)];

  for (const pkg of targets) {
    const current = packageVersion(pkg.dir);
    const target = isRelease ? next(current, bump as Release) : bump;
    setVersion(pkg.dir, target);
    console.log(`${pkg.name}: ${current} -> ${target}`);
  }

  syncScaffoldPins();

  const install = Bun.spawnSync(['bun', 'install'], { cwd: ROOT, stdout: 'inherit', stderr: 'inherit' });
  if (install.exitCode !== 0) {
    console.error('\nbun install failed; bun.lock may be out of step with the new versions');
    process.exit(install.exitCode ?? 1);
  }

  console.log('\ncommit the bumped package.json, template-versions.json and bun.lock.');
  console.log('publish-packages publishes any package whose version is not on the registry.');
}

if (import.meta.main) main();
