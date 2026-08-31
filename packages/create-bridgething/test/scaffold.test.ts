import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { spawnSync, type SpawnSyncReturns } from 'node:child_process';
import {
  existsSync,
  lstatSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const packageDir = resolve(import.meta.dir, '..');
const entry = join(packageDir, 'src', 'index.ts');
const pins = JSON.parse(readFileSync(join(packageDir, 'template-versions.json'), 'utf8')) as Record<string, string>;

const SLUG = 'my-app';
const REPO = 'tester/my-app';

type Manifest = {
  id: string;
  name: string;
  version: string;
  description: string;
  role?: string;
  overlay?: string;
  config: { type: string; data: { key: string; label: string } }[];
  extension?: { entry: string; permissions: string[]; api: number };
};

type Package = {
  name: string;
  scripts: Record<string, string>;
  dependencies: Record<string, string>;
  devDependencies: Record<string, string>;
};

type Listing = {
  author: string;
  homepage: string | null;
  source: string | null;
  icon: string | null;
  screenshots: string[];
  min_libbridgething_version: string;
};

class Source {
  constructor(
    readonly root: string,
    readonly slug: string = SLUG,
  ) {}

  get dir(): string {
    return join(this.root, 'apps', this.slug);
  }

  path(...parts: string[]): string {
    return join(this.dir, ...parts);
  }

  has(...parts: string[]): boolean {
    return existsSync(this.path(...parts));
  }

  read(...parts: string[]): string {
    return readFileSync(this.path(...parts), 'utf8');
  }

  hasRoot(...parts: string[]): boolean {
    return existsSync(join(this.root, ...parts));
  }

  readRoot(...parts: string[]): string {
    return readFileSync(join(this.root, ...parts), 'utf8');
  }

  json<T>(...parts: string[]): T {
    return JSON.parse(this.read(...parts)) as T;
  }

  rootJson<T>(...parts: string[]): T {
    return JSON.parse(this.readRoot(...parts)) as T;
  }

  manifest(): Manifest {
    return this.json<Manifest>('public', 'manifest.json');
  }

  pkg(): Package {
    return this.json<Package>('package.json');
  }

  listing(): Listing {
    return this.json<Listing>('catalog.json');
  }

  write(contents: string, ...parts: string[]): void {
    writeFileSync(this.path(...parts), contents);
  }

  lendModules(): void {
    symlinkSync(join(packageDir, 'node_modules'), this.path('node_modules'), 'dir');
  }
}

const roots: string[] = [];

function create(flags: string[], cwd = packageDir, target: string | null = null): SpawnSyncReturns<string> {
  return spawnSync('bun', [entry, ...(target ? [target] : []), '--yes', '--no-install', '--no-git', ...flags], {
    cwd,
    encoding: 'utf8',
  });
}

function scaffold(...flags: string[]): Source {
  const root = mkdtempSync(join(tmpdir(), 'create-bridgething-'));
  roots.push(root);
  const target = join(root, SLUG);
  const result = create(['--repo', REPO, ...flags], packageDir, target);
  if (result.status !== 0) throw new Error(`scaffold failed: ${result.stdout}\n${result.stderr}`);
  return new Source(target);
}

let plain: Source;
let extension: Source;

beforeAll(() => {
  plain = scaffold();
  extension = scaffold('--extension');
});

afterAll(() => {
  for (const root of roots) rmSync(root, { recursive: true, force: true });
});

describe('the source it scaffolds', () => {
  test('describes itself and the pages site it publishes to', () => {
    const source = plain.rootJson<{ name: string; description: string; homepage: string; base_url: string }>(
      'source.json',
    );
    expect(source.name).toBe(SLUG);
    expect(source.description).toBeTruthy();
    expect(source.homepage).toBe(`https://github.com/${REPO}`);
    expect(source.base_url).toBe('https://tester.github.io/my-app');
  });

  test('is a bun workspace whose scripts drive the published toolkit', () => {
    const pkg = plain.rootJson<Package & { workspaces: string[] }>('package.json');
    expect(pkg.workspaces).toEqual(['apps/*']);
    expect(pkg.devDependencies['@bridgething/source']).toBe(pins['@bridgething/source']);
    expect(pkg.scripts.new).toBe('bunx create-bridgething');
    expect(pkg.scripts.check).toBe('bridgething-source check');
    expect(pkg.scripts.publish).toBe('bridgething-source publish');
    expect(pkg.scripts.dev).toBe('bridgething-source run dev');
  });

  test('ships the gate and the release workflow', () => {
    expect(plain.readRoot('.github', 'workflows', 'ci.yml')).toContain('bun run check');
    expect(plain.readRoot('.github', 'workflows', 'publish.yml')).toContain('bun run publish');
    expect(plain.readRoot('.bun-version').trim()).toMatch(/^\d+\.\d+\.\d+$/);
    expect(plain.readRoot('.gitignore')).toContain('site');
  });

  test('leaves no unsubstituted tokens anywhere', () => {
    for (const file of ['source.json', 'package.json', 'README.md', 'CLAUDE.md']) {
      expect(plain.readRoot(file)).not.toMatch(/__[A-Z0-9_]+__/);
    }
  });
});

describe('the app it scaffolds', () => {
  test('lands under apps/ named after the target', () => {
    expect(plain.has('package.json')).toBe(true);
    expect(plain.pkg().name).toBe(SLUG);
    expect(plain.manifest().name).toBe(SLUG);
  });

  test('gets a uuidv7 the device keys install and storage on', () => {
    expect(plain.manifest().id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(scaffold().manifest().id).not.toBe(plain.manifest().id);
  });

  test('carries a store listing filled from git and the repo it was told about', () => {
    const listing = plain.listing();
    expect(listing.homepage).toBe(`https://github.com/${REPO}`);
    expect(listing.source).toBe(`https://github.com/${REPO}`);
    expect(listing.min_libbridgething_version).toBe(pins.libbridgething);
    expect(listing.screenshots).toEqual([]);
  });

  test('opens a changelog section for the version the manifest carries', () => {
    expect(plain.read('CHANGELOG.md')).toContain(`## ${plain.manifest().version}`);
  });

  test('only includes tsconfig paths that exist', () => {
    expect(plain.json<{ include: string[] }>('tsconfig.json').include).toEqual(['src', 'settings']);
    expect(extension.json<{ include: string[] }>('tsconfig.json').include).toEqual(['src', 'settings', 'extension']);
  });

  test('pins the client at the version this scaffold ships against', () => {
    expect(plain.pkg().dependencies['@bridgething/client']).toBe(pins['@bridgething/client']);
  });
});

describe('the agent guides', () => {
  test('the skill lives once at the source root and every app links to it', () => {
    expect(plain.hasRoot('.claude', 'skills', 'bridgething', 'SKILL.md')).toBe(true);
    expect(lstatSync(plain.path('.claude', 'skills', 'bridgething')).isSymbolicLink()).toBe(true);
    expect(realpathSync(plain.path('.claude', 'skills', 'bridgething'))).toBe(
      realpathSync(join(plain.root, '.claude', 'skills', 'bridgething')),
    );
  });

  test('agents that read AGENTS.md find the same skill, at the root and in an app', () => {
    for (const dir of [plain.root, plain.dir]) {
      expect(lstatSync(join(dir, '.agents', 'skills')).isSymbolicLink()).toBe(true);
      expect(realpathSync(join(dir, '.agents', 'skills', 'bridgething'))).toBe(
        realpathSync(join(plain.root, '.claude', 'skills', 'bridgething')),
      );
    }
  });

  test('the shared guide is the source root one, and a plain app adds nothing', () => {
    expect(plain.readRoot('CLAUDE.md')).toContain('Building bridgething webapps in this repo');
    expect(plain.hasRoot('AGENTS.md')).toBe(true);
    expect(plain.has('CLAUDE.md')).toBe(false);
  });

  test('the extension skill page ships with every source', () => {
    expect(plain.hasRoot('.claude', 'skills', 'bridgething', 'reference', 'extension.md')).toBe(true);
    expect(plain.readRoot('.claude', 'skills', 'bridgething', 'SKILL.md')).toContain('reference/extension.md');
  });

  test('the scaffolded docs name every surface the SDK ships', () => {
    const surfaces = resolve(packageDir, '..', '..', 'crates', 'lib', 'docs', 'surfaces.json');
    const shipped = (JSON.parse(readFileSync(surfaces, 'utf8')) as { surfaces: { name: string }[] }).surfaces
      .map(surface => surface.name)
      .sort();

    const sdk = plain.readRoot('.claude', 'skills', 'bridgething', 'reference', 'sdk.md');
    expect([...sdk.matchAll(/^\| `([a-z]+)` \|/gm)].map(row => row[1]!).sort()).toEqual(shipped);

    const listed = /Every surface: `([^`]+)`/.exec(plain.readRoot('CLAUDE.md'));
    expect(listed?.[1]?.split(/\s+/).sort()).toEqual(shipped);
  });

  test('no scaffolded doc hard-codes a surface count that goes stale', () => {
    const docs = [
      plain.readRoot('CLAUDE.md'),
      plain.readRoot('.claude', 'skills', 'bridgething', 'SKILL.md'),
      plain.readRoot('.claude', 'skills', 'bridgething', 'reference', 'sdk.md'),
    ];

    for (const doc of docs) {
      expect(doc).not.toMatch(/\b\d+[\s-]surfaces?\b/i);
      expect(doc).not.toMatch(/\b(?:all|full|every|only)\s+\d+\b/i);
    }
  });

  test('the extension reference only claims what the builders verified', () => {
    const reference = plain.readRoot('.claude', 'skills', 'bridgething', 'reference', 'extension.md');
    expect(reference).toContain('unix sockets');
    expect(reference).not.toContain('named pipe');
  });
});

describe('adding an app to an existing source', () => {
  test('scaffolds into apps/ and reuses the source-level skill', () => {
    const source = plain;
    const result = create([], source.root, 'second');
    expect(result.status).toBe(0);

    const second = new Source(source.root, 'second');
    expect(second.pkg().name).toBe('second');
    expect(second.manifest().id).not.toBe(source.manifest().id);
    expect(lstatSync(second.path('.claude', 'skills', 'bridgething')).isSymbolicLink()).toBe(true);
    expect(second.hasRoot('source.json')).toBe(true);
    expect(second.rootJson<{ name: string }>('source.json').name).toBe(SLUG);
  });

  test('takes the repo url from the source it joined, before any push', () => {
    const result = create([], plain.root, 'third');
    expect(result.status).toBe(0);
    expect(new Source(plain.root, 'third').listing().source).toBe(`https://github.com/${REPO}`);
  });
});

