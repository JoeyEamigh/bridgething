import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';
import { bridgething, daemonProxy } from './scripts/bridgething';

export default defineConfig(async ({ command }) => ({
  plugins: [react(), tailwindcss(), bridgething()],
  build: {
    target: 'es2022',
    sourcemap: true,
  },
  server: command === 'serve' ? { host: true, proxy: await daemonProxy() } : {},
}));
