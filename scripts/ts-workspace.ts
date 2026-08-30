import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, posix } from 'node:path';

export const ROOT = join(import.meta.dir, '..');
export const READ_BY_TSC = ['.ts', '.tsx', '.mts', '.cts', '.astro'];

export type PackageJson = { name?: string; scripts?: Record<string, string>; workspaces?: string[] };
export type Member = { name: string; dir: string; scripts: Record<string, string>; reads: string[] };

type TsConfig = { include?: string[]; exclude?: string[] };

export function read<T>(path: string): T {
  return JSON.parse(readFileSync(join(ROOT, path), 'utf8')) as T;
}

export function hashed(dir: string): string[] {
  const args = ['ls-files', '--cached', '--others', '--exclude-standard', '-z', '--', dir];
  const result = spawnSync('git', args, { cwd: ROOT, encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`git ls-files failed for ${dir}: ${result.stderr}`);
  return result.stdout
    .split('\0')
    .filter(path => path.length > 0 && existsSync(join(ROOT, path)))
    .map(path => posix.relative(dir, path));
}

function projects(dir: string, script: string): string[] {
  const found = new Set<string>();
  for (const segment of script.split('&&')) {
    const words = segment.trim().split(/\s+/).filter(Boolean);
    if (words.length === 0) continue;
    const flag = words.findIndex(word => word === '-p' || word === '--project');
    const named = flag >= 0 ? words[flag + 1] : words.find(word => word.startsWith('--project='))?.slice(10);
    if (named === undefined) {
      found.add('tsconfig.json');
      continue;
    }
    const path = join(dir, named);
    found.add(posix.join(named, existsSync(path) && statSync(path).isDirectory() ? 'tsconfig.json' : ''));
  }
  return [...found];
}

function expand(entry: string): string {
  return /[*.]/.test(entry) ? entry : posix.join(entry, '**', '*');
}

export function readsOf(dir: string, script: string): string[] {
  const files = hashed(dir);
  const reads = new Set<string>();
  for (const project of projects(dir, script)) {
    const configPath = join(ROOT, dir, project);
    if (!existsSync(configPath)) continue;
    reads.add(project);
    const config = JSON.parse(readFileSync(configPath, 'utf8')) as TsConfig;
    const base = posix.dirname(project) === '.' ? '' : posix.dirname(project);
    const includes = (config.include ?? ['**/*']).map(entry => new Bun.Glob(expand(entry)));
    const excludes = (config.exclude ?? []).map(entry => new Bun.Glob(expand(entry)));
    for (const file of files) {
      if (base.length > 0 && !file.startsWith(`${base}/`)) continue;
      const inner = base.length > 0 ? file.slice(base.length + 1) : file;
      if (!READ_BY_TSC.some(extension => inner.endsWith(extension))) continue;
      if (!includes.some(glob => glob.match(inner))) continue;
      if (excludes.some(glob => glob.match(inner))) continue;
      reads.add(file);
    }
  }
  return [...reads];
}

export function manifests(): string[] {
  const found = new Set<string>();
  for (const pattern of read<PackageJson>('package.json').workspaces ?? []) {
    for (const path of new Bun.Glob(`${pattern}/package.json`).scanSync({ cwd: ROOT, onlyFiles: true })) {
      found.add(path);
    }
  }
  return [...found].sort();
}

export function members(): Member[] {
  const found: Member[] = [];
  for (const manifest of manifests()) {
    const dir = dirname(manifest);
    const pkg = read<PackageJson>(manifest);
    const scripts = pkg.scripts ?? {};
    const typecheck = scripts['typecheck'];
    found.push({ name: pkg.name ?? dir, dir, scripts, reads: typecheck === undefined ? [] : readsOf(dir, typecheck) });
  }
  return found.sort((a, b) => a.name.localeCompare(b.name));
}
