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
import { extname, join } from 'node:path';

export type Substitutions = Record<string, string>;

const RENAMES: Record<string, string> = {
  _gitignore: '.gitignore',
  _claude: '.claude',
  _github: '.github',
  _bunversion: '.bun-version',
  '_prettierrc.js': '.prettierrc.js',
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

export function substitute(raw: string, subs: Substitutions): string {
  return raw.replace(/__[A-Z0-9_]+__/g, token => subs[token] ?? token);
}

export function copyTemplate(src: string, dest: string, subs: Substitutions): void {
  mkdirSync(dest, { recursive: true });
  for (const entry of readdirSync(src)) {
    const srcPath = join(src, entry);
    const destPath = join(dest, RENAMES[entry] ?? entry);
    if (statSync(srcPath).isDirectory()) {
      copyTemplate(srcPath, destPath, subs);
    } else if (BINARY_EXT.has(extname(entry).toLowerCase())) {
      copyFileSync(srcPath, destPath);
    } else {
      writeFileSync(destPath, substitute(readFileSync(srcPath, 'utf8'), subs));
    }
  }
}

export function copyDir(src: string, dest: string): void {
  mkdirSync(dest, { recursive: true });
  for (const entry of readdirSync(src)) {
    const srcPath = join(src, entry);
    const destPath = join(dest, entry);
    if (statSync(srcPath).isDirectory()) copyDir(srcPath, destPath);
    else copyFileSync(srcPath, destPath);
  }
}

export function move(from: string, to: string): void {
  if (!existsSync(from)) return;
  rmSync(to, { recursive: true, force: true });
  mkdirSync(join(to, '..'), { recursive: true });
  copyDir(from, to);
  rmSync(from, { recursive: true, force: true });
}

export function linkOrCopy(target: string, linkPath: string, realPath: string): void {
  rmSync(linkPath, { recursive: true, force: true });
  mkdirSync(join(linkPath, '..'), { recursive: true });
  try {
    symlinkSync(target, linkPath, 'dir');
  } catch {
    copyDir(realPath, linkPath);
  }
}

export function readJson<T>(path: string): T {
  return JSON.parse(readFileSync(path, 'utf8')) as T;
}

export function writeJson(path: string, value: unknown): void {
  mkdirSync(join(path, '..'), { recursive: true });
  writeFileSync(path, `${JSON.stringify(value, null, 2)}\n`);
}

export function patchJson(path: string, patch: Record<string, unknown>): void {
  writeJson(path, { ...readJson<Record<string, unknown>>(path), ...patch });
}

export function sortKeys(record: Record<string, string>): Record<string, string> | undefined {
  const entries = Object.entries(record).sort(([a], [b]) => a.localeCompare(b));
  return entries.length ? Object.fromEntries(entries) : undefined;
}
