import type { IconName } from './icons.tsx';

export type PermissionCopy = { icon: IconName; title: string };

export function humanizePermission(permission: string): PermissionCopy {
  switch (permission) {
    case 'net.fetch':
      return { icon: 'globe', title: 'internet' };
    case 'net.ws':
      return { icon: 'wifi', title: 'websockets' };
    case 'net.proxy':
      return { icon: 'plug', title: 'TCP tunnel' };
    case 'geo':
      return { icon: 'pin', title: 'location' };
    case 'notifications':
      return { icon: 'bell', title: 'notifications' };
    case 'audio':
    case 'audio.tts':
      return { icon: 'speaker', title: 'sound' };
    case 'mic':
      return { icon: 'mic', title: 'microphone' };
    default:
      return { icon: 'shield', title: permission };
  }
}
