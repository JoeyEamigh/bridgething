import preact from '@preact/preset-vite';
import { readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig, type Plugin } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

const DIST = resolve(import.meta.dirname, 'dist');
const PAGE = resolve(DIST, 'default-settings.html');

function embed(): Plugin {
  return {
    name: 'embed-default-settings',
    closeBundle() {
      const html = readFileSync(PAGE, 'utf8');
      writeFileSync(
        resolve(DIST, 'default-settings.js'),
        `export const DEFAULT_SETTINGS_PAGE = ${JSON.stringify(html)};\n`,
      );
      writeFileSync(resolve(DIST, 'default-settings.d.ts'), 'export declare const DEFAULT_SETTINGS_PAGE: string;\n');
      rmSync(PAGE);
    },
  };
}

export default defineConfig({
  root: 'default-settings',
  logLevel: 'warn',
  plugins: [preact(), viteSingleFile(), embed()],
  build: {
    target: 'es2022',
    outDir: DIST,
    emptyOutDir: false,
    rollupOptions: {
      input: resolve(import.meta.dirname, 'default-settings', 'default-settings.html'),
    },
  },
});
