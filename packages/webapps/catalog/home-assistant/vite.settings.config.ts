import preact from '@preact/preset-vite';
import { statSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig, type Plugin } from 'vite';
import { viteSingleFile } from 'vite-plugin-singlefile';

const WARN_BYTES = 200 * 1024;
const LOUD_BYTES = 500 * 1024;
const HARD_CAP = '1 MiB';

function sizeGuard(): Plugin {
  return {
    name: 'settings-size-guard',
    closeBundle() {
      const out = resolve(__dirname, 'dist', 'settings.html');
      let bytes: number;
      try {
        bytes = statSync(out).size;
      } catch {
        return;
      }
      const kib = (bytes / 1024).toFixed(1);
      if (bytes > LOUD_BYTES) {
        console.warn(
          [
            '',
            '========================================================================',
            `  settings.html is ${kib} KiB.`,
            `  the device refuses to install a bundle whose settings page is over ${HARD_CAP}.`,
            '  trim dependencies or inline assets; the whole install fails, not just this page.',
            '========================================================================',
            '',
          ].join('\n'),
        );
      } else if (bytes > WARN_BYTES) {
        console.warn(`settings.html is ${kib} KiB; keep it lean (install fails over ${HARD_CAP}).`);
      }
    },
  };
}

export default defineConfig({
  root: 'settings',
  plugins: [preact(), viteSingleFile(), sizeGuard()],
  build: {
    target: 'es2022',
    outDir: resolve(__dirname, 'dist'),
    emptyOutDir: false,
    rollupOptions: {
      input: resolve(__dirname, 'settings', 'settings.html'),
    },
  },
});
