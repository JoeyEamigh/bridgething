export const SITE = {
  domain: 'bridgething.com',
  ota: 'https://ota.bridgething.com',
  manifestUrl: 'https://ota.bridgething.com/manifest.json',
  daemonManifestUrl: 'https://ota.bridgething.com/daemon/manifest.json',
  companionUrl: 'https://ota.bridgething.com/companion.json',
  officialCatalog: 'https://apps.bridgething.com/catalog.json',
  testflight: 'https://testflight.apple.com/join/PJHyDqZn',
  github: 'https://github.com/JoeyEamigh',
  repo: 'https://github.com/JoeyEamigh/bridgething',
  discord: 'https://tl.mt/d',
  pitch:
    'open firmware for the spotify car thing. a full system image, daemon, and companion app to restore your thing to its former glory.',
} as const;

export const TERBIUM = {
  base: 'https://terbium.app/',
  manifestParam: 'manifest',
  channelParam: 'channel',
} as const;

export function terbiumUrl(channel?: string): string {
  const params = new URLSearchParams({ [TERBIUM.manifestParam]: SITE.manifestUrl });
  if (channel) params.set(TERBIUM.channelParam, channel);
  return `${TERBIUM.base}?${params.toString()}`;
}
