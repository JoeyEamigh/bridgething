import { afterAll, beforeAll, describe, expect, test } from 'bun:test';
import { spawnSync, type SpawnSyncReturns } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

const packageDir = resolve(import.meta.dir, '..');
const entry = join(packageDir, 'src', 'index.ts');

type Manifest = {
  id: string;
  role?: string;
  overlay?: string;
  config: { type: string; data: { key: string; label: string } }[];
  extension?: { entry: string; permissions: string[]; api: number };
};

type Package = {
  scripts: Record<string, string>;
  dependencies: Record<string, string>;
  devDependencies: Record<string, string>;
};

class Scaffold {
  constructor(readonly dir: string) {}

  path(...parts: string[]): string {
    return join(this.dir, ...parts);
  }

  has(...parts: string[]): boolean {
    return existsSync(this.path(...parts));
  }

  read(...parts: string[]): string {
    return readFileSync(this.path(...parts), 'utf8');
  }

  manifest(): Manifest {
    return JSON.parse(this.read('public', 'manifest.json')) as Manifest;
  }

  pkg(): Package {
    return JSON.parse(this.read('package.json')) as Package;
  }

  write(contents: string, ...parts: string[]): void {
    writeFileSync(this.path(...parts), contents);
  }

  lendModules(): void {
    symlinkSync(join(packageDir, 'node_modules'), this.path('node_modules'), 'dir');
  }
}

const roots: string[] = [];

function scaffold(...flags: string[]): Scaffold {
  const root = mkdtempSync(join(tmpdir(), 'create-bridgething-'));
  roots.push(root);
  const target = join(root, 'my-app');
  const result = spawnSync('bun', [entry, target, '--no-install', '--no-git', ...flags], {
    cwd: packageDir,
    encoding: 'utf8',
  });
  if (result.status !== 0) throw new Error(`scaffold failed: ${result.stdout}\n${result.stderr}`);
  return new Scaffold(target);
}

let plain: Scaffold;
let extension: Scaffold;

beforeAll(() => {
  plain = scaffold();
  extension = scaffold('--extension');
});

afterAll(() => {
  for (const root of roots) rmSync(root, { recursive: true, force: true });
});

describe('without --extension', () => {
  test('nothing extension-shaped lands', () => {
    expect(plain.has('extension', 'main.ts')).toBe(false);
    expect(plain.has('scripts', 'bridgething.ts')).toBe(true);
    expect(plain.manifest().extension).toBeUndefined();
    expect(plain.pkg().scripts.build).toBe('vite build && vite build -c vite.settings.config.ts');
    expect(plain.pkg().devDependencies.deno).toBeUndefined();
    expect(plain.pkg().dependencies['@bridgething/extension']).toBeUndefined();
    expect(plain.read('CLAUDE.md')).not.toContain('This project ships a native extension');
  });

  test('the extension skill page ships with every project', () => {
    expect(plain.has('.claude', 'skills', 'bridgething', 'reference', 'extension.md')).toBe(true);
    expect(plain.read('.claude', 'skills', 'bridgething', 'SKILL.md')).toContain('reference/extension.md');
  });

  test('the scaffolded docs name every surface the SDK ships', () => {
    const surfaces = resolve(packageDir, '..', '..', 'crates', 'lib', 'docs', 'surfaces.json');
    const shipped = (JSON.parse(readFileSync(surfaces, 'utf8')) as { surfaces: { name: string }[] }).surfaces
      .map(surface => surface.name)
      .sort();

    const sdk = plain.read('.claude', 'skills', 'bridgething', 'reference', 'sdk.md');
    expect([...sdk.matchAll(/^\| `([a-z]+)` \|/gm)].map(row => row[1]!).sort()).toEqual(shipped);

    const listed = /Every surface: `([^`]+)`/.exec(plain.read('CLAUDE.md'));
    expect(listed?.[1]?.split(/\s+/).sort()).toEqual(shipped);
  });

  test('no scaffolded doc hard-codes a surface count that goes stale', () => {
    const docs = [
      plain.read('CLAUDE.md'),
      plain.read('.claude', 'skills', 'bridgething', 'SKILL.md'),
      plain.read('.claude', 'skills', 'bridgething', 'reference', 'sdk.md'),
    ];

    for (const doc of docs) {
      expect(doc).not.toMatch(/\b\d+[\s-]surfaces?\b/i);
      expect(doc).not.toMatch(/\b(?:all|full|every|only)\s+\d+\b/i);
    }
  });

  test('the extension reference only claims what the builders verified', () => {
    const reference = plain.read('.claude', 'skills', 'bridgething', 'reference', 'extension.md');

    expect(reference).toContain('unix sockets');
    expect(reference).not.toContain('named pipe');
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
    const ownVersion = (JSON.parse(readFileSync(join(packageDir, 'package.json'), 'utf8')) as { version: string })
      .version;
    expect(pkg.dependencies['@bridgething/extension']).toBe(`^${ownVersion}`);
    expect(pkg.dependencies['@bridgething/client']).toBeDefined();
    expect(pkg.devDependencies.esbuild).toBe('^0.28.2');
    expect(pkg.devDependencies.deno).toBe('2.9.6');
    expect(pkg.devDependencies.vite).toBeDefined();
    expect(pkg.scripts).toEqual(plain.pkg().scripts);
  });

  test('appends the extension guide to CLAUDE.md and leaves no marker file', () => {
    const claude = extension.read('CLAUDE.md');
    expect(claude).toContain('This project ships a native extension');
    expect(claude).toContain('Building this bridgething webapp');
    expect(extension.has('_claude_append.md')).toBe(false);
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
    expect(claude).toContain('This project is a launcher');
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
  test('lists the flag', () => {
    const result = spawnSync('bun', [entry, '--help'], { cwd: packageDir, encoding: 'utf8' });
    expect(result.status).toBe(0);
    expect(result.stdout).toContain('--extension');
    expect(result.stdout).toContain('--launcher');
    expect(result.stdout).toContain('--overlay');
  });
});

function buildExtension(app: Scaffold): SpawnSyncReturns<string> {
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
