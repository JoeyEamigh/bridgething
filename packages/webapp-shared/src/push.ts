import { spawn } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { bundleDirName, resolveGatewayTarget, setSlot, switchTo, uuidToString, type Slot } from './gateway.js';

const WEBAPP_ROOT = '/var/bridgething/webapps';
const SSH_OPTS = ['-o', 'UserKnownHostsFile=/dev/null', '-o', 'StrictHostKeyChecking=no', '-o', 'LogLevel=ERROR'];

function run(cmd: string, args: string[], label: string, cwd?: string): Promise<void> {
  return new Promise<void>((res, rej) => {
    const child = spawn(cmd, args, { stdio: 'inherit', cwd });
    child.on('exit', code => (code === 0 ? res() : rej(new Error(`${label} exited ${code}`))));
    child.on('error', rej);
  });
}

function pipeThrough(
  producer: { cmd: string; args: string[] },
  consumer: { cmd: string; args: string[] },
  label: string,
): Promise<void> {
  return new Promise<void>((res, rej) => {
    const source = spawn(producer.cmd, producer.args, { stdio: ['ignore', 'pipe', 'inherit'] });
    const sink = spawn(consumer.cmd, consumer.args, { stdio: ['pipe', 'inherit', 'inherit'] });
    source.stdout.pipe(sink.stdin);
    source.on('error', rej);
    sink.on('error', rej);
    source.on('exit', code => {
      if (code !== 0) rej(new Error(`${producer.cmd} exited ${code}`));
    });
    sink.on('exit', code => (code === 0 ? res() : rej(new Error(`${label} exited ${code}`))));
  });
}

async function pushBundle(localDir: string, host: string, dirName: string): Promise<void> {
  const staged = `.push.${dirName}`;
  console.log(`copying ${localDir} -> root@${host}:${WEBAPP_ROOT}/${dirName}/`);
  const receive = [
    'set -e',
    `cd ${WEBAPP_ROOT}`,
    `rm -rf ${staged} .old.${dirName}`,
    `mkdir -p ${staged}`,
    `tar -xzf - -C ${staged}`,
    `if [ -d ${dirName} ]; then mv ${dirName} .old.${dirName}; fi`,
    `mv ${staged} ${dirName}`,
    `rm -rf .old.${dirName}`,
  ].join('; ');
  await pipeThrough(
    { cmd: 'tar', args: ['-czf', '-', '-C', localDir, '.'] },
    { cmd: 'ssh', args: [...SSH_OPTS, `root@${host}`, receive] },
    'ssh',
  );
}

function buildBundle(repoDir: string): Promise<void> {
  console.log('bun run build');
  return run('bun', ['run', 'build'], 'bun run build', repoDir);
}

type Manifest = { id?: string; name?: string; role?: string; overlay?: string };

function declaredSlots(manifest: Manifest): Slot[] {
  const slots: Slot[] = [];
  if (manifest.role === 'launcher') slots.push('launcher');
  if (manifest.overlay) slots.push('overlay');
  return slots;
}

type Args = {
  host: string;
  skipBuild: boolean;
  claimSlots: boolean;
  release: boolean;
  switchAfter: boolean | null;
};

function parseArgs(argv: string[]): Args {
  const args: Args = {
    host: process.env.SUPERBIRD_HOST ?? 'bridgething.local',
    skipBuild: process.env.SKIP_BUILD === '1',
    claimSlots: true,
    release: false,
    switchAfter: null,
  };
  for (const arg of argv) {
    if (arg === '--skip-build') args.skipBuild = true;
    else if (arg === '--no-switch') args.switchAfter = false;
    else if (arg === '--switch') args.switchAfter = true;
    else if (arg === '--no-slot') args.claimSlots = false;
    else if (arg === '--release') args.release = true;
    else if (arg === '--help' || arg === '-h') {
      printHelp();
      process.exit(0);
    } else if (arg.startsWith('--')) throw new Error(`unknown flag: ${arg}`);
    else args.host = arg;
  }
  return args;
}

function printHelp(): void {
  console.log(`Usage: bun run push [host] [options]

Build, copy dist/ onto a connected Car Thing, and make it visible.

A plain webapp becomes the active app. A launcher also takes the home-screen
slot; an overlay takes the overlay slot and the daemon reloads the kiosk so it
is injected into whatever app is showing. Overlay-only bundles are not switched
to by default, since switching away is the opposite of what you want to test.

Options:
  --release      hand this bundle's slots back to the built-in ones and stop.
                 the recovery path when a build wedges the screen.
  --no-slot      push without claiming any slot.
  --switch       switch to this bundle even if it is overlay-only.
  --no-switch    push without switching.
  --skip-build   copy whatever is already in dist/.

Env: SUPERBIRD_HOST, BRIDGETHING_GATEWAY_PORT, SKIP_BUILD=1
`);
}

export type BridgethingPushOptions = {
  scriptUrl: string;
};

export async function bridgethingPush({ scriptUrl }: BridgethingPushOptions): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const target = await resolveGatewayTarget(args.host);

  const repoDir = resolve(dirname(new URL(scriptUrl).pathname), '..');
  const distDir = resolve(repoDir, 'dist');
  const manifestPath = resolve(distDir, 'manifest.json');

  const readManifest = (): Manifest => {
    if (!existsSync(manifestPath)) {
      throw new Error(`no manifest.json at ${manifestPath}; run 'bun run build' first or drop --skip-build`);
    }
    const parsed = JSON.parse(readFileSync(manifestPath, 'utf8')) as Manifest;
    if (!parsed.id) throw new Error(`${manifestPath} has no 'id' field`);
    return parsed;
  };

  if (args.release) {
    const manifest = readManifest();
    for (const slot of declaredSlots(manifest)) {
      const result = await setSlot(target, slot, null);
      if (!result.ok) throw new Error(result.reason);
      console.log(`${slot} slot: reverted to the built-in one`);
    }
    if (declaredSlots(manifest).length === 0) console.log('this bundle declares no slots; nothing to release');
    return;
  }

  if (!args.skipBuild) await buildBundle(repoDir);
  const manifest = readManifest();
  const id = manifest.id as string;
  const slots = declaredSlots(manifest);

  await pushBundle(distDir, args.host, bundleDirName(id));

  if (args.claimSlots) {
    for (const slot of slots) {
      const result = await setSlot(target, slot, id);
      if (!result.ok) throw new Error(result.reason);
      console.log(`${slot} slot: ${manifest.name ?? id}`);
    }
  }

  const overlayOnly = slots.length > 0 && slots.every(s => s === 'overlay');
  const shouldSwitch = args.switchAfter ?? !overlayOnly;
  if (!shouldSwitch) {
    console.log(overlayOnly ? 'overlay pushed; the kiosk reloaded with it injected' : 'skipping switch');
    return;
  }

  const switched = await switchTo(target, id);
  if (!switched.ok) throw new Error(switched.reason);
  const active = switched.value;
  if (!active) {
    console.log('switched (the daemon dropped the push connection reloading the kiosk)');
    return;
  }
  const activeStr = active.id ? uuidToString(active.id) : '(none)';
  console.log(`active webapp: ${active.name ?? '(unnamed)'} ${activeStr}`);
}
