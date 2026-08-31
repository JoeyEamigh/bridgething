#!/usr/bin/env bun

import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

const port = Number(process.env.E2E_FIXTURE_PORT ?? '8899');
const version = process.env.E2E_COMPANION_VERSION ?? '99.0.0';

const CHANNEL = process.env.E2E_OTA_CHANNEL ?? 'dev';
const VARIANT = process.env.E2E_OTA_VARIANT ?? 'dev';
const DEVICE_IMAGE = process.env.E2E_OTA_IMAGE_VERSION ?? '0.1.0';
const DEVICE_DAEMON = process.env.E2E_OTA_DAEMON_VERSION ?? '0.10.0';

const NEXT_IMAGE = '99.9.9';
const NEXT_DAEMON = '99.0.0';
const NEXT_WEBAPP = '99.0.0';
const NEXT_WAKEWORD = '99.0.0';
const WAKEWORD_FILE = 'hey_bridgething.btww';
const BUILTIN = 'hub';

const STORE_APP_ID = '019e6701-13f8-71b5-ba04-85d326630e98';

const ROOT = join(dirname(Bun.main), '..', '..');
const DEV_DAEMON = join(ROOT, 'scripts', 'dev-daemon.sh');
const DEV_DIR = process.env.BRIDGETHING_DEV_DIR ?? join(ROOT, '.e2e');

function pattern(bytes: number): Uint8Array {
  const out = new Uint8Array(bytes);
  for (let i = 0; i < bytes; i++) out[i] = i % 251;
  return out;
}

function digestOf(body: Uint8Array) {
  return {
    size: body.byteLength,
    sha256: createHash('sha256').update(body).digest('hex'),
  };
}

const artifacts = {
  imageSwu: pattern(64 * 1024),
  imageZck: pattern(32 * 1024),
  imageBootZck: pattern(8 * 1024),
  daemon: pattern(48 * 1024),
  webapp: pattern(16 * 1024),
  wakeword: pattern(3 * 1024),
};

const SETTINGS_PAGE = `<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>Fixture settings</title>
  </head>
  <body>
    <h1>Fixture Calendar settings</h1>
    <p id="hosted-settings-marker">the settings page rendered</p>
  </body>
</html>
`;

function storeBundle(): Uint8Array {
  const staging = join(DEV_DIR, 'store-fixture');
  const dist = join(staging, 'dist');
  const out = join(staging, 'app.zip');
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(dist, { recursive: true });

  writeFileSync(
    join(dist, 'manifest.json'),
    JSON.stringify(
      {
        id: STORE_APP_ID,
        name: 'Fixture Calendar',
        version: '1.0.0',
        description: 'A store fixture with a settings page.',
        settings: 'settings.html',
        config: [{ type: 'string', data: { key: 'greeting', label: 'Greeting' } }],
        permissions: [],
      },
      null,
      2,
    ),
  );
  writeFileSync(join(dist, 'index.html'), '<!doctype html><title>Fixture Calendar</title><h1>fixture</h1>\n');
  writeFileSync(join(dist, 'settings.html'), SETTINGS_PAGE);

  const zipped = spawnSync('zip', ['-q', '-X', '-r', out, 'manifest.json', 'index.html', 'settings.html'], {
    cwd: dist,
  });
  if (zipped.status !== 0) throw new Error(`could not build the store fixture bundle: ${zipped.stderr}`);
  return new Uint8Array(readFileSync(out));
}

const storeApp = storeBundle();
const storeSettings = new TextEncoder().encode(SETTINGS_PAGE);
let hostedSettingsHits = 0;

type Mode = 'current' | 'image' | 'daemon' | 'webapp' | 'wakeword';
const MODES: Mode[] = ['current', 'image', 'daemon', 'webapp', 'wakeword'];

let bounceMode: Mode = 'image';

const companion = {
  android: {
    version,
    url: `http://10.0.2.2:${port}/companion/android/${version}/bridgething-${version}.apk`,
    size: 1,
    sha256: '0'.repeat(64),
    released_at: '2026-01-01T00:00:00Z',
  },
};

function manifest(mode: Mode) {
  const image = mode === 'image' ? NEXT_IMAGE : DEVICE_IMAGE;
  const daemon = mode === 'daemon' ? NEXT_DAEMON : DEVICE_DAEMON;
  const latest = `${daemon}+image.${image}`;

  return {
    manifest_version: 1,
    updated_at: '2026-08-22T00:00:00Z',
    channels: {
      [CHANNEL]: {
        name: CHANNEL,
        stability: 'stable',
        default: true,
        latest,
        releases: [latest],
      },
    },
    releases: {
      [latest]: {
        version: latest,
        channel: CHANNEL,
        yanked: null,
        deprecated: false,
        builtin_webapps: mode === 'webapp' ? { [BUILTIN]: NEXT_WEBAPP } : {},
        wakeword: mode === 'wakeword' ? { runtime: daemon, model: NEXT_WAKEWORD, model_trained_against: {} } : null,
        artifacts: {
          daemon: mode === 'daemon' ? digestOf(artifacts.daemon) : null,
          daemon_zst: null,
          image_swu: mode === 'image' ? digestOf(artifacts.imageSwu) : null,
          image_zck: mode === 'image' ? digestOf(artifacts.imageZck) : null,
          image_boot_zck: mode === 'image' ? digestOf(artifacts.imageBootZck) : null,
          webapps: {},
          wakeword: mode === 'wakeword' ? { model: digestOf(artifacts.wakeword) } : { model: null },
          daemon_patches: {},
        },
      },
    },
  };
}

