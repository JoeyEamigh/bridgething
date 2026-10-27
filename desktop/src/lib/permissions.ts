import { permissionLabel } from '@bridgething/catalog';

import type { IconName } from './icons.tsx';

export type PermissionCopy = { icon: IconName; title: string };

const ICONS: Record<string, IconName> = {
  'net.fetch': 'globe',
  'net.ws': 'wifi',
  'net.proxy': 'plug',
  geo: 'pin',
  notifications: 'bell',
  audio: 'speaker',
  'audio.tts': 'speaker',
  mic: 'mic',
};

export function humanizePermission(permission: string): PermissionCopy {
  return { icon: ICONS[permission] ?? 'shield', title: permissionLabel(permission) };
}
