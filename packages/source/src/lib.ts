import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, readdir, readFile, rm, stat, utimes, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { appsDir, fail, SOURCE_FILE, sourceRoot } from './paths.ts';

export interface RecommendedSource {
  name: string;
  url: string;
  description: string | null;
  attested: boolean;
}

export interface SourceConfig {
  name: string;
  description: string;
  homepage: string | null;
  icon: string | null;
  base_url: string | null;
  recommended_sources: RecommendedSource[];
}

export interface AppMeta {
  author: string;
  homepage: string | null;
  source: string | null;
  icon: string | null;
  screenshots: string[];
  min_libbridgething_version: string;
}

export interface Manifest {
  id: string;
  name: string;
  version: string;
  description: string;
  icon?: string;
  settings?: string;
  role?: string;
  overlay?: string;
  permissions?: string[];
  extension?: { entry: string; permissions: string[]; api: number };
}

export interface App {
  slug: string;
  dir: string;
  manifest: Manifest;
  meta: AppMeta;
}

export interface LedgerVersion {
  released_at: string;
  sha256: string;
  size: number;
  file: string;
  settings?: { file: string; size: number; sha256: string };
}

export type Ledger = Record<string, Record<string, LedgerVersion>>;

export async function readJson<T>(path: string): Promise<T> {
  return JSON.parse(await readFile(path, 'utf8')) as T;
}

export async function writeJson(path: string, value: unknown): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

export async function writeText(path: string, value: string): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, value);
}

export async function writeBytes(path: string, value: Uint8Array): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, value);
}

export async function readSource(): Promise<SourceConfig> {
  const path = join(sourceRoot(), SOURCE_FILE);
  const raw = await readJson<Partial<SourceConfig>>(path);
  for (const field of ['name', 'description'] as const) {
    if (!raw[field]) fail(`${SOURCE_FILE} is missing "${field}"`);
  }
  return {
    name: raw.name!,
    description: raw.description!,
    homepage: raw.homepage ?? null,
    icon: raw.icon ?? null,
    base_url: raw.base_url ?? null,
    recommended_sources: raw.recommended_sources ?? [],
  };
}

export function publicBase(source: SourceConfig): string {
  const configured = source.base_url || process.env.CATALOG_BASE_URL || null;
  if (configured) return configured.replace(/\/+$/, '');

  const repository = process.env.GITHUB_REPOSITORY || githubRemote();
  if (!repository) fail(`cannot figure out where this catalog will be served. set "base_url" in ${SOURCE_FILE}`);
  const [owner, repo] = repository.split('/');
  if (!owner || !repo) fail(`"${repository}" is not owner/repo`);
  return `https://${owner.toLowerCase()}.github.io/${repo}`;
}

export function githubRemote(): string | null {
  const remote = capture('git', ['remote', 'get-url', 'origin']);
  const match = remote ? /github\.com[:/]([^/]+)\/(.+?)(?:\.git)?$/.exec(remote) : null;
  return match ? `${match[1]}/${match[2]}` : null;
}

export async function listApps(): Promise<App[]> {
  const dir = appsDir();
  if (!existsSync(dir)) return [];
  const entries = await readdir(dir, { withFileTypes: true });
  const apps: App[] = [];
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
    if (!existsSync(join(dir, entry.name, 'public', 'manifest.json'))) continue;
    apps.push(await readApp(entry.name));
  }
  return apps;
}

export async function readApp(slug: string): Promise<App> {
  const dir = join(appsDir(), slug);
  const manifestPath = join(dir, 'public', 'manifest.json');
  if (!existsSync(manifestPath)) fail(`apps/${slug} has no public/manifest.json`);
  const manifest = await readJson<Manifest>(manifestPath);
  for (const field of ['id', 'name', 'version'] as const)
    if (!manifest[field]) fail(`apps/${slug}/public/manifest.json is missing "${field}"`);

  const metaPath = join(dir, 'catalog.json');
  if (!existsSync(metaPath)) fail(`apps/${slug} has no catalog.json`);
  const raw = await readJson<Partial<AppMeta>>(metaPath);
  if (!raw.author) fail(`apps/${slug}/catalog.json is missing "author"`);
  if (!raw.min_libbridgething_version) fail(`apps/${slug}/catalog.json is missing "min_libbridgething_version"`);

  return {
    slug,
    dir,
    manifest,
    meta: {
      author: raw.author,
      homepage: raw.homepage ?? null,
      source: raw.source ?? null,
      icon: raw.icon ?? null,
      screenshots: raw.screenshots ?? [],
      min_libbridgething_version: raw.min_libbridgething_version,
    },
  };
}

