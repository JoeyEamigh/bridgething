#!/usr/bin/env bun
import { copyFileSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

const SHARED = resolve(import.meta.dir, '..', '..', 'webapp-shared', 'src');
const TEMPLATE = resolve(import.meta.dir, '..', 'template');

const HEADER = '// Generated from @bridgething/webapp-shared.\n';

async function bundle(entry: string, external: string[]): Promise<string> {
  const result = await Bun.build({
    entrypoints: [resolve(SHARED, entry)],
    target: 'node',
    format: 'esm',
    external,
    minify: false,
  });
  if (!result.success) {
    throw new Error(result.logs.map(log => log.message).join('\n'));
  }
  const [output] = result.outputs;
  if (!output) throw new Error(`bundling ${entry} produced nothing`);
  return await output.text();
}

async function emit(entry: string, dest: string, external: string[], lead: string, tail = ''): Promise<void> {
  const code = await bundle(entry, external);
  const path = resolve(TEMPLATE, dest);
  writeFileSync(path, `${lead}${HEADER}${code}${tail}`);
  console.log(`wrote ${path}`);
}

await emit(
  'push.ts',
  'scripts/push.ts',
  ['@msgpack/msgpack'],
  '#!/usr/bin/env bun\n',
  `
bridgethingPush({ scriptUrl: import.meta.url }).catch(err => {
  console.error(err instanceof Error ? err.message : err);
  process.exit(1);
});
`,
);

await emit('dev.ts', 'scripts/bridgething.ts', ['@msgpack/msgpack', 'vite', 'esbuild'], '');

const daemon = resolve(TEMPLATE, 'src', 'daemon.ts');
copyFileSync(resolve(SHARED, 'daemon.ts'), daemon);
writeFileSync(daemon, HEADER + readFileSync(daemon, 'utf8'));
console.log(`wrote ${daemon}`);

const formatted = Bun.spawnSync(
  [
    'bunx',
    'prettier',
    '--write',
    resolve(TEMPLATE, 'scripts', 'push.ts'),
    resolve(TEMPLATE, 'scripts', 'bridgething.ts'),
    daemon,
  ],
  { cwd: resolve(import.meta.dir, '..', '..', '..'), stdout: 'inherit', stderr: 'inherit' },
);
if (formatted.exitCode !== 0) throw new Error(`prettier exited ${formatted.exitCode}`);
