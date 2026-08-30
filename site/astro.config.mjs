import preact from '@astrojs/preact';
import sitemap from '@astrojs/sitemap';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'astro/config';
import { spawn } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { APP_DETAIL_SHELL } from './src/lib/app-routes.ts';
import { FEATURES } from './src/lib/features.ts';

const EXCLUDED = ['/admin', APP_DETAIL_SHELL, ...(FEATURES.browserFlasher ? [] : ['/install/flash'])];

const WORKER_DEV_PORT = 8787;
const WORKER_DEV_ORIGIN = `http://127.0.0.1:${WORKER_DEV_PORT}`;
const WORKER_ROUTES = ['/api', '/oauth/callback'];

function workerDev() {
  let child = null;
  return {
    name: 'worker-dev',
    hooks: {
      'astro:server:setup': () => {
        mkdirSync(fileURLToPath(new URL('./dist', import.meta.url)), { recursive: true });
        child = spawn('wrangler', ['dev', '--port', String(WORKER_DEV_PORT)], {
          cwd: fileURLToPath(new URL('.', import.meta.url)),
          stdio: ['ignore', 'inherit', 'inherit'],
        });
      },
      'astro:server:done': () => {
        child?.kill();
        child = null;
      },
    },
  };
}

export default defineConfig({
  site: 'https://bridgething.com',
  output: 'static',
  integrations: [
    preact(),
    workerDev(),
    sitemap({
      filter: page => {
        const path = new URL(page).pathname.replace(/\/$/, '');
        return !EXCLUDED.some(excluded => path === excluded.replace(/\/$/, ''));
      },
    }),
  ],
  trailingSlash: 'ignore',
  redirects: { '/apps/store': '/apps' },
  build: {
    format: 'directory',
    inlineStylesheets: 'auto',
  },
  vite: {
    plugins: [tailwindcss()],
    resolve: { alias: { 'node:zlib': fileURLToPath(new URL('./src/lib/zlib-shim.ts', import.meta.url)) } },
    server: {
      fs: { allow: ['..'] },
      proxy: Object.fromEntries(WORKER_ROUTES.map(route => [route, WORKER_DEV_ORIGIN])),
    },
  },
});
