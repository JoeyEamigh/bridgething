#!/usr/bin/env bun
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { mkdir, readdir, readFile, rm, stat } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';

interface Args {
  dist: string;
  output: string;
}

function parseArgs(argv: string[]): Args {
  const args: Partial<Args> = {};
  for (let i = 0; i < argv.length; i++) {
    const next = argv[i + 1];
    switch (argv[i]) {
      case '--dist':
        args.dist = next;
        i++;
        break;
      case '--output':
        args.output = next;
        i++;
        break;
      case '--help':
      case '-h':
        printHelpAndExit(0);
        break;
      default:
        console.error(`unknown argument: ${argv[i]}`);
        printHelpAndExit(2);
    }
  }
  for (const required of ['dist', 'output'] as const) {
    if (!args[required]) {
      console.error(`missing required argument --${required}`);
      printHelpAndExit(2);
    }
  }
  return args as Args;
}

function printHelpAndExit(code: number): never {
  console.log(
    [
      'Usage: bun run scripts/webapp-zip.ts --dist <path> --output <path>',
      '',
      '  --dist <path>     built webapp dist dir (index.html + manifest.json + icon at root)',
      '  --output <path>   output .zip path',
    ].join('\n'),
  );
  process.exit(code);
}

function run(cmd: string, cmdArgs: string[], cwd?: string): void {
  const r = spawnSync(cmd, cmdArgs, { stdio: 'inherit', cwd });
  if (r.status !== 0) {
    throw new Error(`${cmd} ${cmdArgs.join(' ')} exited with status ${r.status}`);
  }
}

async function sha256(path: string): Promise<string> {
  return await new Promise<string>((resolveP, rejectP) => {
    const hash = createHash('sha256');
    const stream = createReadStream(path);
    stream.on('data', chunk => hash.update(chunk));
    stream.on('end', () => resolveP(hash.digest('hex')));
    stream.on('error', rejectP);
  });
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const dist = resolve(args.dist);
  const out = resolve(args.output);

  const manifestRaw = await readFile(join(dist, 'manifest.json'), 'utf-8').catch(() => {
    throw new Error(`no manifest.json in ${dist}; is this a built webapp dist dir?`);
  });
  const manifest = JSON.parse(manifestRaw) as {
    id?: string;
    name?: string;
    description?: string;
    version?: string;
    icon?: string;
    role?: string;
    overlay?: string;
    permissions?: string[];
    extension?: { entry?: string; permissions?: string[]; api?: number };
  };
  for (const field of ['id', 'name', 'description', 'version'] as const) {
    if (!manifest[field]) throw new Error(`${dist}/manifest.json missing required field "${field}"`);
  }
  if (!(await readdir(dist)).includes('index.html')) {
    throw new Error(`${dist} has no index.html at its root`);
  }
  if (manifest.extension?.entry) {
    const entry = join(dist, manifest.extension.entry);
    if (!(await stat(entry).catch(() => null))?.isFile()) {
      throw new Error(
        `${dist}/manifest.json declares extension entry "${manifest.extension.entry}", which is not in the bundle`,
      );
    }
  }

  await mkdir(dirname(out), { recursive: true });
  await rm(out, { force: true });
  const entries = (await readdir(dist)).filter(e => !e.startsWith('.'));
  run('zip', ['-q', '-X', '-r', out, ...entries], dist);

  const size = (await stat(out)).size;
  const digest = await sha256(out);

  const summary = {
    id: manifest.id,
    name: manifest.name,
    description: manifest.description,
    version: manifest.version,
    permissions: manifest.permissions ?? [],
    role: manifest.role ?? null,
    provides_overlay: Boolean(manifest.overlay),
    extension: manifest.extension ?? null,
    icon: manifest.icon ?? null,
    iconPath: manifest.icon ? join(dist, manifest.icon) : null,
    size,
    sha256: digest,
    output: out,
  };
  console.log(JSON.stringify(summary, null, 2));
}

await main();