const SHOT = join(ROOT, 'site', 'public', 'screenshots', 'device-calendar.png');

function storeCatalog(host: string) {
  return {
    schema: 'catalog.v1',
    updated_at: '2026-01-01T00:00:00Z',
    repo: { name: 'e2e fixtures', description: 'store fixtures for the e2e lane', homepage: null, icon: null },
    apps: [
      {
        id: STORE_APP_ID,
        name: 'Fixture Calendar',
        description: 'A store fixture with a screenshot.',
        author: 'e2e',
        icon: null,
        screenshots: [`http://${host}/store/shot.png`, `http://${host}/store/shot.png?two`],
        homepage: null,
        source: 'https://github.com/JoeyEamigh/bridgething',
        versions: [
          {
            version: '1.0.0',
            released_at: '2026-01-01T00:00:00Z',
            download: { url: `http://${host}/store/app.zip`, ...digestOf(storeApp) },
            settings: { url: `http://${host}/store/settings.html`, ...digestOf(storeSettings) },
            permissions: [],
            min_libbridgething_version: '0.4.0',
            changelog: null,
          },
        ],
      },
    ],
    recommended_sources: [],
  };
}

function bytes(body: Uint8Array): Response {
  return new Response(body, {
    headers: {
      'content-type': 'application/octet-stream',
      'content-length': String(body.byteLength),
      'accept-ranges': 'bytes',
    },
  });
}

function daemonCtl(action: 'start' | 'stop'): Promise<void> {
  return new Promise(resolve => {
    const child = spawn(DEV_DAEMON, [action], {
      env: { ...process.env, BRIDGETHING_DEV_DIR: DEV_DIR },
      stdio: 'ignore',
    });
    child.on('exit', () => resolve());
    child.on('error', () => resolve());
  });
}

const sleep = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

function otaRoute(mode: Mode, path: string): Response {
  if (path === '/manifest.json') return Response.json(manifest(mode));
  if (path === `/images/${CHANNEL}/${NEXT_IMAGE}/bridgething-${VARIANT}-image.swu`) return bytes(artifacts.imageSwu);
  if (path === `/images/${CHANNEL}/${NEXT_IMAGE}/bridgething-${VARIANT}-image.zck`) return bytes(artifacts.imageZck);
  if (path === `/images/${CHANNEL}/${NEXT_IMAGE}/bridgething-${VARIANT}-image-boot.zck`)
    return bytes(artifacts.imageBootZck);
  if (path === `/daemon/${CHANNEL}/${NEXT_DAEMON}/bridgething`) return bytes(artifacts.daemon);
  if (path === `/webapps/${CHANNEL}/${BUILTIN}/${NEXT_WEBAPP}/${BUILTIN}.zip`) return bytes(artifacts.webapp);
  if (path === `/wakeword/${CHANNEL}/model/${NEXT_WAKEWORD}/${WAKEWORD_FILE}`) return bytes(artifacts.wakeword);
  return new Response('not found', { status: 404 });
}

Bun.serve({
  port,
  hostname: '0.0.0.0',
  idleTimeout: 120,
  async fetch(request) {
    const { pathname, searchParams } = new URL(request.url);
    console.log(`[e2e-fixtures] ${request.method} ${pathname}`);

    if (pathname === '/companion.json') return Response.json(companion);

    if (pathname === '/store/catalog.json') {
      return Response.json(storeCatalog(new URL(request.url).host), {
        headers: { 'access-control-allow-origin': '*' },
      });
    }
    if (pathname === '/store/shot.png') {
      return new Response(Bun.file(SHOT), { headers: { 'content-type': 'image/png' } });
    }
    if (pathname === '/store/app.zip') return bytes(storeApp);
    if (pathname === '/store/settings.html') {
      hostedSettingsHits += 1;
      return new Response(SETTINGS_PAGE, {
        headers: { 'content-type': 'text/html; charset=utf-8', 'access-control-allow-origin': '*' },
      });
    }
    if (pathname === '/store/settings-hits') {
      return Response.json({ hits: hostedSettingsHits }, { headers: { 'access-control-allow-origin': '*' } });
    }

    const rooted = /^\/m\/([a-z]+)(\/.*)?$/.exec(pathname);
    if (rooted) {
      const name = rooted[1];
      const rest = rooted[2] ?? '/';
      const mode: Mode | null = name === 'bounce' ? bounceMode : MODES.includes(name as Mode) ? (name as Mode) : null;
      if (mode) return otaRoute(mode, rest);
    }

    if (pathname === '/control/daemon/bounce') {
      const downMs = Number(searchParams.get('downMs') ?? '10000');
      const next = searchParams.get('mode') as Mode | null;
      await daemonCtl('stop');
      if (next) {
        bounceMode = next;
        console.log(`[e2e-fixtures] bounce root -> ${bounceMode} while the link is down`);
      }
      console.log(`[e2e-fixtures] daemon down for ${downMs}ms`);
      await sleep(downMs);
      await daemonCtl('start');
      console.log('[e2e-fixtures] daemon back up');
      return Response.json({ bounced: true, downMs, mode: bounceMode });
    }

    return new Response('not found', { status: 404 });
  },
});

console.log(
  `[e2e-fixtures] companion v${version} + ota roots /m/{${MODES.join('|')}|bounce} + store /store/catalog.json on :${port}`,
);