describe('without --extension', () => {
  test('nothing extension-shaped lands', () => {
    expect(plain.has('extension', 'main.ts')).toBe(false);
    expect(plain.has('scripts', 'bridgething.ts')).toBe(true);
    expect(plain.manifest().extension).toBeUndefined();
    expect(plain.pkg().scripts.build).toBe('vite build && vite build -c vite.settings.config.ts');
    expect(plain.pkg().devDependencies.deno).toBeUndefined();
    expect(plain.pkg().dependencies['@bridgething/extension']).toBeUndefined();
  });
});

describe('--extension', () => {
  test('copies the extension sources with substitutions applied', () => {
    expect(extension.has('extension', 'main.ts')).toBe(true);

    const main = extension.read('extension', 'main.ts');
    expect(main).toContain("from '@bridgething/extension'");
    expect(main).toContain('my-app extension up');
    expect(main).not.toContain('__PROJECT_NAME__');
  });

  test('writes the manifest block the daemon parses', () => {
    expect(extension.manifest().extension).toEqual({
      entry: 'extension/desktop.mjs',
      permissions: ['all'],
      api: 1,
    });
  });

  test('gives the example a config key to read', () => {
    expect(extension.manifest().config).toEqual([
      { type: 'string', data: { key: 'greeting', label: 'Greeting the extension sends on connect' } },
    ]);
  });

  test('adds the dependency and the runtime, and leaves the scripts alone', () => {
    const pkg = extension.pkg();
    expect(pkg.dependencies['@bridgething/extension']).toBe(pins['@bridgething/extension']);
    expect(pkg.dependencies['@bridgething/client']).toBe(pins['@bridgething/client']);
    expect(pkg.devDependencies.esbuild).toBe('^0.28.2');
    expect(pkg.devDependencies.deno).toBe('2.9.6');
    expect(pkg.devDependencies.vite).toBeDefined();
    expect(pkg.scripts).toEqual(plain.pkg().scripts);
  });

  test('writes the extension guide as the app-level CLAUDE.md and leaves no marker file', () => {
    const claude = extension.read('CLAUDE.md');
    expect(claude).toStartWith('# This project ships a native extension');
    expect(extension.has('_claude_append.md')).toBe(false);
    expect(extension.has('AGENTS.md')).toBe(true);
  });
});

