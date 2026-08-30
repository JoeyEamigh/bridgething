import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig, type UserConfig } from 'vite';
import { bridgething, daemonProxy } from './dev.js';

export type BridgethingViteOverrides = {
  plugins?: NonNullable<UserConfig['plugins']>;
  build?: UserConfig['build'];
  server?: UserConfig['server'];
};

export async function defineBridgethingConfig(overrides: BridgethingViteOverrides = {}): Promise<UserConfig> {
  return defineConfig({
    plugins: [react(), tailwindcss(), bridgething(), ...(overrides.plugins ?? [])],
    build: {
      target: 'es2022',
      sourcemap: true,
      ...(overrides.build ?? {}),
    },
    server: {
      host: true,
      proxy: await daemonProxy(),
      ...(overrides.server ?? {}),
    },
  }) as UserConfig;
}
