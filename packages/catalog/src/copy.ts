import type { StoreSource } from './store.ts';

export const STORE_COPY = {
  empty: 'no apps',
  noMatches: 'no matches',
  needsFirmware: 'needs newer firmware',
  unpublished: 'not published yet',
  unreviewed: 'not reviewed by bridgething',
  suggestedTitle: 'suggested sources',
  suggestedEmpty: 'none',
} as const;

function plural(count: number, word: string): string {
  return `${count} ${word}${count === 1 ? '' : 's'}`;
}

export function failureLine(failures: number): string {
  return `${plural(failures, 'source')} unreachable`;
}

export function installsLabel(installs: number): string | null {
  return installs > 0 ? plural(installs, 'install') : null;
}

export function alsoAvailableLabel(alsoAvailableFrom: readonly string[]): string | null {
  return alsoAvailableFrom.length > 0 ? `also offered by ${plural(alsoAvailableFrom.length, 'other source')}` : null;
}

const PERMISSION_LABELS: Record<string, string> = {
  'net.fetch': 'internet',
  'net.ws': 'websockets',
  'net.proxy': 'TCP tunnel',
  geo: 'location',
  notifications: 'notifications',
  audio: 'sound',
  'audio.tts': 'sound',
  mic: 'microphone',
};

export function permissionLabel(permission: string): string {
  return PERMISSION_LABELS[permission] ?? permission;
}

export function sourceLine(source: StoreSource | null | undefined): string {
  if (!source) return STORE_COPY.unreviewed;
  if (source.trust === 'official') return 'from bridgething';
  if (source.trust === 'attested') return `from ${source.name}`;
  return `from ${source.name} · ${STORE_COPY.unreviewed}`;
}