describe('--extension composed with a variant', () => {
  test('with --launcher, both the role and the extension land', () => {
    const app = scaffold('--launcher', '--extension');
    const manifest = app.manifest();
    expect(manifest.role).toBe('launcher');
    expect(manifest.extension?.entry).toBe('extension/desktop.mjs');
    expect(app.has('extension', 'main.ts')).toBe(true);
    expect(app.pkg().scripts.build).toBe('vite build && vite build -c vite.settings.config.ts');

    const claude = app.read('CLAUDE.md');
    expect(claude).toStartWith('# This project is a launcher');
    expect(claude).toContain('This project ships a native extension');
    expect(app.has('_claude_append.md')).toBe(false);
  });

  test('with --overlay, the overlay build and the extension land together', () => {
    const app = scaffold('--overlay', '--extension');
    const manifest = app.manifest();
    expect(manifest.overlay).toBe('overlay.js');
    expect(manifest.extension?.api).toBe(1);
    expect(app.has('overlay', 'main.tsx')).toBe(true);
    expect(app.pkg().scripts.build).toBe(
      'vite build && vite build -c vite.settings.config.ts && vite build -c vite.overlay.config.ts',
    );

    const claude = app.read('CLAUDE.md');
    expect(claude).toContain('This project is a system overlay');
    expect(claude).toContain('This project ships a native extension');
  });

  test('flag order does not matter', () => {
    const app = scaffold('--extension', '--launcher');
    expect(app.manifest().role).toBe('launcher');
    expect(app.manifest().extension?.api).toBe(1);
  });
});