export async function requireApp(slug: string): Promise<App> {
  if (!existsSync(join(appsDir(), slug))) {
    const known = (await listApps()).map(a => a.slug);
    fail(`no app "${slug}"${known.length ? `; this source has ${known.join(', ')}` : ' and apps/ is empty'}`);
  }
  return readApp(slug);
}

export async function readChangelog(app: App): Promise<Record<string, string>> {
  const path = join(app.dir, 'CHANGELOG.md');
  if (!existsSync(path)) return {};
  const sections: Record<string, string> = {};
  let version: string | null = null;
  let body: string[] = [];
  const flush = () => {
    if (version) sections[version] = body.join('\n').trim();
    body = [];
  };
  for (const line of (await readFile(path, 'utf8')).split('\n')) {
    const heading = /^##\s+v?([^\s]+)\s*$/.exec(line);
    if (heading) {
      flush();
      version = heading[1]!;
      continue;
    }
    if (version) body.push(line);
  }
  flush();
  return sections;
}

export interface RunOptions {
  cwd?: string;
  env?: Record<string, string>;
}

function options(given: string | RunOptions): { cwd: string; env: NodeJS.ProcessEnv } {
  const { cwd = sourceRoot(), env } = typeof given === 'string' ? { cwd: given, env: undefined } : given;
  return { cwd, env: env ? { ...process.env, ...env } : process.env };
}

export function run(cmd: string, args: string[], given: string | RunOptions = {}): void {
  const result = spawnSync(cmd, args, { stdio: 'inherit', ...options(given) });
  if (result.error) throw result.error;
  if (result.status !== 0) fail(`${cmd} ${args.join(' ')} exited with ${result.status}`);
}

export function capture(cmd: string, args: string[], given: string | RunOptions = {}): string | null {
  const result = spawnSync(cmd, args, { encoding: 'utf8', ...options(given) });
  if (result.status !== 0) return null;
  return result.stdout.trim();
}

export async function sha256(path: string): Promise<string> {
  return createHash('sha256')
    .update(await readFile(path))
    .digest('hex');
}

export interface Bundle {
  zip: string;
  size: number;
  sha256: string;
  iconPath: string | null;
  iconExt: string | null;
  settingsPath: string | null;
  settingsSize: number;
  settingsSha256: string | null;
}

const EPOCH = new Date('2020-01-01T00:00:00Z');

async function flattenTimestamps(dir: string): Promise<void> {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) await flattenTimestamps(path);
    await utimes(path, EPOCH, EPOCH);
  }
}

export async function bundle(app: App, output: string): Promise<Bundle> {
  run('bun', ['run', 'build'], app.dir);

  const dist = join(app.dir, 'dist');
  const entries = (await readdir(dist).catch(() => [])).filter(e => !e.startsWith('.'));
  if (!entries.length) fail(`apps/${app.slug} built nothing into dist/`);
  if (!entries.includes('index.html')) fail(`apps/${app.slug}/dist has no index.html`);

  const built = await readJson<Manifest>(join(dist, 'manifest.json')).catch(() => {
    fail(`apps/${app.slug}/dist has no manifest.json`);
  });
  if (built.version !== app.manifest.version)
    fail(`apps/${app.slug} built version ${built.version} but public/manifest.json says ${app.manifest.version}`);

  if (built.extension?.entry) {
    const entry = join(dist, built.extension.entry);
    if (!(await stat(entry).catch(() => null))?.isFile())
      fail(`apps/${app.slug} declares extension entry "${built.extension.entry}" which is not in the bundle`);
  }

  let settingsPath: string | null = null;
  let settingsSize = 0;
  let settingsSha256: string | null = null;
  if (built.settings) {
    const candidate = join(dist, built.settings);
    const found = await stat(candidate).catch(() => null);
    if (!found?.isFile())
      fail(`apps/${app.slug} declares settings page "${built.settings}" which is not in the bundle`);
    settingsPath = candidate;
    settingsSize = found.size;
    settingsSha256 = await sha256(candidate);
  }

  let iconPath: string | null = null;
  let iconExt: string | null = null;
  if (built.icon) {
    const candidate = join(dist, built.icon);
    if (!(await stat(candidate).catch(() => null))?.isFile())
      fail(`apps/${app.slug} declares icon "${built.icon}" which is not in the bundle`);

    iconPath = candidate;
    iconExt = built.icon.split('.').pop()!.toLowerCase();
  }

  await mkdir(dirname(output), { recursive: true });
  await rm(output, { force: true });
  await flattenTimestamps(dist);
  run('zip', ['-q', '-X', '-r', '-D', output, ...entries.sort()], dist);

  return {
    zip: output,
    size: (await stat(output)).size,
    sha256: await sha256(output),
    iconPath,
    iconExt,
    settingsPath,
    settingsSize,
    settingsSha256,
  };
}
