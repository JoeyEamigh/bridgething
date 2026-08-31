import { existsSync } from 'node:fs';
import { mkdir, readdir } from 'node:fs/promises';
import { join } from 'node:path';
import { requireApp, writeBytes } from './lib.ts';
import { fail } from './paths.ts';

const HOST = process.env.SUPERBIRD_HOST ?? 'bridgething.local';
const CDP_PORT = 9222;
const MAX_SHOTS = 6;
const SCREEN = { width: 800, height: 480 };

export const SHOT_USAGE = `Usage: bun run shot <slug> [--replace] [--name <label>]

Captures what is on the Car Thing's screen right now into
apps/<slug>/screenshots/, where publishing picks it up and lists it in order.

Get the app on the kiosk first with "bun run push <slug>" or "bun run dev:device
<slug>". SUPERBIRD_HOST picks another device.

  --replace       overwrite the first screenshot instead of adding one
  --name <label>  name the file, rather than numbering it

The store shows them in filename order and uses the first as the card image.`;

interface Target {
  type: string;
  url: string;
  webSocketDebuggerUrl: string;
}

async function targets(): Promise<Target[] | null> {
  try {
    const response = await fetch(`http://${HOST}:${CDP_PORT}/json`, { signal: AbortSignal.timeout(5000) });
    return response.ok ? ((await response.json()) as Target[]) : null;
  } catch {
    return null;
  }
}

async function capture(target: Target): Promise<Buffer> {
  const socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve();
    socket.onerror = () => reject(new Error(`could not open a CDP socket to ${target.webSocketDebuggerUrl}`));
    setTimeout(() => reject(new Error('the CDP socket did not open within 8s')), 8000);
  });

  try {
    const reply = await new Promise<{ data: string }>((resolve, reject) => {
      socket.onmessage = event => {
        const message = JSON.parse(event.data as string) as { id?: number; result?: { data: string }; error?: unknown };
        if (message.id !== 1) return;
        if (message.error) reject(new Error(`CDP refused the capture: ${JSON.stringify(message.error)}`));
        else resolve(message.result as { data: string });
      };
      socket.send(JSON.stringify({ id: 1, method: 'Page.captureScreenshot', params: { format: 'png' } }));
      setTimeout(() => reject(new Error('the capture did not come back within 15s')), 15000);
    });
    return Buffer.from(reply.data, 'base64');
  } finally {
    socket.close();
  }
}

function pngSize(bytes: Buffer): { width: number; height: number } | null {
  if (bytes.length < 24 || bytes.subarray(12, 16).toString('ascii') !== 'IHDR') return null;
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

async function nextName(dir: string, label: string | null, replace: boolean): Promise<string> {
  if (label) return `${label.replace(/[^a-z0-9-]+/gi, '-').toLowerCase()}.png`;
  const taken = existsSync(dir) ? (await readdir(dir)).filter(file => file.endsWith('.png')).sort() : [];
  if (replace) return taken[0] ?? '01.png';
  if (taken.length >= MAX_SHOTS) {
    fail(`there are already ${taken.length} screenshots and the store shows at most ${MAX_SHOTS}; pass --replace`);
  }
  return `${String(taken.length + 1).padStart(2, '0')}.png`;
}

export async function shot(argv: string[]): Promise<void> {
  let slug: string | null = null;
  let label: string | null = null;
  let replace = false;
  for (let at = 0; at < argv.length; at++) {
    const arg = argv[at]!;
    if (arg === '--replace') replace = true;
    else if (arg === '--name') label = argv[++at] ?? fail('--name needs a label');
    else if (arg.startsWith('-')) fail(`unknown flag ${arg}`);
    else if (slug) fail('name one app');
    else slug = arg;
  }
  if (!slug) {
    console.log(SHOT_USAGE);
    process.exit(1);
  }

  const app = await requireApp(slug);
  const found = await targets();
  if (!found) fail(`${HOST} is not answering CDP on ${CDP_PORT}. Is the device plugged in and the kiosk running?`);
  const page = found.find(target => target.type === 'page');
  if (!page) fail(`${HOST} answered CDP but is showing no page`);
  const bytes = await capture(page);

  const dir = join(app.dir, 'screenshots');
  await mkdir(dir, { recursive: true });
  const name = await nextName(dir, label, replace);
  await writeBytes(join(dir, name), bytes);

  const size = pngSize(bytes);
  const measured = size ? `${size.width}x${size.height}` : 'unknown size';
  console.log(`apps/${app.slug}/screenshots/${name}  ${(bytes.length / 1024).toFixed(0)} KiB, ${measured}`);
  if (size && (size.width !== SCREEN.width || size.height !== SCREEN.height)) {
    console.warn(`warning: the store expects ${SCREEN.width}x${SCREEN.height}; this capture is ${measured}`);
  }
  console.log('commit it, and the next publish uploads it.');
}