describe('help', () => {
  test('lists the flags', () => {
    const result = spawnSync('bun', [entry, '--help'], { cwd: packageDir, encoding: 'utf8' });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('--extension');
    expect(result.stdout).toContain('--launcher');
    expect(result.stdout).toContain('--overlay');
    expect(result.stdout).toContain('--repo');
  });
});

function buildExtension(app: Source): SpawnSyncReturns<string> {
  app.write(
    [
      "import { buildExtension } from './scripts/bridgething';",
      "await buildExtension(process.cwd(), 'dist', { entry: 'extension/desktop.mjs', api: 1 });",
      '',
    ].join('\n'),
    'build-check.ts',
  );
  return spawnSync('bun', ['build-check.ts'], { cwd: app.dir, encoding: 'utf8' });
}

describe('the scaffolded extension bundler', () => {
  test('bundles the untouched entry against the real @bridgething/extension through the generated plugin script', () => {
    const app = scaffold('--extension');
    app.lendModules();

    const result = buildExtension(app);
    expect(result.stderr).not.toContain('Could not resolve');
    expect(result.status).toBe(0);

    const bundle = app.read('dist', 'extension', 'desktop.mjs');
    expect(bundle).not.toContain('@bridgething/extension');
    expect(bundle).toContain('function defineExtension');
    expect(bundle).toContain('my-app extension up');
  });

  test('keeps npm:, jsr: and node: specifiers and still inlines relative modules', () => {
    const app = scaffold('--extension');
    app.lendModules();
    app.write("export const greeting = 'bundled from a relative module';\n", 'extension', 'helper.ts');
    app.write(
      [
        "import chalk from 'npm:chalk@5';",
        "import { assertEquals } from 'jsr:@std/assert@1';",
        "import { readFileSync } from 'node:fs';",
        "import { greeting } from './helper.ts';",
        '',
        'export default { chalk, assertEquals, readFileSync, greeting };',
        '',
      ].join('\n'),
      'extension',
      'main.ts',
    );

    const result = buildExtension(app);
    expect(result.stderr).not.toContain('Could not resolve');
    expect(result.status).toBe(0);

    const bundle = app.read('dist', 'extension', 'desktop.mjs');
    expect(bundle).toContain('"npm:chalk@5"');
    expect(bundle).toContain('"jsr:@std/assert@1"');
    expect(bundle).toContain('"node:fs"');
    expect(bundle).toContain('bundled from a relative module');
  });
});
