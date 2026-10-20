import { cp, mkdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

const stage = process.argv[2];
if (!stage) throw new Error('usage: build.ts <stage dir>');
const here = import.meta.dir;

await rm(stage, { recursive: true, force: true });
await mkdir(stage, { recursive: true });
await cp(join(here, 'manifest.json'), join(stage, 'manifest.json'));
await cp(join(here, 'index.html'), join(stage, 'index.html'));

const built = await Bun.build({
  entrypoints: [join(here, 'settings.ts')],
  target: 'browser',
  minify: true,
});
if (!built.success)
  throw new AggregateError(built.logs, 'the settings page did not build');
const script = (await built.outputs[0]!.text()).replaceAll(
  '</script',
  '<\\/script',
);
await writeFile(
  join(stage, 'settings.html'),
  `<!doctype html><html><body><script type="module">${script}</script></body></html>`,
);
