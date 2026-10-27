import { permissionLabel } from '@bridgething/catalog';

import type { IconName } from '../components/Icon';

export type PermissionCopy = {
  icon: IconName;
  title: string;
};

const ICONS: Record<string, IconName> = {
  'net.fetch': 'Globe',
  'net.ws': 'Wifi',
  'net.proxy': 'Cable',
  geo: 'MapPin',
  notifications: 'Bell',
  audio: 'Speaker',
  'audio.tts': 'Speaker',
  mic: 'Mic',
};

export function humanizePermission(perm: string): PermissionCopy {
  return { icon: ICONS[perm] ?? 'Shield', title: permissionLabel(perm) };
}
