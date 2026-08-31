#!/usr/bin/env node
import { bump, BUMP_USAGE } from './bump.ts';
import { readLedger, stage } from './catalog.ts';
import { check } from './check.ts';
import { listApps, publicBase, readSource, run as spawn } from './lib.ts';
import { fail, siteDir, UserError } from './paths.ts';
import { publish } from './publish.ts';
import { shot, SHOT_USAGE } from './shot.ts';
import { reportSkillDrift, skills } from './skills.ts';

const USAGE = `Usage: bridgething-source <command> [options]

  run <script>       run a package script in an app dir
  check              typecheck, build, bundle, and validate every app
  bump               move an app to its next version
  shot               capture the device screen into an app's screenshots/
  catalog            stage the catalog into site/ without pushing
  publish            build every unpublished version and push the catalog branch
  skills             refresh the /bridgething agent skill from the registry

${BUMP_USAGE}

${SHOT_USAGE}`;

async function runInApp(argv: string[]): Promise<void> {
  const [script, ...rest] = argv;
  if (!script) fail('pass a script: bridgething-source run <script> [slug]');

  const apps = await listApps();
  if (!apps.length) fail('apps/ is empty. run "bun run new <slug>" to add an app');

  const named = rest[0] && !rest[0].startsWith('-') ? rest.shift() : null;
  const app = named ? apps.find(a => a.slug === named) : apps.length === 1 ? apps[0] : null;
  if (named && !app) fail(`no app "${named}". this source has ${apps.map(a => a.slug).join(', ')}`);
  if (!app) fail(`this source has ${apps.map(a => a.slug).join(', ')}`);

  spawn('bun', ['run', script, ...rest], app.dir);
}

async function catalog(): Promise<void> {
  const source = await readSource();
  const base = publicBase(source);
  const site = siteDir();
  const result = await stage(site, await listApps(), await readLedger(site), base);
  console.log(`\nstaged ${result.catalog.apps.length} app(s) into site/ against ${base}`);
  console.log(`new this run: ${result.added.map(a => `${a.slug} ${a.version}`).join(', ') || 'none'}`);
  console.log('site/ contains a local preview');
}

async function main(): Promise<void> {
  const [command, ...argv] = process.argv.slice(2);
  switch (command) {
    case 'run':
      return runInApp(argv);
    case 'check':
      await reportSkillDrift();
      return check();
    case 'bump':
      return bump(argv);
    case 'shot':
      return shot(argv);
    case 'catalog':
      return catalog();
    case 'publish':
      return publish(argv);
    case 'skills':
      return skills(argv);
    default:
      console.log(USAGE);
      process.exit(command ? 1 : 0);
  }
}

try {
  await main();
} catch (err) {
  if (err instanceof UserError) {
    console.error(`error: ${err.message}`);
    process.exit(1);
  }
  throw err;
}
