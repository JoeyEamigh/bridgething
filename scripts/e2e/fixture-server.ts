#!/usr/bin/env bun

import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
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

console.log(`[e2e-fixtures] companion v${version} + ota roots /m/{${MODES.join('|')}|bounce} on :${port}`);
