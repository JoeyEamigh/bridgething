import preact from '@preact/preset-vite';
import tailwindcss from '@tailwindcss/vite';
import { statSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig, type Plugin } from 'vite';

const WARN_BYTES = 128 * 1024;
const HARD_CAP = '512 KiB';

function sizeGuard(): Plugin {
  return {
    name: 'overlay-size-guard',
    closeBundle() {
      let bytes: number;
      try {
        bytes = statSync(resolve(__dirname, 'dist', 'overlay.js')).size;
      } catch {
        return;
      }
      if (bytes > WARN_BYTES) {
        console.warn(`overlay.js is ${(bytes / 1024).toFixed(1)} KiB; install fails over ${HARD_CAP}.`);
      }
    },
  };
}

export default defineConfig({
  plugins: [preact(), tailwindcss(), sizeGuard()],
  build: {
    target: 'es2022',
    outDir: resolve(__dirname, 'dist'),
    emptyOutDir: false,
    cssCodeSplit: false,
    lib: {
      entry: resolve(__dirname, 'overlay', 'main.tsx'),
      formats: ['iife'],
      name: '__overlay',
      fileName: () => 'overlay.js',
    },
    rollupOptions: {
      output: { inlineDynamicImports: true },
    },
  },
});
