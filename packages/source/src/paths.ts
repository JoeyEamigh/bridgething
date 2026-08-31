import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';

export const LEDGER_FILE = 'published.json';
export const CATALOG_FILE = 'catalog.v1.json';
export const SOURCE_FILE = 'source.json';

export class UserError extends Error {}

export function fail(message: string): never {
  throw new UserError(message);
}

let cachedRoot: string | null = null;

export function sourceRoot(): string {
  if (cachedRoot) return cachedRoot;
  let dir = resolve(process.env.BRIDGETHING_SOURCE_ROOT ?? process.cwd());
  for (;;) {
    if (existsSync(join(dir, SOURCE_FILE)) && existsSync(join(dir, 'apps'))) {
      cachedRoot = dir;
      return dir;
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  fail(`cannot find ${SOURCE_FILE} beside an apps/ directory`);
}

export function appsDir(): string {
  return join(sourceRoot(), 'apps');
}

export function siteDir(): string {
  return join(sourceRoot(), 'site');
}
