import type { IconName } from './lib/icons.tsx';

export const PATHS = {
  devices: '/devices',
  apps: '/apps',
  app: (webappId: string) => `/apps/${encodeURIComponent(webappId)}`,
  appSettings: (webappId: string) => `/apps/${encodeURIComponent(webappId)}/settings`,
  store: '/store',
  storeApp: (appId: string) => `/store/app/${encodeURIComponent(appId)}`,
  storeSource: (url: string) => `/store/source/${encodeURIComponent(url)}`,
  storeSourceApp: (url: string, appId: string) =>
    `/store/source/${encodeURIComponent(url)}/app/${encodeURIComponent(appId)}`,
  updates: '/updates',
  logs: '/logs',
  settings: '/settings',
} as const;

export const SECTIONS: { path: string; label: string; icon: IconName }[] = [
  { path: PATHS.devices, label: 'devices', icon: 'device' },
  { path: PATHS.apps, label: 'apps', icon: 'grid' },
  { path: PATHS.store, label: 'store', icon: 'store' },
  { path: PATHS.updates, label: 'updates', icon: 'download' },
  { path: PATHS.logs, label: 'logs', icon: 'terminal' },
  { path: PATHS.settings, label: 'settings', icon: 'gear' },
];

export function sectionFor(path: string): string | null {
  return SECTIONS.find(section => path === section.path || path.startsWith(`${section.path}/`))?.path ?? null;
}
